//! gRPC API for CyxCloud Gateway
//!
//! Provides:
//! - NodeService: Node registration and heartbeat handling
//! - DataService: Streaming data access for ML training pipelines

use crate::node_client::NodeClient;
use crate::AppState;
use cyxcloud_metadata::{CreateNode, MetadataService, Node};
use cyxcloud_protocol::data::{
    data_service_server::DataService, DataChunk, DatasetInfo as ProtoDatasetInfo,
    GetDatasetRequest, PrefetchRequest, PrefetchResponse, StreamDataRequest,
};
use cyxcloud_protocol::node::{
    node_service_server::NodeService, DrainNodeRequest, DrainNodeResponse, GetNodeRequest,
    GetNodeResponse, HeartbeatRequest, HeartbeatResponse, ListNodesRequest, ListNodesResponse,
    NodeCapacity, NodeInfo, NodeLocation, NodeStatus, RegisterNodeRequest, RegisterNodeResponse,
    ReportMetricsRequest, ReportMetricsResponse,
};
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::Stream;
use tonic::{Request, Response, Status};
use tracing::{debug, error, info, instrument, warn};
use uuid::Uuid;

// =============================================================================
// NODE SERVICE IMPLEMENTATION
// =============================================================================

/// gRPC Node Service implementation
pub struct NodeServiceImpl {
    state: Arc<AppState>,
}

impl NodeServiceImpl {
    /// Create a new NodeService with application state
    pub fn new(state: Arc<AppState>) -> Self {
        Self { state }
    }

    /// Get metadata service from state
    fn metadata(&self) -> Option<&MetadataService> {
        self.state.metadata_service()
    }

    /// Convert internal Node model to proto NodeInfo
    fn node_to_proto(node: &Node) -> NodeInfo {
        let status = match node.status.as_str() {
            "online" => NodeStatus::Online,
            "offline" => NodeStatus::Offline,
            "draining" => NodeStatus::Draining,
            "maintenance" => NodeStatus::Maintenance,
            _ => NodeStatus::Unknown,
        };

        NodeInfo {
            node_id: node.id.to_string(),
            public_key: node.public_key.clone().unwrap_or_default(),
            wallet_address: node.wallet_address.clone().unwrap_or_default(),
            listen_addrs: vec![node.grpc_address.clone()],
            location: Some(NodeLocation {
                datacenter: node.datacenter.clone().unwrap_or_default(),
                rack: node.rack.unwrap_or(0) as u32,
                region: node.region.clone().unwrap_or_default(),
                latitude: node.latitude.unwrap_or(0.0),
                longitude: node.longitude.unwrap_or(0.0),
            }),
            capacity: Some(NodeCapacity {
                storage_total: node.storage_total as u64,
                storage_used: node.storage_used as u64,
                bandwidth_mbps: node.bandwidth_mbps as u64,
                max_connections: node.max_connections as u32,
            }),
            status: status.into(),
            registered_at: node.created_at.timestamp(),
        }
    }
}

