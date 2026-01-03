//! DataStream gRPC Service
//!
//! Zero-copy ML training data streaming with cryptographic verification.
//! Provides:
//! - Dataset management (create, list, share)
//! - Batch streaming for training
//! - Data access tokens for Server Nodes
//! - Verification against public dataset registry

use crate::grpc_api::RequestClaimsExt;
use crate::AppState;
use cyxcloud_metadata::{
    CreateDataAccessToken, CreateDataset, CreateDatasetFile, CreateDatasetShare, Dataset,
    DatasetFile, MetadataService, PublicDataset, TrustLevel,
};
use cyxcloud_protocol::datastream::{
    data_stream_service_server::DataStreamService, AccessTokenResponse, BatchResponse,
    CreateAccessTokenRequest, CreateDatasetRequest, CreateDatasetResponse, DatasetFileInfo,
    DatasetInfo, DatasetInfoResponse, FileVerification, GetDatasetInfoRequest,
    ListDatasetsRequest, ListDatasetsResponse, ListPublicDatasetsRequest,
    ListPublicDatasetsResponse, PublicDatasetInfo, PublicDatasetMatch, RevokeAccessTokenRequest,
    RevokeAccessTokenResponse, ShareDatasetRequest, ShareDatasetResponse, StreamBatchesRequest,
    TrustLevel as ProtoTrustLevel, VerificationResult, VerifyDatasetRequest,
};
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::Stream;
use tonic::{Request, Response, Status};
use tracing::{debug, info, instrument};
use uuid::Uuid;

/// gRPC DataStream Service implementation
pub struct DataStreamServiceImpl {
    state: Arc<AppState>,
}

impl DataStreamServiceImpl {
    /// Create a new DataStreamService with application state
    pub fn new(state: Arc<AppState>) -> Self {
        Self { state }
    }

    /// Get metadata service from state
    fn metadata(&self) -> Result<&MetadataService, Status> {
        self.state
            .metadata_service()
            .ok_or_else(|| Status::unavailable("Metadata service not configured"))
    }

    /// Get user ID from request claims
    fn get_user_id(request: &Request<impl std::any::Any>) -> Result<Uuid, Status> {
        let claims = request.require_auth()?;
        Uuid::parse_str(&claims.sub)
            .map_err(|_| Status::internal("Invalid user ID in token"))
    }

    /// Convert internal Dataset to proto DatasetInfo
    fn dataset_to_proto(dataset: &Dataset) -> DatasetInfo {
        DatasetInfo {
            id: dataset.id.to_string(),
            name: dataset.name.clone(),
            owner_id: dataset.owner_id.to_string(),
            description: dataset.description.clone().unwrap_or_default(),
            total_size_bytes: dataset.total_size_bytes,
            file_count: dataset.file_count,
            content_hash: dataset.content_hash.clone(),
            trust_level: dataset.trust_level,
            signature: dataset.signature.clone().unwrap_or_default(),
            schema_json: dataset
                .schema
                .as_ref()
                .map(|s| s.to_string())
                .unwrap_or_default(),
            version: dataset.version,
            created_at: dataset.created_at.timestamp(),
            updated_at: dataset.updated_at.timestamp(),
        }
    }

    /// Convert internal DatasetFile to proto DatasetFileInfo
    fn dataset_file_to_proto(file: &DatasetFile) -> DatasetFileInfo {
        DatasetFileInfo {
            id: file.id.to_string(),
            path_in_dataset: file.path_in_dataset.clone(),
            size_bytes: file.size_bytes,
            content_hash: file.content_hash.clone(),
            file_index: file.file_index,
        }
    }

    /// Convert internal PublicDataset to proto PublicDatasetInfo
    fn public_dataset_to_proto(dataset: &PublicDataset) -> PublicDatasetInfo {
        PublicDatasetInfo {
            id: dataset.id.to_string(),
            name: dataset.name.clone(),
            version: dataset.version.clone(),
            official_url: dataset.official_url.clone(),
            official_hash: dataset.official_hash.clone(),
            paper_url: dataset.paper_url.clone().unwrap_or_default(),
            license: dataset.license.clone().unwrap_or_default(),
            verified_by: dataset.verified_by.clone(),
            cached: dataset.cached_dataset_id.is_some(),
            cached_dataset_id: dataset
                .cached_dataset_id
                .map(|id| id.to_string())
                .unwrap_or_default(),
        }
    }

