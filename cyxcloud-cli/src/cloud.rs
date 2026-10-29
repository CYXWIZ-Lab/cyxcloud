//! Datasets over the gateway's gRPC services: the same calls the CyxWiz
//! Engine makes (DataStreamClient), so the CLI, the Engine and the web see the
//! same objects (TOFIX177 slice 2, TOFIX164).

use anyhow::{bail, Context, Result};
use cyxcloud_protocol::data::data_service_client::DataServiceClient;
use cyxcloud_protocol::data::StreamDataRequest;
use cyxcloud_protocol::datastream::data_stream_service_client::DataStreamServiceClient;
use cyxcloud_protocol::datastream::{
    upload_file_request, CreateAccessTokenRequest, CreateDatasetRequest, DatasetFileInfo,
    DatasetInfo, DeleteDatasetRequest, GetDatasetInfoRequest, ListDatasetsRequest,
    ListPublicDatasetsRequest, PublicDatasetInfo, UploadFileMetadata, UploadFileRequest,
};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tonic::transport::{Channel, ClientTlsConfig, Endpoint};

/// Upload pieces, as the Engine sends them.
pub const UPLOAD_CHUNK: usize = 1024 * 1024;
const MAX_MESSAGE: usize = 64 << 20;

/// A signed-in connection to the gateway's dataset services.
pub struct Cloud {
    channel: Channel,
    bearer: tonic::metadata::MetadataValue<tonic::metadata::Ascii>,
}

impl Cloud {
    pub async fn connect(grpc_url: &str, jwt: &str) -> Result<Self> {
        let mut endpoint = Endpoint::from_shared(grpc_url.to_string())
            .with_context(|| format!("'{}' is not a gateway address", grpc_url))?
            .connect_timeout(std::time::Duration::from_secs(10));
        if grpc_url.starts_with("https://") {
            endpoint = endpoint.tls_config(ClientTlsConfig::new())?;
        }
        let channel = endpoint.connect().await.with_context(|| {
            format!(
                "cannot reach the storage gateway at {} (run `cyxcloud doctor`)",
                grpc_url
            )
        })?;
        let bearer = format!("Bearer {}", jwt)
            .parse()
            .context("the saved sign-in is not a valid token: run `cyxcloud login`")?;
        Ok(Self { channel, bearer })
    }

    fn request<T>(&self, inner: T) -> tonic::Request<T> {
        let mut r = tonic::Request::new(inner);
        r.metadata_mut()
            .insert("authorization", self.bearer.clone());
        r
    }

    fn datasets(&self) -> DataStreamServiceClient<Channel> {
        DataStreamServiceClient::new(self.channel.clone())
            .max_decoding_message_size(MAX_MESSAGE)
            .max_encoding_message_size(MAX_MESSAGE)
    }

    pub async fn list(&self, include_shared: bool) -> Result<Vec<DatasetInfo>> {
        let r = self
            .datasets()
            .list_datasets(self.request(ListDatasetsRequest {
                limit: 1000,
                offset: 0,
                include_shared,
                max_trust_level: 4,
            }))
            .await
            .map_err(status_error)?;
        Ok(r.into_inner().datasets)
    }

    pub async fn list_public(&self, filter: Option<String>) -> Result<Vec<PublicDatasetInfo>> {
        let r = self
            .datasets()
            .list_public_datasets(self.request(ListPublicDatasetsRequest {
                name_filter: filter.unwrap_or_default(),
            }))
            .await
            .map_err(status_error)?;
        Ok(r.into_inner().datasets)
    }

    pub async fn info(&self, dataset_id: &str) -> Result<(DatasetInfo, Vec<DatasetFileInfo>)> {
        let r = self
            .datasets()
            .get_dataset_info(self.request(GetDatasetInfoRequest {
                dataset_id: dataset_id.to_string(),
            }))
            .await
            .map_err(status_error)?
            .into_inner();
        let dataset = r
            .dataset
            .context("the gateway answered without the dataset")?;
        Ok((dataset, r.files))
    }

