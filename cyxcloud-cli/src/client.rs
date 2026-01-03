//! Gateway Client
//!
//! HTTP client for communicating with the CyxCloud gateway.
//! Supports HTTPS with custom CA certificates and client certificates (mTLS).

use bytes::Bytes;
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;
use thiserror::Error;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Client errors
#[derive(Error, Debug)]
pub enum ClientError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("API error: {status} - {message}")]
    Api { status: u16, message: String },

    #[error("Not found: {0}")]
    NotFound(String),
}

pub type Result<T> = std::result::Result<T, ClientError>;

/// Object metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectInfo {
    pub key: String,
    pub size: u64,
    pub last_modified: String,
    pub etag: String,
}

/// List objects response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListResponse {
    pub objects: Vec<ObjectInfo>,
    pub is_truncated: bool,
    pub next_token: Option<String>,
}

/// Storage status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageStatus {
    pub total_files: u64,
    pub total_bytes: u64,
    pub buckets: Vec<BucketInfo>,
}

/// Bucket info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BucketInfo {
    pub name: String,
    pub object_count: u64,
    pub total_size: u64,
}

/// Dataset info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatasetInfo {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub owner_id: String,
    pub file_count: i64,
    pub size_bytes: i64,
    pub content_hash: Vec<u8>,
    pub trust_level: i32,
    pub version: String,
    pub created_at: String,
    pub updated_at: String,
}

/// Public dataset info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicDatasetInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub license: Option<String>,
    pub verified_by: Vec<String>,
    pub cached: bool,
}

/// Dataset verification result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationResult {
    pub manifest_valid: bool,
    pub all_files_valid: bool,
    pub files_verified: i32,
    pub files_failed: i32,
    pub trust_level: i32,
    pub public_match: Option<PublicDatasetMatch>,
}

/// Public dataset match
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicDatasetMatch {
    pub name: String,
    pub version: String,
    pub verified_by: Vec<String>,
    pub license: Option<String>,
}

/// Dataset share result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShareResult {
    pub success: bool,
    pub share_id: String,
}

/// TLS configuration for the gateway client
#[derive(Debug, Clone, Default)]
pub struct TlsConfig {
    /// Path to CA certificate (PEM format) for verifying the gateway
    pub ca_cert: Option<PathBuf>,
    /// Path to client certificate (PEM format) for mTLS
    pub client_cert: Option<PathBuf>,
    /// Path to client private key (PEM format) for mTLS
    pub client_key: Option<PathBuf>,
    /// Skip server certificate verification (DANGEROUS - only for development)
    pub danger_accept_invalid_certs: bool,
}

impl TlsConfig {
    /// Create a new TLS config with only CA certificate
    pub fn with_ca_cert(ca_cert: PathBuf) -> Self {
        Self {
            ca_cert: Some(ca_cert),
            ..Default::default()
        }
    }

    /// Create a full mTLS config
    pub fn with_mtls(ca_cert: PathBuf, client_cert: PathBuf, client_key: PathBuf) -> Self {
        Self {
            ca_cert: Some(ca_cert),
            client_cert: Some(client_cert),
            client_key: Some(client_key),
            danger_accept_invalid_certs: false,
        }
    }
}

/// Gateway client
pub struct GatewayClient {
    client: Client,
    base_url: String,
    auth_token: Option<String>,
}

impl GatewayClient {
    /// Create a new gateway client without TLS
    pub fn new(base_url: &str, auth_token: Option<String>) -> Self {
        Self::with_tls(base_url, auth_token, None)
    }