    /// Convert proto TrustLevel to internal
    fn proto_to_trust_level(level: i32) -> TrustLevel {
        TrustLevel::from_i32(level)
    }
}

#[tonic::async_trait]
impl DataStreamService for DataStreamServiceImpl {
    type StreamBatchesStream =
        Pin<Box<dyn Stream<Item = Result<BatchResponse, Status>> + Send + 'static>>;

    // =========================================================================
    // STREAM BATCHES
    // =========================================================================

    #[instrument(skip(self, request), fields(dataset_id))]
    async fn stream_batches(
        &self,
        request: Request<StreamBatchesRequest>,
    ) -> Result<Response<Self::StreamBatchesStream>, Status> {
        let req = request.into_inner();
        let dataset_id_str = req.dataset_id.clone();
        tracing::Span::current().record("dataset_id", &dataset_id_str);

        info!(
            dataset_id = %dataset_id_str,
            batch_size = req.batch_size,
            shuffle = req.shuffle,
            max_trust_level = req.max_trust_level,
            "Starting batch stream"
        );

        let metadata = self.metadata()?;

        // Parse dataset ID
        let dataset_id = Uuid::parse_str(&dataset_id_str)
            .map_err(|e| Status::invalid_argument(format!("Invalid dataset_id: {}", e)))?;

        // Get dataset info
        let dataset = metadata
            .database()
            .get_dataset(dataset_id)
            .await
            .map_err(|e| Status::internal(format!("Database error: {}", e)))?
            .ok_or_else(|| Status::not_found("Dataset not found"))?;

        // Check trust level
        let max_trust = if req.max_trust_level > 0 {
            req.max_trust_level
        } else {
            TrustLevel::Verified as i32
        };
        if dataset.trust_level > max_trust {
            return Err(Status::permission_denied(format!(
                "Dataset trust level {} exceeds maximum allowed {}",
                dataset.trust_level, max_trust
            )));
        }

        // Get dataset files
        let files = metadata
            .database()
            .get_dataset_files(dataset_id)
            .await
            .map_err(|e| Status::internal(format!("Database error: {}", e)))?;

        if files.is_empty() {
            return Err(Status::not_found("Dataset has no files"));
        }

        info!(
            dataset_id = %dataset_id_str,
            file_count = files.len(),
            "Starting batch stream for dataset"
        );

        // Create channel for streaming
        let (tx, rx) = mpsc::channel(req.prefetch_batches.max(1) as usize);

        let metadata_arc = self.state.metadata_service_arc();
        let node_client = self.state.node_client_arc();
        let batch_size = req.batch_size.max(1) as usize;
        let shuffle = req.shuffle;
        let seed = req.seed;

        // Spawn task to stream batches
        tokio::spawn(async move {
            // Optionally shuffle file indices
            let file_indices: Vec<usize> = if shuffle {
                use rand::seq::SliceRandom;
                use rand::SeedableRng;
                let mut indices: Vec<usize> = (0..files.len()).collect();
                if seed != 0 {
                    let mut rng = rand::rngs::StdRng::seed_from_u64(seed as u64);
                    indices.shuffle(&mut rng);
                } else {
                    let mut rng = rand::thread_rng();
                    indices.shuffle(&mut rng);
                }
                indices
            } else {
                (0..files.len()).collect()
            };

            let total_batches = (files.len() + batch_size - 1) / batch_size;
            let mut batch_index: u64 = 0;
            let mut current_batch_items: Vec<Vec<u8>> = Vec::with_capacity(batch_size);
            let mut current_batch_hashes: Vec<Vec<u8>> = Vec::with_capacity(batch_size);

            for (i, &file_idx) in file_indices.iter().enumerate() {
                let file = &files[file_idx];
                let is_last_file = i == file_indices.len() - 1;

                // Get the actual file data
                if let Some(ref meta) = metadata_arc {
                    // Get file from storage
                    if let Ok(Some(_stored_file)) = meta.database().get_file(file.file_id).await {
                        // Get chunks for this file
                        if let Ok(chunks) = meta.get_file_chunks(file.file_id).await {
                            // Retrieve and assemble file data
                            let mut file_data = Vec::new();
                            for chunk in &chunks {
                                if let Ok(addrs) = meta.get_chunk_locations(&chunk.chunk_id).await {
                                    if !addrs.is_empty() {
                                        if let Ok(data) = node_client
                                            .get_chunk_from_any(&addrs, &chunk.chunk_id)
                                            .await
                                        {
                                            file_data.extend_from_slice(&data);
                                        }
                                    }
                                }
                            }

                            if !file_data.is_empty() {
                                // Compute hash for verification
                                let item_hash = blake3::hash(&file_data).as_bytes().to_vec();

                                current_batch_items.push(file_data);
                                current_batch_hashes.push(item_hash);
                            }
                        }
                    }
                }

                // Send batch when full or at end
                let should_send = current_batch_items.len() >= batch_size || is_last_file;
                if should_send && !current_batch_items.is_empty() {
                    // Compute batch hash
                    let mut hasher = blake3::Hasher::new();
                    for hash in &current_batch_hashes {
                        hasher.update(hash);
                    }
                    let batch_hash = hasher.finalize().as_bytes().to_vec();

                    let response = BatchResponse {
                        batch_index,
                        items: std::mem::take(&mut current_batch_items),
                        item_hashes: std::mem::take(&mut current_batch_hashes),
                        batch_hash,
                        total_batches: total_batches as u64,
                        is_last: is_last_file,
                    };

                    if tx.send(Ok(response)).await.is_err() {
                        debug!("Client disconnected, stopping stream");
                        break;
                    }

                    batch_index += 1;
                }
            }

            debug!(dataset_id = %dataset_id_str, "Batch stream completed");
        });

        let stream = ReceiverStream::new(rx);
        Ok(Response::new(Box::pin(stream) as Self::StreamBatchesStream))
    }