    /// Upload one file; `progress(bytes_sent)` after each piece. Returns the file id.
    pub async fn upload(&self, path: &Path, mut progress: impl FnMut(u64)) -> Result<String> {
        let data = tokio::fs::read(path)
            .await
            .with_context(|| format!("cannot read {}", path.display()))?;
        let filename = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .context("the path has no file name")?;
        let hash = cyxcloud_core::ContentHash::compute(&data);
        let size = data.len();
        let mut messages = vec![UploadFileRequest {
            request: Some(upload_file_request::Request::Metadata(UploadFileMetadata {
                filename,
                size_bytes: size as i64,
                content_type: content_type_for(path).to_string(),
                content_hash: hash.as_bytes().to_vec(),
            })),
        }];
        for piece in data.chunks(UPLOAD_CHUNK) {
            messages.push(UploadFileRequest {
                request: Some(upload_file_request::Request::Chunk(piece.to_vec())),
            });
        }
        drop(data);
        let sent = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let counter = sent.clone();
        let stream = tokio_stream::iter(messages.into_iter().inspect(move |m| {
            if let Some(upload_file_request::Request::Chunk(c)) = &m.request {
                counter.fetch_add(c.len() as u64, std::sync::atomic::Ordering::Relaxed);
            }
        }));
        let mut client = self.datasets();
        let call = client.upload_file(self.request(stream));
        tokio::pin!(call);
        let response = loop {
            tokio::select! {
                r = &mut call => break r,
                _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {
                    progress(sent.load(std::sync::atomic::Ordering::Relaxed));
                }
            }
        };
        progress(size as u64);
        let r = response.map_err(status_error)?.into_inner();
        if !r.success {
            bail!("the gateway refused the upload: {}", r.error);
        }
        Ok(r.file_id)
    }

    pub async fn create(
        &self,
        name: &str,
        description: &str,
        file_ids: Vec<String>,
    ) -> Result<DatasetInfo> {
        let r = self
            .datasets()
            .create_dataset(self.request(CreateDatasetRequest {
                name: name.to_string(),
                description: description.to_string(),
                file_ids,
                schema_json: String::new(),
            }))
            .await
            .map_err(status_error)?;
        r.into_inner()
            .dataset
            .context("the gateway answered without the dataset")
    }

    pub async fn delete(&self, dataset_id: &str) -> Result<()> {
        let r = self
            .datasets()
            .delete_dataset(self.request(DeleteDatasetRequest {
                dataset_id: dataset_id.to_string(),
            }))
            .await
            .map_err(status_error)?
            .into_inner();
        if !r.success {
            bail!("the gateway did not delete the dataset: {}", r.error);
        }
        Ok(())
    }

    /// A scoped access token for a dataset (what a training node streams with).
    pub async fn token(&self, dataset_id: &str, ttl_seconds: i64) -> Result<(String, i64)> {
        let r = self
            .datasets()
            .create_access_token(self.request(CreateAccessTokenRequest {
                dataset_id: dataset_id.to_string(),
                node_id: String::new(),
                scopes: vec!["read".into(), "stream".into()],
                ttl_seconds,
            }))
            .await
            .map_err(status_error)?
            .into_inner();
        Ok((r.token, r.expires_at))
    }