    /// Create a new gateway client with optional TLS configuration
    pub fn with_tls(base_url: &str, auth_token: Option<String>, tls: Option<TlsConfig>) -> Self {
        let mut builder = Client::builder().timeout(Duration::from_secs(300));

        if let Some(tls_config) = tls {
            // Add CA certificate
            if let Some(ref ca_path) = tls_config.ca_cert {
                if let Ok(ca_cert_pem) = std::fs::read(ca_path) {
                    if let Ok(cert) = reqwest::Certificate::from_pem(&ca_cert_pem) {
                        builder = builder.add_root_certificate(cert);
                    }
                }
            }

            // Add client identity for mTLS
            if let (Some(ref cert_path), Some(ref key_path)) =
                (&tls_config.client_cert, &tls_config.client_key)
            {
                if let (Ok(cert_pem), Ok(key_pem)) =
                    (std::fs::read(cert_path), std::fs::read(key_path))
                {
                    // Combine cert and key into a single PEM buffer
                    let mut identity_pem = cert_pem;
                    identity_pem.extend_from_slice(&key_pem);

                    if let Ok(identity) = reqwest::Identity::from_pem(&identity_pem) {
                        builder = builder.identity(identity);
                    }
                }
            }

            // Skip certificate verification (DANGEROUS)
            if tls_config.danger_accept_invalid_certs {
                builder = builder.danger_accept_invalid_certs(true);
            }
        }

        let client = builder.build().expect("Failed to create HTTP client");

        Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            auth_token,
        }
    }

    /// Add authorization header to a request if token is available
    fn auth_headers(&self) -> Option<String> {
        self.auth_token.as_ref().map(|t| format!("Bearer {}", t))
    }

    /// Check gateway health
    pub async fn health(&self) -> Result<bool> {
        let url = format!("{}/health", self.base_url);
        let response = self.client.get(&url).send().await?;
        Ok(response.status().is_success())
    }

    /// Create a bucket
    pub async fn create_bucket(&self, bucket: &str) -> Result<()> {
        let url = format!("{}/s3/{}", self.base_url, bucket);
        let mut req = self.client.put(&url);
        if let Some(auth) = self.auth_headers() {
            req = req.header("Authorization", auth);
        }
        let response = req.send().await?;

        if response.status().is_success() {
            Ok(())
        } else if response.status() == StatusCode::CONFLICT {
            // Bucket already exists, that's fine
            Ok(())
        } else {
            Err(ClientError::Api {
                status: response.status().as_u16(),
                message: response.text().await.unwrap_or_default(),
            })
        }
    }

    /// Upload a file
    pub async fn upload_file(
        &self,
        bucket: &str,
        key: &str,
        data: Bytes,
        content_type: &str,
    ) -> Result<String> {
        let url = format!("{}/s3/{}/{}", self.base_url, bucket, key);

        let mut req = self
            .client
            .put(&url)
            .header("Content-Type", content_type)
            .body(data);

        if let Some(auth) = self.auth_headers() {
            req = req.header("Authorization", auth);
        }

        let response = req.send().await?;

        if response.status().is_success() {
            let etag = response
                .headers()
                .get("etag")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .trim_matches('"')
                .to_string();
            Ok(etag)
        } else {
            Err(ClientError::Api {
                status: response.status().as_u16(),
                message: response.text().await.unwrap_or_default(),
            })
        }
    }

    /// Upload a local file
    pub async fn upload_local_file(
        &self,
        bucket: &str,
        key: &str,
        path: &Path,
    ) -> Result<(String, u64)> {
        let mut file = File::open(path).await?;
        let metadata = file.metadata().await?;
        let size = metadata.len();

        let mut data = Vec::with_capacity(size as usize);
        file.read_to_end(&mut data).await?;

        // Guess content type
        let content_type = mime_guess::from_path(path)
            .first_or_octet_stream()
            .to_string();

        let etag = self
            .upload_file(bucket, key, Bytes::from(data), &content_type)
            .await?;

        Ok((etag, size))
    }

    /// Download a file
    pub async fn download_file(&self, bucket: &str, key: &str) -> Result<Bytes> {
        let url = format!("{}/s3/{}/{}", self.base_url, bucket, key);

        let mut req = self.client.get(&url);
        if let Some(auth) = self.auth_headers() {
            req = req.header("Authorization", auth);
        }

        let response = req.send().await?;

        if response.status().is_success() {
            let bytes = response.bytes().await?;
            Ok(bytes)
        } else if response.status() == StatusCode::NOT_FOUND {
            Err(ClientError::NotFound(format!("{}/{}", bucket, key)))
        } else {
            Err(ClientError::Api {
                status: response.status().as_u16(),
                message: response.text().await.unwrap_or_default(),
            })
        }
    }

    /// Download to a local file
    pub async fn download_to_file(&self, bucket: &str, key: &str, path: &Path) -> Result<u64> {
        let data = self.download_file(bucket, key).await?;
        let size = data.len() as u64;

        let mut file = File::create(path).await?;
        file.write_all(&data).await?;

        Ok(size)
    }

    /// Get object metadata (HEAD request)
    pub async fn head_object(&self, bucket: &str, key: &str) -> Result<ObjectInfo> {
        let url = format!("{}/s3/{}/{}", self.base_url, bucket, key);

        let mut req = self.client.head(&url);
        if let Some(auth) = self.auth_headers() {
            req = req.header("Authorization", auth);
        }

        let response = req.send().await?;

        if response.status().is_success() {
            let headers = response.headers();

            let size = headers
                .get("content-length")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);

            let etag = headers
                .get("etag")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .trim_matches('"')
                .to_string();

            let last_modified = headers
                .get("last-modified")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string();

            Ok(ObjectInfo {
                key: key.to_string(),
                size,
                last_modified,
                etag,
            })
        } else if response.status() == StatusCode::NOT_FOUND {
            Err(ClientError::NotFound(format!("{}/{}", bucket, key)))
        } else {
            Err(ClientError::Api {
                status: response.status().as_u16(),
                message: "HEAD request failed".to_string(),
            })
        }
    }

    /// Delete an object
    pub async fn delete_object(&self, bucket: &str, key: &str) -> Result<()> {
        let url = format!("{}/s3/{}/{}", self.base_url, bucket, key);

        let mut req = self.client.delete(&url);
        if let Some(auth) = self.auth_headers() {
            req = req.header("Authorization", auth);
        }

        let response = req.send().await?;

        if response.status().is_success() || response.status() == StatusCode::NO_CONTENT {
            Ok(())
        } else {
            Err(ClientError::Api {
                status: response.status().as_u16(),
                message: response.text().await.unwrap_or_default(),
            })
        }
    }

    /// List objects in a bucket
    pub async fn list_objects(
        &self,
        bucket: &str,
        prefix: Option<&str>,
        max_keys: Option<i32>,
    ) -> Result<ListResponse> {
        let mut url = format!("{}/s3/{}", self.base_url, bucket);

        let mut params = Vec::new();
        params.push("list-type=2".to_string());

        if let Some(p) = prefix {
            params.push(format!("prefix={}", p));
        }
        if let Some(m) = max_keys {
            params.push(format!("max-keys={}", m));
        }

        if !params.is_empty() {
            url.push('?');
            url.push_str(&params.join("&"));
        }

        let mut req = self.client.get(&url);
        if let Some(auth) = self.auth_headers() {
            req = req.header("Authorization", auth);
        }

        let response = req.send().await?;

        if response.status().is_success() {
            // Parse XML response (simplified - would use quick-xml in production)
            let text = response.text().await?;
            let objects = parse_list_response(&text)?;
            Ok(objects)
        } else if response.status() == StatusCode::NOT_FOUND {
            Err(ClientError::NotFound(bucket.to_string()))
        } else {
            Err(ClientError::Api {
                status: response.status().as_u16(),
                message: response.text().await.unwrap_or_default(),
            })
        }
    }

    // ==================== Dataset API ====================

    /// List user's datasets
    pub async fn list_datasets(
        &self,
        include_shared: bool,
        limit: i32,
    ) -> Result<Vec<DatasetInfo>> {
        let url = format!(
            "{}/api/datasets?include_shared={}&limit={}",
            self.base_url, include_shared, limit
        );

        let mut req = self.client.get(&url);
        if let Some(auth) = self.auth_headers() {
            req = req.header("Authorization", auth);
        }

        let response = req.send().await?;

        if response.status().is_success() {
            let datasets: Vec<DatasetInfo> = response.json().await?;
            Ok(datasets)
        } else {
            Err(ClientError::Api {
                status: response.status().as_u16(),
                message: response.text().await.unwrap_or_default(),
            })
        }
    }

    /// List public datasets
    pub async fn list_public_datasets(
        &self,
        name_filter: Option<&str>,
    ) -> Result<Vec<PublicDatasetInfo>> {
        let mut url = format!("{}/api/datasets/public", self.base_url);
        if let Some(filter) = name_filter {
            url.push_str(&format!("?filter={}", filter));
        }

        let mut req = self.client.get(&url);
        if let Some(auth) = self.auth_headers() {
            req = req.header("Authorization", auth);
        }

        let response = req.send().await?;

        if response.status().is_success() {
            let datasets: Vec<PublicDatasetInfo> = response.json().await?;
            Ok(datasets)
        } else {
            Err(ClientError::Api {
                status: response.status().as_u16(),
                message: response.text().await.unwrap_or_default(),
            })
        }
    }

    /// Create a dataset from files in a bucket
    pub async fn create_dataset(
        &self,
        name: &str,
        description: Option<&str>,
        bucket: &str,
        prefix: Option<&str>,
    ) -> Result<DatasetInfo> {
        let url = format!("{}/api/datasets", self.base_url);

        #[derive(Serialize)]
        struct CreateDatasetRequest<'a> {
            name: &'a str,
            description: Option<&'a str>,
            bucket: &'a str,
            prefix: Option<&'a str>,
        }

        let body = CreateDatasetRequest {
            name,
            description,
            bucket,
            prefix,
        };

        let mut req = self.client.post(&url).json(&body);
        if let Some(auth) = self.auth_headers() {
            req = req.header("Authorization", auth);
        }

        let response = req.send().await?;

        if response.status().is_success() {
            let dataset: DatasetInfo = response.json().await?;
            Ok(dataset)
        } else {
            Err(ClientError::Api {
                status: response.status().as_u16(),
                message: response.text().await.unwrap_or_default(),
            })
        }
    }

    /// Verify a dataset's integrity
    pub async fn verify_dataset(
        &self,
        dataset_id: &str,
        check_public: bool,
        full_verification: bool,
    ) -> Result<VerificationResult> {
        let url = format!(
            "{}/api/datasets/{}/verify?check_public={}&full={}",
            self.base_url, dataset_id, check_public, full_verification
        );

        let mut req = self.client.post(&url);
        if let Some(auth) = self.auth_headers() {
            req = req.header("Authorization", auth);
        }

        let response = req.send().await?;

        if response.status().is_success() {
            let result: VerificationResult = response.json().await?;
            Ok(result)
        } else if response.status() == StatusCode::NOT_FOUND {
            Err(ClientError::NotFound(dataset_id.to_string()))
        } else {
            Err(ClientError::Api {
                status: response.status().as_u16(),
                message: response.text().await.unwrap_or_default(),
            })
        }
    }

    /// Share a dataset with another user
    pub async fn share_dataset(
        &self,
        dataset_id: &str,
        share_with: &str,
        permissions: &[String],
    ) -> Result<ShareResult> {
        let url = format!("{}/api/datasets/{}/share", self.base_url, dataset_id);

        #[derive(Serialize)]
        struct ShareRequest<'a> {
            share_with: &'a str,
            permissions: &'a [String],
        }

        let body = ShareRequest {
            share_with,
            permissions,
        };

        let mut req = self.client.post(&url).json(&body);
        if let Some(auth) = self.auth_headers() {
            req = req.header("Authorization", auth);
        }

        let response = req.send().await?;

        if response.status().is_success() {
            let result: ShareResult = response.json().await?;
            Ok(result)
        } else if response.status() == StatusCode::NOT_FOUND {
            Err(ClientError::NotFound(dataset_id.to_string()))
        } else {
            Err(ClientError::Api {
                status: response.status().as_u16(),
                message: response.text().await.unwrap_or_default(),
            })
        }
    }

    /// Get detailed dataset information
    pub async fn get_dataset_info(&self, dataset_id: &str) -> Result<DatasetInfo> {
        let url = format!("{}/api/datasets/{}", self.base_url, dataset_id);

        let mut req = self.client.get(&url);
        if let Some(auth) = self.auth_headers() {
            req = req.header("Authorization", auth);
        }

        let response = req.send().await?;

        if response.status().is_success() {
            let dataset: DatasetInfo = response.json().await?;
            Ok(dataset)
        } else if response.status() == StatusCode::NOT_FOUND {
            Err(ClientError::NotFound(dataset_id.to_string()))
        } else {
            Err(ClientError::Api {
                status: response.status().as_u16(),
                message: response.text().await.unwrap_or_default(),
            })
        }
    }
}