    // =========================================================================
    // GET DATASET INFO
    // =========================================================================

    #[instrument(skip(self, request), fields(dataset_id))]
    async fn get_dataset_info(
        &self,
        request: Request<GetDatasetInfoRequest>,
    ) -> Result<Response<DatasetInfoResponse>, Status> {
        let req = request.into_inner();
        tracing::Span::current().record("dataset_id", &req.dataset_id);

        let metadata = self.metadata()?;

        let dataset_id = Uuid::parse_str(&req.dataset_id)
            .map_err(|e| Status::invalid_argument(format!("Invalid dataset_id: {}", e)))?;

        let dataset = metadata
            .database()
            .get_dataset(dataset_id)
            .await
            .map_err(|e| Status::internal(format!("Database error: {}", e)))?
            .ok_or_else(|| Status::not_found("Dataset not found"))?;

        let files = metadata
            .database()
            .get_dataset_files(dataset_id)
            .await
            .map_err(|e| Status::internal(format!("Database error: {}", e)))?;

        debug!(
            dataset_id = %req.dataset_id,
            file_count = files.len(),
            "Retrieved dataset info"
        );

        Ok(Response::new(DatasetInfoResponse {
            dataset: Some(Self::dataset_to_proto(&dataset)),
            files: files.iter().map(Self::dataset_file_to_proto).collect(),
        }))
    }

    // =========================================================================
    // LIST DATASETS
    // =========================================================================