    /// Stream one file of a dataset into `out`, checking the gateway's SHA-256
    /// trailer; the file is written next to `out` first and only renamed into
    /// place once it verifies.
    pub async fn download(
        &self,
        file_id: &str,
        access_token: &str,
        out: &Path,
        mut progress: impl FnMut(u64),
    ) -> Result<u64> {
        let mut client =
            DataServiceClient::new(self.channel.clone()).max_decoding_message_size(MAX_MESSAGE);
        let mut stream = client
            .stream_data(StreamDataRequest {
                dataset_id: file_id.to_string(),
                batch_size: 1024,
                shuffle: false,
                prefetch: 4,
                start_epoch: 0,
                access_token: access_token.to_string(),
            })
            .await
            .map_err(status_error)?
            .into_inner();
        let partial = partial_path(out);
        let mut file = tokio::fs::File::create(&partial)
            .await
            .with_context(|| format!("cannot write {}", partial.display()))?;
        let mut hasher = Sha256::new();
        let mut total: u64 = 0;
        let mut trailer: Option<(String, u64)> = None;
        let result: Result<()> = async {
            use tokio::io::AsyncWriteExt;
            while let Some(chunk) = stream.message().await.map_err(status_error)? {
                file.write_all(&chunk.data).await?;
                hasher.update(&chunk.data);
                total += chunk.data.len() as u64;
                progress(total);
                if chunk.is_last {
                    trailer = Some((chunk.stream_sha256, chunk.total_bytes));
                }
            }
            file.flush().await?;
            Ok(())
        }
        .await;
        drop(file);
        if let Err(e) = result {
            let _ = std::fs::remove_file(&partial);
            return Err(e);
        }
        let digest = hex::encode(hasher.finalize());
        match trailer {
            Some((sha, bytes)) if sha.eq_ignore_ascii_case(&digest) && bytes == total => {}
            Some((sha, bytes)) => {
                let _ = std::fs::remove_file(&partial);
                bail!(
                    "the download does not match what the gateway sent ({} bytes, sha256 {}; expected {} bytes, sha256 {}); nothing was written",
                    total, digest, bytes, sha
                );
            }
            None => {
                let _ = std::fs::remove_file(&partial);
                bail!("the stream ended before its last piece; nothing was written");
            }
        }
        std::fs::rename(&partial, out)
            .with_context(|| format!("cannot move the download to {}", out.display()))?;
        Ok(total)
    }
}

/// Words for a gRPC failure.
pub fn status_error(status: tonic::Status) -> anyhow::Error {
    use tonic::Code;
    let hint = match status.code() {
        Code::Unauthenticated => " (sign in again: cyxcloud login)",
        Code::PermissionDenied => " (this account may not do that)",
        Code::NotFound => "",
        Code::Unavailable => " (the gateway is not reachable: cyxcloud doctor)",
        _ => "",
    };
    crate::exit::coded(
        crate::exit::grpc_kind(status.code()),
        format!("{}{}", status.message(), hint),
    )
}

/// `name` as a file name in `dir`: the last path part only, so a name the
/// server sends ("../../.bashrc", "C:\\x") cannot write outside `dir`.
pub fn safe_file_name(dir: &Path, name: &str) -> Option<PathBuf> {
    let last = name.rsplit(['/', '\\']).next()?.trim();
    if last.is_empty() || last == "." || last == ".." || last.contains(':') {
        return None;
    }
    Some(dir.join(last))
}

fn partial_path(out: &Path) -> PathBuf {
    let mut name = out
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".partial");
    out.with_file_name(name)
}

/// The MIME type the gateway records for a file.
pub fn content_type_for(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("parquet") => "application/vnd.apache.parquet",
        Some("arrow") | Some("feather") | Some("ipc") => "application/vnd.apache.arrow.file",
        Some("csv") => "text/csv",
        Some("json") => "application/json",
        Some("jsonl") => "application/jsonl",
        Some("txt") => "text/plain",
        _ => "application/octet-stream",
    }
}