#[tonic::async_trait]
impl NodeService for NodeServiceImpl {
    #[instrument(skip(self, request), fields(node_id))]
    async fn register_node(
        &self,
        request: Request<RegisterNodeRequest>,
    ) -> Result<Response<RegisterNodeResponse>, Status> {
        let req = request.into_inner();
        let node_id = req.node_id.clone();
        tracing::Span::current().record("node_id", &node_id);

        info!(node_id = %node_id, "Processing node registration");

        let metadata = self.metadata().ok_or_else(|| {
            error!("Metadata service not available");
            Status::unavailable("Metadata service not configured")
        })?;

        // Extract node info from request
        let info = req
            .info
            .ok_or_else(|| Status::invalid_argument("NodeInfo is required"))?;

        // Get location and capacity
        let location = info.location.unwrap_or_default();
        let capacity = info.capacity.unwrap_or_default();

        // Create the node in metadata service
        // Gateway reserves 2GB for metadata, erasure coding, and rebalancing
        const GATEWAY_RESERVED_BYTES: i64 = 2 * 1024 * 1024 * 1024; // 2 GB
        let create_node = CreateNode {
            peer_id: node_id.clone(),
            grpc_address: info.listen_addrs.first().cloned().unwrap_or_default(),
            storage_total: capacity.storage_total as i64,
            storage_reserved: GATEWAY_RESERVED_BYTES,
            bandwidth_mbps: capacity.bandwidth_mbps as i32,
            datacenter: if location.datacenter.is_empty() {
                None
            } else {
                Some(location.datacenter)
            },
            region: if location.region.is_empty() {
                None
            } else {
                Some(location.region)
            },
            version: None,
            wallet_address: if info.wallet_address.is_empty() {
                None
            } else {
                Some(info.wallet_address.clone())
            },
            public_key: if info.public_key.is_empty() {
                None
            } else {
                Some(info.public_key.clone())
            },
        };

        // Log wallet if provided
        if !info.wallet_address.is_empty() {
            info!(
                node_id = %node_id,
                wallet = %info.wallet_address,
                "Node registering with wallet address"
            );
        }

        match metadata.register_node(create_node).await {
            Ok(node) => {
                info!(
                    node_id = %node.id,
                    peer_id = %node.peer_id,
                    grpc_addr = %node.grpc_address,
                    "Node registered successfully"
                );

                // Generate a proper JWT token for the node
                let auth_token = self
                    .state
                    .auth_service()
                    .generate_token(
                        &node.id.to_string(),
                        crate::auth::TokenType::Node,
                        node.wallet_address.clone(),
                        vec![
                            crate::auth::permissions::NODE_REGISTER.to_string(),
                            crate::auth::permissions::STORAGE_READ.to_string(),
                            crate::auth::permissions::STORAGE_WRITE.to_string(),
                        ],
                    )
                    .map_err(|e| {
                        error!(error = %e, "Failed to generate node auth token");
                        Status::internal("Failed to generate auth token")
                    })?;

                Ok(Response::new(RegisterNodeResponse {
                    success: true,
                    error: String::new(),
                    auth_token,
                }))
            }
            Err(e) => {
                error!(error = %e, node_id = %node_id, "Failed to register node");
                Ok(Response::new(RegisterNodeResponse {
                    success: false,
                    error: format!("Registration failed: {}", e),
                    auth_token: String::new(),
                }))
            }
        }
    }

    #[instrument(skip(self, request), fields(node_id))]
    async fn heartbeat(
        &self,
        request: Request<HeartbeatRequest>,
    ) -> Result<Response<HeartbeatResponse>, Status> {
        let req = request.into_inner();
        let node_id_str = req.node_id.clone();
        tracing::Span::current().record("node_id", &node_id_str);

        debug!(node_id = %node_id_str, "Processing heartbeat");

        let metadata = self
            .metadata()
            .ok_or_else(|| Status::unavailable("Metadata service not configured"))?;

        // Use peer_id directly - the node sends its own ID which is stored as peer_id
        // Update heartbeat with recovery-aware logic
        match metadata.heartbeat_by_peer_id(&node_id_str).await {
            Ok(status) => {
                let status_str = status.to_string();
                if status == cyxcloud_metadata::NodeStatus::Recovering {
                    debug!(
                        node_id = %node_id_str,
                        status = %status_str,
                        "Heartbeat recorded - node in recovery quarantine"
                    );
                } else {
                    debug!(node_id = %node_id_str, status = %status_str, "Heartbeat recorded");
                }
                Ok(Response::new(HeartbeatResponse {
                    acknowledged: true,
                    commands: vec![], // TODO: Return pending commands
                }))
            }
            Err(e) => {
                warn!(error = %e, node_id = %node_id_str, "Failed to record heartbeat");
                Ok(Response::new(HeartbeatResponse {
                    acknowledged: false,
                    commands: vec![],
                }))
            }
        }
    }