    #[instrument(skip(self, request))]
    async fn list_datasets(
        &self,
        request: Request<ListDatasetsRequest>,
    ) -> Result<Response<ListDatasetsResponse>, Status> {
        let user_id = Self::get_user_id(&request)?;
        let req = request.into_inner();

        let metadata = self.metadata()?;

        let datasets = if req.include_shared {
            metadata
                .database()
                .get_user_accessible_datasets(user_id)
                .await
                .map_err(|e| Status::internal(format!("Database error: {}", e)))?
        } else {
            metadata
                .database()
                .get_datasets_by_owner(user_id)
                .await
                .map_err(|e| Status::internal(format!("Database error: {}", e)))?
        };

        // Filter by trust level if specified
        let filtered: Vec<_> = if req.max_trust_level > 0 {
            datasets
                .into_iter()
                .filter(|d| d.trust_level <= req.max_trust_level)
                .collect()
        } else {
            datasets
        };

        let total_count = filtered.len() as i32;

        // Apply pagination
        let offset = req.offset.max(0) as usize;
        let limit = if req.limit > 0 { req.limit as usize } else { 100 };
        let paginated: Vec<_> = filtered
            .into_iter()
            .skip(offset)
            .take(limit)
            .map(|d| Self::dataset_to_proto(&d))
            .collect();

        debug!(count = paginated.len(), total = total_count, "Listed datasets");

        Ok(Response::new(ListDatasetsResponse {
            datasets: paginated,
            total_count,
        }))
    }

    // =========================================================================
    // CREATE DATASET
    // =========================================================================

    #[instrument(skip(self, request), fields(name))]
    async fn create_dataset(
        &self,
        request: Request<CreateDatasetRequest>,
    ) -> Result<Response<CreateDatasetResponse>, Status> {
        let user_id = Self::get_user_id(&request)?;
        let req = request.into_inner();
        tracing::Span::current().record("name", &req.name);

        let metadata = self.metadata()?;

        // Parse file IDs
        let file_ids: Vec<Uuid> = req
            .file_ids
            .iter()
            .map(|s| {
                Uuid::parse_str(s)
                    .map_err(|e| Status::invalid_argument(format!("Invalid file_id '{}': {}", s, e)))
            })
            .collect::<Result<Vec<_>, _>>()?;

        if file_ids.is_empty() {
            return Err(Status::invalid_argument("At least one file_id is required"));
        }

        // Verify all files exist and calculate totals
        let mut total_size: i64 = 0;
        let mut file_infos: Vec<(Uuid, cyxcloud_metadata::File, i32)> = Vec::new();

        for (idx, file_id) in file_ids.iter().enumerate() {
            let file = metadata
                .database()
                .get_file(*file_id)
                .await
                .map_err(|e| Status::internal(format!("Database error: {}", e)))?
                .ok_or_else(|| Status::not_found(format!("File not found: {}", file_id)))?;

            total_size += file.size_bytes;
            file_infos.push((*file_id, file, idx as i32));
        }

        // Compute content hash (hash of all file hashes)
        let mut hasher = blake3::Hasher::new();
        for (_, file, _) in &file_infos {
            hasher.update(&file.content_hash);
        }
        let content_hash = hasher.finalize().as_bytes().to_vec();

        // Parse schema if provided
        let schema = if !req.schema_json.is_empty() {
            Some(
                serde_json::from_str(&req.schema_json)
                    .map_err(|e| Status::invalid_argument(format!("Invalid schema JSON: {}", e)))?,
            )
        } else {
            None
        };

        // Create dataset
        let create_dataset = CreateDataset {
            name: req.name.clone(),
            owner_id: user_id,
            description: if req.description.is_empty() {
                None
            } else {
                Some(req.description.clone())
            },
            content_hash,
            total_size_bytes: total_size,
            file_count: file_infos.len() as i32,
            schema,
            trust_level: TrustLevel::SelfUploaded, // User's own data
            signature: None,
            parent_version_id: None,
        };

        let dataset = metadata
            .database()
            .create_dataset(create_dataset)
            .await
            .map_err(|e| Status::internal(format!("Failed to create dataset: {}", e)))?;

        // Add files to dataset
        for (file_id, file, idx) in file_infos {
            let create_file = CreateDatasetFile {
                dataset_id: dataset.id,
                file_id,
                path_in_dataset: file.path.clone(),
                content_hash: file.content_hash.clone(),
                size_bytes: file.size_bytes,
                file_index: idx,
            };

            metadata
                .database()
                .create_dataset_file(create_file)
                .await
                .map_err(|e| Status::internal(format!("Failed to add file to dataset: {}", e)))?;
        }

        info!(
            dataset_id = %dataset.id,
            name = %req.name,
            files = file_ids.len(),
            "Dataset created"
        );

        Ok(Response::new(CreateDatasetResponse {
            dataset: Some(Self::dataset_to_proto(&dataset)),
        }))
    }