/// Parse S3 ListBucketResult XML (simplified)
fn parse_list_response(xml: &str) -> Result<ListResponse> {
    let mut objects = Vec::new();
    let mut is_truncated = false;

    // Simple XML parsing (in production, use quick-xml)
    for line in xml.lines() {
        let line = line.trim();

        if line.contains("<IsTruncated>true</IsTruncated>") {
            is_truncated = true;
        }

        if line.starts_with("<Contents>") || line.contains("<Key>") {
            // Try to extract object info
            if let Some(key) = extract_xml_value(xml, "Key") {
                let size = extract_xml_value(xml, "Size")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
                let last_modified = extract_xml_value(xml, "LastModified").unwrap_or_default();
                let etag = extract_xml_value(xml, "ETag")
                    .unwrap_or_default()
                    .trim_matches('"')
                    .to_string();

                objects.push(ObjectInfo {
                    key,
                    size,
                    last_modified,
                    etag,
                });
                break; // Only get first for now - proper parsing would iterate
            }
        }
    }

    // Better parsing: find all <Contents> blocks
    let contents_regex = "<Contents>";
    let mut start = 0;
    while let Some(pos) = xml[start..].find(contents_regex) {
        let block_start = start + pos;
        if let Some(end_pos) = xml[block_start..].find("</Contents>") {
            let block = &xml[block_start..block_start + end_pos + 11];

            if let (Some(key), Some(size_str)) = (
                extract_xml_value(block, "Key"),
                extract_xml_value(block, "Size"),
            ) {
                let size = size_str.parse().unwrap_or(0);
                let last_modified = extract_xml_value(block, "LastModified").unwrap_or_default();
                let etag = extract_xml_value(block, "ETag")
                    .unwrap_or_default()
                    .trim_matches('"')
                    .to_string();

                // Avoid duplicates
                if !objects.iter().any(|o| o.key == key) {
                    objects.push(ObjectInfo {
                        key,
                        size,
                        last_modified,
                        etag,
                    });
                }
            }

            start = block_start + end_pos + 11;
        } else {
            break;
        }
    }

    Ok(ListResponse {
        objects,
        is_truncated,
        next_token: None,
    })
}