    #[instrument(skip(self, request), fields(node_id))]
    async fn get_node(
        &self,
        request: Request<GetNodeRequest>,
    ) -> Result<Response<GetNodeResponse>, Status> {
        let req = request.into_inner();
        tracing::Span::current().record("node_id", &req.node_id);

        let metadata = self
            .metadata()
            .ok_or_else(|| Status::unavailable("Metadata service not configured"))?;

        // Get node from metadata by peer_id
        match metadata.database().get_node_by_peer_id(&req.node_id).await {
            Ok(Some(node)) => {
                let info = Self::node_to_proto(&node);
                Ok(Response::new(GetNodeResponse {
                    info: Some(info),
                    metrics: None, // TODO: Add metrics tracking
                    found: true,
                }))
            }
            Ok(None) => Ok(Response::new(GetNodeResponse {
                info: None,
                metrics: None,
                found: false,
            })),
            Err(e) => {
                error!(error = %e, "Failed to get node");
                Err(Status::internal(format!("Database error: {}", e)))
            }
        }
    }

    #[instrument(skip(self, request))]
    async fn list_nodes(
        &self,
        request: Request<ListNodesRequest>,
    ) -> Result<Response<ListNodesResponse>, Status> {
        let req = request.into_inner();

        let metadata = self
            .metadata()
            .ok_or_else(|| Status::unavailable("Metadata service not configured"))?;

        // Get all online nodes (or filter by status if specified)
        let nodes = match metadata.get_online_nodes().await {
            Ok(nodes) => nodes,
            Err(e) => {
                error!(error = %e, "Failed to list nodes");
                return Err(Status::internal(format!("Database error: {}", e)));
            }
        };

        // Apply filters
        let filtered: Vec<NodeInfo> = nodes
            .iter()
            .filter(|n| {
                // Filter by status if specified
                if req.status_filter != 0 {
                    let status = match n.status.as_str() {
                        "online" => NodeStatus::Online as i32,
                        "offline" => NodeStatus::Offline as i32,
                        "draining" => NodeStatus::Draining as i32,
                        "maintenance" => NodeStatus::Maintenance as i32,
                        _ => NodeStatus::Unknown as i32,
                    };
                    if status != req.status_filter {
                        return false;
                    }
                }
                // Filter by region if specified
                if !req.region_filter.is_empty() {
                    if let Some(region) = &n.region {
                        if region != &req.region_filter {
                            return false;
                        }
                    } else {
                        return false;
                    }
                }
                true
            })
            .map(Self::node_to_proto)
            .collect();

        debug!(count = filtered.len(), "Listed nodes");

        Ok(Response::new(ListNodesResponse { nodes: filtered }))
    }

    #[instrument(skip(self, request), fields(node_id))]
    async fn report_metrics(
        &self,
        request: Request<ReportMetricsRequest>,
    ) -> Result<Response<ReportMetricsResponse>, Status> {
        let req = request.into_inner();
        tracing::Span::current().record("node_id", &req.node_id);

        debug!(
            node_id = %req.node_id,
            metrics = ?req.metrics,
            "Received metrics report"
        );

        // TODO: Store metrics in time-series database or update node record
        // For now, just acknowledge receipt

        Ok(Response::new(ReportMetricsResponse { success: true }))
    }

    #[instrument(skip(self, request), fields(node_id))]
    async fn drain_node(
        &self,
        request: Request<DrainNodeRequest>,
    ) -> Result<Response<DrainNodeResponse>, Status> {
        let req = request.into_inner();
        tracing::Span::current().record("node_id", &req.node_id);

        info!(
            node_id = %req.node_id,
            reason = %req.reason,
            "Node drain requested"
        );

        let metadata = self
            .metadata()
            .ok_or_else(|| Status::unavailable("Metadata service not configured"))?;

        // Parse node UUID
        let node_uuid = Uuid::parse_str(&req.node_id)
            .map_err(|e| Status::invalid_argument(format!("Invalid node_id format: {}", e)))?;

        // Update node status to draining
        match metadata
            .database()
            .update_node_status(node_uuid, "draining")
            .await
        {
            Ok(()) => {
                info!(node_id = %req.node_id, "Node marked as draining");
                Ok(Response::new(DrainNodeResponse {
                    accepted: true,
                    estimated_duration_secs: 300, // 5 minutes default estimate
                }))
            }
            Err(e) => {
                error!(error = %e, "Failed to mark node as draining");
                Ok(Response::new(DrainNodeResponse {
                    accepted: false,
                    estimated_duration_secs: 0,
                }))
            }
        }
    }
}