    // =========================================================================
    // CREATE ACCESS TOKEN
    // =========================================================================

    #[instrument(skip(self, request), fields(dataset_id))]
    async fn create_access_token(
        &self,
        request: Request<CreateAccessTokenRequest>,
    ) -> Result<Response<AccessTokenResponse>, Status> {
        let user_id = Self::get_user_id(&request)?;
        let req = request.into_inner();
        tracing::Span::current().record("dataset_id", &req.dataset_id);

        let metadata = self.metadata()?;

        let dataset_id = Uuid::parse_str(&req.dataset_id)
            .map_err(|e| Status::invalid_argument(format!("Invalid dataset_id: {}", e)))?;

        // Verify user has access to dataset
        let dataset = metadata
            .database()
            .get_dataset(dataset_id)
            .await
            .map_err(|e| Status::internal(format!("Database error: {}", e)))?
            .ok_or_else(|| Status::not_found("Dataset not found"))?;

        if dataset.owner_id != user_id {
            // Check if shared
            let share = metadata
                .database()
                .check_dataset_access(dataset_id, user_id)
                .await
                .map_err(|e| Status::internal(format!("Database error: {}", e)))?;

            if share.is_none() {
                return Err(Status::permission_denied("No access to dataset"));
            }
        }

        // Parse node ID if provided
        let node_id = if !req.node_id.is_empty() {
            Some(
                Uuid::parse_str(&req.node_id)
                    .map_err(|e| Status::invalid_argument(format!("Invalid node_id: {}", e)))?,
            )
        } else {
            None
        };

        // Generate token
        let token_id = Uuid::new_v4();
        let token_bytes: [u8; 32] = rand::random();
        let token_string = base64::Engine::encode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            token_bytes,
        );
        let token_hash = blake3::hash(token_string.as_bytes()).as_bytes().to_vec();

        // Calculate expiration
        let ttl_seconds = if req.ttl_seconds > 0 {
            req.ttl_seconds
        } else {
            24 * 60 * 60 // 24 hours default
        };
        let expires_at = chrono::Utc::now() + chrono::Duration::seconds(ttl_seconds);

        // Determine scopes
        let scopes = if req.scopes.is_empty() {
            vec!["read".to_string(), "stream".to_string()]
        } else {
            req.scopes
        };

        // Store token
        let create_token = CreateDataAccessToken {
            id: token_id,
            dataset_id,
            node_id,
            user_id,
            token_hash,
            scopes: scopes.clone(),
            expires_at,
        };

        metadata
            .database()
            .create_data_access_token(create_token)
            .await
            .map_err(|e| Status::internal(format!("Failed to create token: {}", e)))?;

        info!(
            token_id = %token_id,
            dataset_id = %dataset_id,
            expires_at = %expires_at,
            "Access token created"
        );