/// Extract value from XML tag
fn extract_xml_value(xml: &str, tag: &str) -> Option<String> {
    let open_tag = format!("<{}>", tag);
    let close_tag = format!("</{}>", tag);

    let start = xml.find(&open_tag)? + open_tag.len();
    let end = xml[start..].find(&close_tag)?;

    Some(xml[start..start + end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_xml_value() {
        let xml = "<Key>test/file.txt</Key>";
        assert_eq!(
            extract_xml_value(xml, "Key"),
            Some("test/file.txt".to_string())
        );
    }

    #[test]
    fn test_parse_list_response() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<ListBucketResult>
  <Name>test-bucket</Name>
  <IsTruncated>false</IsTruncated>
  <Contents>
    <Key>file1.txt</Key>
    <Size>1024</Size>
    <LastModified>2024-01-01T00:00:00Z</LastModified>
    <ETag>"abc123"</ETag>
  </Contents>
  <Contents>
    <Key>file2.txt</Key>
    <Size>2048</Size>
    <LastModified>2024-01-02T00:00:00Z</LastModified>
    <ETag>"def456"</ETag>
  </Contents>
</ListBucketResult>"#;

        let result = parse_list_response(xml).unwrap();
        assert_eq!(result.objects.len(), 2);
        assert_eq!(result.objects[0].key, "file1.txt");
        assert_eq!(result.objects[0].size, 1024);
        assert!(!result.is_truncated);
    }
}