// =============================================================================
// DATA SERVICE IMPLEMENTATION
// =============================================================================


/// A dataset access token may stream a file when it carries the "stream" or
/// "read" scope (CreateAccessToken grants both by default).
pub fn token_allows_stream(scopes: &[String]) -> bool {
    scopes.iter().any(|s| s == "stream" || s == "read")
}

/// The status a stream ends with when chunk `index` of `count` cannot be
/// served: data loss, never a silent skip.
pub fn chunk_failure(index: usize, count: usize, why: &str, detail: &str) -> Status {
    let mut message = format!("chunk {} of {} could not be served: {}", index + 1, count, why);
    if !detail.is_empty() {
        message.push_str(" (");
        message.push_str(detail);
        message.push(')');
    }
    Status::data_loss(message)
}

/// gRPC Data Service implementation for ML training data streaming
pub struct DataServiceImpl {
    state: Arc<AppState>,
}

impl DataServiceImpl {
    /// Create a new DataService with application state
    pub fn new(state: Arc<AppState>) -> Self {
        Self { state }
    }

    /// Get metadata service from state
    fn metadata(&self) -> Option<&MetadataService> {
        self.state.metadata_service()
    }

    /// Get node client from state
    fn node_client(&self) -> &NodeClient {
        self.state.node_client()
    }
}

#[tonic::async_trait]
impl DataService for DataServiceImpl {
    type StreamDataStream = Pin<Box<dyn Stream<Item = Result<DataChunk, Status>> + Send + 'static>>;