        Ok(Response::new(AccessTokenResponse {
            token: token_string,
            token_id: token_id.to_string(),
            expires_at: expires_at.timestamp(),
            scopes,
        }))
    }

    // =========================================================================
    // REVOKE ACCESS TOKEN
    // =========================================================================

    #[instrument(skip(self, request), fields(token_id))]
    async fn revoke_access_token(
        &self,
        request: Request<RevokeAccessTokenRequest>,
    ) -> Result<Response<RevokeAccessTokenResponse>, Status> {
        let _user_id = Self::get_user_id(&request)?;
        let req = request.into_inner();
        tracing::Span::current().record("token_id", &req.token_id);

        let metadata = self.metadata()?;

        let token_id = Uuid::parse_str(&req.token_id)
            .map_err(|e| Status::invalid_argument(format!("Invalid token_id: {}", e)))?;

        metadata
            .database()
            .revoke_data_access_token(token_id)
            .await
            .map_err(|e| Status::internal(format!("Failed to revoke token: {}", e)))?;

        info!(token_id = %token_id, "Access token revoked");

        Ok(Response::new(RevokeAccessTokenResponse { success: true }))
    }

    // =========================================================================
    // VERIFY DATASET
    // =========================================================================

    #[instrument(skip(self, request), fields(dataset_id))]
    async fn verify_dataset(
        &self,
        request: Request<VerifyDatasetRequest>,
    ) -> Result<Response<VerificationResult>, Status> {
        let req = request.into_inner();
        tracing::Span::current().record("dataset_id", &req.dataset_id);

        let metadata = self.metadata()?;

        let dataset_id = Uuid::parse_str(&req.dataset_id)
            .map_err(|e| Status::invalid_argument(format!("Invalid dataset_id: {}", e)))?;

        let dataset = metadata
            .database()
            .get_dataset(dataset_id)
            .await
            .map_err(|e| Status::internal(format!("Database error: {}", e)))?
            .ok_or_else(|| Status::not_found("Dataset not found"))?;

        let files = metadata
            .database()
            .get_dataset_files(dataset_id)
            .await
            .map_err(|e| Status::internal(format!("Database error: {}", e)))?;

        // Verify manifest hash
        let mut hasher = blake3::Hasher::new();
        for file in &files {
            hasher.update(file.path_in_dataset.as_bytes());
            hasher.update(&file.content_hash);
        }
        let computed_hash = hasher.finalize().as_bytes().to_vec();
        let manifest_valid = computed_hash == dataset.content_hash;

        // Verify files if full verification requested
        let mut file_verifications = Vec::new();
        let mut all_files_valid = true;

        if req.full_verification {
            for file in &files {
                // Get actual file and verify hash
                let file_record = metadata
                    .database()
                    .get_file(file.file_id)
                    .await
                    .map_err(|e| Status::internal(format!("Database error: {}", e)))?;

                let (valid, actual_hash) = if let Some(stored_file) = file_record {
                    (stored_file.content_hash == file.content_hash, Some(stored_file.content_hash))
                } else {
                    (false, None)
                };

                if !valid {
                    all_files_valid = false;
                }

                file_verifications.push(FileVerification {
                    path: file.path_in_dataset.clone(),
                    expected_hash: file.content_hash.clone(),
                    actual_hash: actual_hash.unwrap_or_default(),
                    valid,
                    error: if valid {
                        String::new()
                    } else {
                        "Hash mismatch".to_string()
                    },
                });
            }
        }

        // Check against public dataset registry
        let public_match = if req.check_public_registry {
            if let Ok(Some(public)) = metadata
                .database()
                .find_public_dataset_by_hash(&dataset.content_hash)
                .await
            {
                Some(PublicDatasetMatch {
                    name: public.name,
                    version: public.version,
                    official_url: public.official_url,
                    license: public.license.unwrap_or_default(),
                    verified_by: public.verified_by,
                    confidence: 1.0, // Exact hash match
                })
            } else {
                None
            }
        } else {
            None
        };

        // Determine computed trust level
        let computed_trust_level = if public_match.is_some() {
            ProtoTrustLevel::TrustVerified as i32
        } else if dataset.signature.is_some() {
            ProtoTrustLevel::TrustSigned as i32
        } else if dataset.owner_id != Uuid::nil() {
            ProtoTrustLevel::TrustSelf as i32
        } else {
            ProtoTrustLevel::TrustUntrusted as i32
        };

        let message = if manifest_valid && all_files_valid {
            if public_match.is_some() {
                "Dataset verified against public registry".to_string()
            } else {
                "Dataset integrity verified".to_string()
            }
        } else if !manifest_valid {
            "Manifest hash mismatch".to_string()
        } else {
            "Some files failed verification".to_string()
        };

        info!(
            dataset_id = %req.dataset_id,
            manifest_valid = manifest_valid,
            all_files_valid = all_files_valid,
            public_match = public_match.is_some(),
            "Dataset verification completed"
        );

        Ok(Response::new(VerificationResult {
            dataset_id: req.dataset_id,
            manifest_valid,
            all_files_valid,
            computed_trust_level,
            files: file_verifications,
            public_match,
            verified_at: chrono::Utc::now().timestamp(),
            message,
        }))
    }

    // =========================================================================
    // SHARE DATASET
    // =========================================================================

    #[instrument(skip(self, request), fields(dataset_id))]
    async fn share_dataset(
        &self,
        request: Request<ShareDatasetRequest>,
    ) -> Result<Response<ShareDatasetResponse>, Status> {
        let user_id = Self::get_user_id(&request)?;
        let req = request.into_inner();
        tracing::Span::current().record("dataset_id", &req.dataset_id);

        let metadata = self.metadata()?;

        let dataset_id = Uuid::parse_str(&req.dataset_id)
            .map_err(|e| Status::invalid_argument(format!("Invalid dataset_id: {}", e)))?;

        let share_with_user_id = Uuid::parse_str(&req.share_with_user_id)
            .map_err(|e| Status::invalid_argument(format!("Invalid share_with_user_id: {}", e)))?;

        // Verify user owns the dataset
        let dataset = metadata
            .database()
            .get_dataset(dataset_id)
            .await
            .map_err(|e| Status::internal(format!("Database error: {}", e)))?
            .ok_or_else(|| Status::not_found("Dataset not found"))?;

        if dataset.owner_id != user_id {
            return Err(Status::permission_denied("Only the owner can share a dataset"));
        }

        // Determine permissions
        let permissions = if req.permissions.is_empty() {
            vec!["read".to_string()]
        } else {
            req.permissions
        };

        // Calculate expiration if provided
        let expires_at = if req.expires_at > 0 {
            Some(chrono::DateTime::from_timestamp(req.expires_at, 0)
                .ok_or_else(|| Status::invalid_argument("Invalid expires_at timestamp"))?
                .with_timezone(&chrono::Utc))
        } else {
            None
        };

        // Create share
        let create_share = CreateDatasetShare {
            dataset_id,
            shared_with_user_id: share_with_user_id,
            shared_by_user_id: user_id,
            permissions,
            expires_at,
        };

        let share = metadata
            .database()
            .create_dataset_share(create_share)
            .await
            .map_err(|e| Status::internal(format!("Failed to share dataset: {}", e)))?;

        info!(
            share_id = %share.id,
            dataset_id = %dataset_id,
            shared_with = %share_with_user_id,
            "Dataset shared"
        );

        Ok(Response::new(ShareDatasetResponse {
            share_id: share.id.to_string(),
            success: true,
        }))
    }

    // =========================================================================
    // LIST PUBLIC DATASETS
    // =========================================================================

    #[instrument(skip(self, request))]
    async fn list_public_datasets(
        &self,
        request: Request<ListPublicDatasetsRequest>,
    ) -> Result<Response<ListPublicDatasetsResponse>, Status> {
        let req = request.into_inner();
        let metadata = self.metadata()?;

        let datasets = metadata
            .database()
            .get_all_public_datasets()
            .await
            .map_err(|e| Status::internal(format!("Database error: {}", e)))?;

        // Filter by name if specified
        let filtered: Vec<_> = if !req.name_filter.is_empty() {
            let filter_lower = req.name_filter.to_lowercase();
            datasets
                .into_iter()
                .filter(|d| d.name.to_lowercase().contains(&filter_lower))
                .map(|d| Self::public_dataset_to_proto(&d))
                .collect()
        } else {
            datasets
                .iter()
                .map(|d| Self::public_dataset_to_proto(d))
                .collect()
        };

        debug!(count = filtered.len(), "Listed public datasets");

        Ok(Response::new(ListPublicDatasetsResponse {
            datasets: filtered,
        }))
    }
}