/// A dataset id, or the id of the one dataset with this name.
pub fn resolve_dataset<'a>(
    datasets: &'a [DatasetInfo],
    name_or_id: &str,
) -> Result<&'a DatasetInfo> {
    if let Some(d) = datasets.iter().find(|d| d.id == name_or_id) {
        return Ok(d);
    }
    let by_prefix: Vec<&DatasetInfo> = if name_or_id.len() >= 8 {
        datasets
            .iter()
            .filter(|d| d.id.starts_with(name_or_id))
            .collect()
    } else {
        Vec::new()
    };
    let by_name: Vec<&DatasetInfo> = datasets.iter().filter(|d| d.name == name_or_id).collect();
    match (by_name.as_slice(), by_prefix.as_slice()) {
        ([one], _) => Ok(one),
        ([], [one]) => Ok(one),
        ([], []) => bail!(
            "no dataset named or with id '{}' (cyxcloud dataset ls)",
            name_or_id
        ),
        (many, _) if many.len() > 1 => bail!(
            "{} datasets are named '{}'; use the id: {}",
            many.len(),
            name_or_id,
            many.iter()
                .map(|d| short_id(&d.id))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        (_, many) => bail!(
            "'{}' matches {} dataset ids; give more of it",
            name_or_id,
            many.len()
        ),
    }
}

pub fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ds(id: &str, name: &str) -> DatasetInfo {
        DatasetInfo {
            id: id.into(),
            name: name.into(),
            ..Default::default()
        }
    }

    #[test]
    fn datasets_are_found_by_name_id_or_id_prefix() {
        let all = vec![
            ds("f8ccb4ed-dda2-4b37-a693-61657fd6a257", "tiny_causal_lm"),
            ds("968cedb3-2fb4-49b6-b2c6-12ca29e2666d", "twonode_blob"),
        ];
        assert_eq!(
            resolve_dataset(&all, "tiny_causal_lm").unwrap().name,
            "tiny_causal_lm"
        );
        assert_eq!(
            resolve_dataset(&all, "968cedb3").unwrap().name,
            "twonode_blob"
        );
        assert_eq!(
            resolve_dataset(&all, "f8ccb4ed-dda2-4b37-a693-61657fd6a257")
                .unwrap()
                .name,
            "tiny_causal_lm"
        );
        assert!(resolve_dataset(&all, "nope")
            .unwrap_err()
            .to_string()
            .contains("dataset ls"));
        assert!(
            resolve_dataset(&all, "968").is_err(),
            "short prefixes are not ids"
        );
    }

    #[test]
    fn two_datasets_with_one_name_need_the_id() {
        let all = vec![ds("aaaaaaaa-1", "x"), ds("bbbbbbbb-2", "x")];
        let err = resolve_dataset(&all, "x").unwrap_err().to_string();
        assert!(
            err.contains("aaaaaaaa") && err.contains("bbbbbbbb"),
            "{err}"
        );
    }

    #[test]
    fn server_file_names_cannot_leave_the_folder() {
        let dir = Path::new("/tmp/out");
        assert_eq!(
            safe_file_name(dir, "tokens.parquet"),
            Some(dir.join("tokens.parquet"))
        );
        assert_eq!(
            safe_file_name(dir, "u-a78/tokens.parquet"),
            Some(dir.join("tokens.parquet"))
        );
        assert_eq!(
            safe_file_name(dir, "../../.bashrc"),
            Some(dir.join(".bashrc"))
        );
        assert_eq!(
            safe_file_name(dir, r"..\..\evil.exe"),
            Some(dir.join("evil.exe"))
        );
        assert_eq!(safe_file_name(dir, ".."), None);
        assert_eq!(safe_file_name(dir, "a/"), None);
        assert_eq!(safe_file_name(dir, "C:evil"), None);
    }

    #[test]
    fn content_types_follow_the_extension() {
        assert_eq!(
            content_type_for(Path::new("a.parquet")),
            "application/vnd.apache.parquet"
        );
        assert_eq!(
            content_type_for(Path::new("a.ARROW")),
            "application/vnd.apache.arrow.file"
        );
        assert_eq!(
            content_type_for(Path::new("a.bin")),
            "application/octet-stream"
        );
    }

    #[test]
    fn partial_downloads_sit_next_to_the_target() {
        assert_eq!(
            partial_path(Path::new("/d/tokens.parquet")),
            PathBuf::from("/d/tokens.parquet.partial")
        );
    }
}