    #[instrument(skip(self, request), fields(dataset_id))]
    async fn stream_data(
        &self,
        request: Request<StreamDataRequest>,
    ) -> Result<Response<Self::StreamDataStream>, Status> {
        let req = request.into_inner();
        let dataset_id = req.dataset_id.clone();
        tracing::Span::current().record("dataset_id", &dataset_id);

        let metadata = self
            .metadata()
            .ok_or_else(|| Status::unavailable("Metadata service not configured"))?;

        // The file to stream (StreamData names a file by its UUID).
        let file_uuid = Uuid::parse_str(&dataset_id)
            .map_err(|e| Status::invalid_argument(format!("Invalid dataset_id: {}", e)))?;

        // Whole-file streams are ordered; shuffling only makes sense for
        // sample batches, which this service does not produce.
        if req.shuffle {
            return Err(Status::invalid_argument(
                "StreamData sends a file in order; shuffle is not supported",
            ));
        }

        // Every stream presents a dataset access token for a dataset that
        // contains the file (TOFIX164): no token, no bytes.
        if req.access_token.is_empty() {
            return Err(Status::unauthenticated("Dataset access token required"));
        }
        let token_hash = blake3::hash(req.access_token.as_bytes()).as_bytes().to_vec();
        let token = metadata
            .database()
            .validate_data_access_token(&token_hash)
            .await
            .map_err(|e| Status::internal(format!("Database error: {}", e)))?
            .ok_or_else(|| Status::unauthenticated("Dataset access token is invalid, expired or revoked"))?;
        if !token_allows_stream(&token.scopes) {
            return Err(Status::permission_denied("Token does not allow streaming"));
        }
        let dataset_files = metadata
            .database()
            .get_dataset_files(token.dataset_id)
            .await
            .map_err(|e| Status::internal(format!("Database error: {}", e)))?;
        if !dataset_files.iter().any(|f| f.file_id == file_uuid) {
            return Err(Status::permission_denied("Token does not cover this file"));
        }

        let file = metadata
            .get_file(file_uuid)
            .await
            .map_err(|e| Status::internal(format!("Database error: {}", e)))?
            .ok_or_else(|| Status::not_found(format!("Dataset not found: {}", dataset_id)))?;
        let expected_hash = file.content_hash.clone();

        let chunks = metadata.get_file_chunks(file_uuid).await.map_err(|e| {
            error!(error = %e, "Failed to get file chunks");
            Status::internal(format!("Failed to get dataset chunks: {}", e))
        })?;
        if chunks.is_empty() {
            return Err(Status::not_found("Dataset has no chunks"));
        }

        info!(
            dataset_id = %dataset_id,
            chunk_count = chunks.len(),
            token_id = %token.id,
            "Found dataset chunks, starting stream"
        );

        let (tx, rx) = mpsc::channel(req.prefetch.max(1) as usize);
        let metadata_clone = self.state.metadata_service_arc();
        let node_client = self.state.node_client_arc();
        let batch_bytes = (req.batch_size.max(1) as usize) * 1024;

        tokio::spawn(async move {
            use sha2::Digest;
            let mut file_hasher = blake3::Hasher::new();
            let mut stream_hasher = sha2::Sha256::new();
            let mut total_bytes: u64 = 0;
            let mut batch_index: u64 = 0;
            let mut batch_data: Vec<u8> = Vec::with_capacity(batch_bytes);
            let chunk_count = chunks.len();

            for (i, chunk) in chunks.iter().enumerate() {
                let locations = match metadata_clone.as_ref() {
                    Some(meta) => match meta.get_chunk_locations(&chunk.chunk_id).await {
                        Ok(locations) => locations,
                        Err(e) => {
                            let _ = tx.send(Err(chunk_failure(i, chunk_count, "its locations could not be read", &e.to_string()))).await;
                            return;
                        }
                    },
                    None => Vec::new(),
                };
                if locations.is_empty() {
                    let _ = tx.send(Err(chunk_failure(i, chunk_count, "no storage node holds it", ""))).await;
                    return;
                }
                let data = match node_client.get_chunk_from_any(&locations, &chunk.chunk_id).await {
                    Ok(data) => data,
                    Err(e) => {
                        let _ = tx.send(Err(chunk_failure(i, chunk_count, "no storage node could serve it", &e.to_string()))).await;
                        return;
                    }
                };
                file_hasher.update(&data);
                stream_hasher.update(&data);
                total_bytes += data.len() as u64;
                batch_data.extend_from_slice(&data);

                let is_last = i + 1 == chunk_count;
                if batch_data.len() >= batch_bytes && !is_last {
                    let message = DataChunk {
                        batch_index,
                        data: std::mem::take(&mut batch_data),
                        is_last: false,
                        stream_sha256: String::new(),
                        total_bytes: 0,
                    };
                    if tx.send(Ok(message)).await.is_err() {
                        debug!("Client disconnected, stopping stream");
                        return;
                    }
                    batch_index += 1;
                }
            }

            // Everything was read: the assembled bytes must be the stored file.
            let assembled = file_hasher.finalize();
            if assembled.as_bytes() != expected_hash.as_slice() {
                error!(dataset_id = %dataset_id, "Assembled bytes do not match the stored content hash");
                let _ = tx
                    .send(Err(Status::data_loss(
                        "the assembled file does not match its stored content hash",
                    )))
                    .await;
                return;
            }
            let trailer = DataChunk {
                batch_index,
                data: batch_data,
                is_last: true,
                stream_sha256: hex::encode(stream_hasher.finalize()),
                total_bytes,
            };
            let _ = tx.send(Ok(trailer)).await;
            debug!(dataset_id = %dataset_id, total_bytes, "Data stream completed");
        });

        let stream = ReceiverStream::new(rx);
        Ok(Response::new(Box::pin(stream) as Self::StreamDataStream))
    }

    #[instrument(skip(self, request), fields(dataset_id))]
    async fn get_dataset(
        &self,
        request: Request<GetDatasetRequest>,
    ) -> Result<Response<ProtoDatasetInfo>, Status> {
        let req = request.into_inner();
        tracing::Span::current().record("dataset_id", &req.dataset_id);

        let metadata = self
            .metadata()
            .ok_or_else(|| Status::unavailable("Metadata service not configured"))?;

        // Parse dataset ID as UUID (file ID)
        let file_uuid = Uuid::parse_str(&req.dataset_id)
            .map_err(|e| Status::invalid_argument(format!("Invalid dataset_id: {}", e)))?;

        // Get file metadata
        let file = metadata.get_file(file_uuid).await.map_err(|e| {
            error!(error = %e, "Failed to get file metadata");
            Status::internal(format!("Database error: {}", e))
        })?;

        let file = file
            .ok_or_else(|| Status::not_found(format!("Dataset not found: {}", req.dataset_id)))?;

        // Get chunk count
        let chunks = metadata.get_file_chunks(file_uuid).await.map_err(|e| {
            error!(error = %e, "Failed to get file chunks");
            Status::internal("Failed to get chunk information")
        })?;

        debug!(
            dataset_id = %req.dataset_id,
            size = file.size_bytes,
            chunks = chunks.len(),
            "Retrieved dataset info"
        );

        // Build response - extract ML-specific metadata from file.metadata if available
        let metadata_str = file.metadata.as_ref().map(|v| v.to_string());
        let (sample_shape, dtype, num_samples) = parse_dataset_metadata(&metadata_str);

        Ok(Response::new(ProtoDatasetInfo {
            id: file.id.to_string(),
            name: file.path.clone(),
            size_bytes: file.size_bytes as u64,
            num_samples,
            num_chunks: chunks.len() as u32,
            sample_shape,
            dtype,
            metadata_json: metadata_str.unwrap_or_default(),
        }))
    }

    #[instrument(skip(self, request), fields(dataset_id))]
    async fn prefetch(
        &self,
        request: Request<PrefetchRequest>,
    ) -> Result<Response<PrefetchResponse>, Status> {
        let req = request.into_inner();
        tracing::Span::current().record("dataset_id", &req.dataset_id);

        info!(
            dataset_id = %req.dataset_id,
            chunk_count = req.chunk_ids.len(),
            "Processing prefetch request"
        );

        let metadata = self
            .metadata()
            .ok_or_else(|| Status::unavailable("Metadata service not configured"))?;

        let mut cached_chunks = 0u64;
        let mut failed_chunks = 0u64;

        // If specific chunk IDs provided, prefetch those
        // Otherwise, prefetch all chunks for the dataset
        let chunk_ids: Vec<Vec<u8>> = if req.chunk_ids.is_empty() {
            // Get all chunks for dataset
            let file_uuid = Uuid::parse_str(&req.dataset_id)
                .map_err(|e| Status::invalid_argument(format!("Invalid dataset_id: {}", e)))?;

            metadata
                .get_file_chunks(file_uuid)
                .await
                .map_err(|e| Status::internal(format!("Failed to get chunks: {}", e)))?
                .into_iter()
                .map(|c| c.chunk_id)
                .collect()
        } else {
            req.chunk_ids
                .into_iter()
                .filter_map(|s| hex::decode(&s).ok())
                .collect()
        };

        // Prefetch each chunk (retrieve from nodes to verify availability)
        for chunk_id in &chunk_ids {
            let locations = metadata
                .get_chunk_locations(chunk_id)
                .await
                .unwrap_or_default();

            if locations.is_empty() {
                failed_chunks += 1;
                continue;
            }

            // Try to retrieve the chunk (this validates it's accessible)
            match self
                .node_client()
                .get_chunk_from_any(&locations, chunk_id)
                .await
            {
                Ok(_) => {
                    cached_chunks += 1;
                    // In a real implementation, we would cache this data locally
                    // For now, we just verify accessibility
                }
                Err(e) => {
                    warn!(
                        chunk_id = %hex::encode(chunk_id),
                        error = %e,
                        "Failed to prefetch chunk"
                    );
                    failed_chunks += 1;
                }
            }
        }

        info!(
            dataset_id = %req.dataset_id,
            cached = cached_chunks,
            failed = failed_chunks,
            "Prefetch completed"
        );

        Ok(Response::new(PrefetchResponse {
            cached_chunks,
            failed_chunks,
        }))
    }
}

/// Parse ML-specific metadata from JSON
fn parse_dataset_metadata(metadata_json: &Option<String>) -> (Vec<i32>, String, u64) {
    let default_shape = vec![];
    let default_dtype = "float32".to_string();
    let default_samples = 0u64;

    let Some(json_str) = metadata_json else {
        return (default_shape, default_dtype, default_samples);
    };

    let Ok(json) = serde_json::from_str::<serde_json::Value>(json_str) else {
        return (default_shape, default_dtype, default_samples);
    };

    let sample_shape = json
        .get("sample_shape")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_i64().map(|n| n as i32))
                .collect()
        })
        .unwrap_or(default_shape);

    let dtype = json
        .get("dtype")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or(default_dtype);

    let num_samples = json
        .get("num_samples")
        .and_then(|v| v.as_u64())
        .unwrap_or(default_samples);

    (sample_shape, dtype, num_samples)
}

// =============================================================================
// CLAIMS ON A REQUEST (set by grpc_auth::AuthLayer)
// =============================================================================

use crate::auth::Claims;

/// Extension trait for extracting claims from request
pub trait RequestClaimsExt {
    /// Get authenticated user claims from request
    fn claims(&self) -> Option<&Claims>;

    /// Require authentication and return claims
    fn require_auth(&self) -> Result<&Claims, Status>;
}

impl<T> RequestClaimsExt for tonic::Request<T> {
    fn claims(&self) -> Option<&Claims> {
        self.extensions().get::<Claims>()
    }

    fn require_auth(&self) -> Result<&Claims, Status> {
        self.claims()
            .ok_or_else(|| Status::unauthenticated("Authentication required"))
    }
}

// =============================================================================
// DATA SERVICE TYPES (for AppState compatibility)
// =============================================================================

/// Data service module with placeholder types
pub mod data_service {
    /// Dataset information
    #[derive(Debug, Clone)]
    pub struct DatasetInfo {
        pub id: String,
        pub name: String,
        pub size_bytes: u64,
        pub num_samples: u64,
        pub num_chunks: u32,
        pub sample_shape: Vec<i32>,
        pub dtype: String,
        pub metadata_json: String,
    }
}

// =============================================================================
// TESTS
// =============================================================================

#[cfg(test)]
mod tests {
    use super::{chunk_failure, token_allows_stream};

    #[test]
    fn stream_scope_is_read_or_stream() {
        assert!(token_allows_stream(&["read".to_string(), "stream".to_string()]));
        assert!(token_allows_stream(&["stream".to_string()]));
        assert!(token_allows_stream(&["read".to_string()]));
        assert!(!token_allows_stream(&["write".to_string()]));
        assert!(!token_allows_stream(&[]));
    }

    #[test]
    fn a_missing_chunk_is_data_loss_with_its_position() {
        let status = chunk_failure(2, 7, "no storage node holds it", "");
        assert_eq!(status.code(), tonic::Code::DataLoss);
        assert_eq!(status.message(), "chunk 3 of 7 could not be served: no storage node holds it");
        let detailed = chunk_failure(0, 1, "no storage node could serve it", "connection refused");
        assert!(detailed.message().ends_with("(connection refused)"));
    }

    use crate::auth::Claims;

    #[test]
    fn test_jwt_claims_creation() {
        let claims = Claims {
            sub: "user123".to_string(),
            exp: 1700000000,
            iat: 1699900000,
            nbf: 1699900000,
            jti: "test-jwt-id".to_string(),
            user_type: "user".to_string(),
            wallet: None,
            permissions: vec!["read".to_string(), "write".to_string()],
        };

        assert_eq!(claims.sub, "user123");
        assert!(claims.permissions.contains(&"read".to_string()));
    }
}
