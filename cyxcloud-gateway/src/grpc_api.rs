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
            public_key: String::new(), // TODO: Add public key support
            wallet_address: String::new(), // TODO: Add wallet support
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
        let info = req.info.ok_or_else(|| {
            Status::invalid_argument("NodeInfo is required")
        })?;

        // Get location and capacity
        let location = info.location.unwrap_or_default();
        let capacity = info.capacity.unwrap_or_default();

        // Create the node in metadata service
        let create_node = CreateNode {
            peer_id: node_id.clone(),
            grpc_address: info.listen_addrs.first().cloned().unwrap_or_default(),
            storage_total: capacity.storage_total as i64,
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
        };

        match metadata.register_node(create_node).await {
            Ok(node) => {
                info!(
                    node_id = %node.id,
                    peer_id = %node.peer_id,
                    grpc_addr = %node.grpc_address,
                    "Node registered successfully"
                );

                // Generate a simple auth token (in production, use proper JWT)
                let auth_token = format!("node-token-{}", node.id);

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

        let metadata = self.metadata().ok_or_else(|| {
            Status::unavailable("Metadata service not configured")
        })?;

        // Parse node UUID
        let node_uuid = Uuid::parse_str(&node_id_str).map_err(|e| {
            Status::invalid_argument(format!("Invalid node_id format: {}", e))
        })?;

        // Update heartbeat with recovery-aware logic
        match metadata.heartbeat_with_recovery(node_uuid).await {
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

        let metadata = self.metadata().ok_or_else(|| {
            Status::unavailable("Metadata service not configured")
        })?;

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

        let metadata = self.metadata().ok_or_else(|| {
            Status::unavailable("Metadata service not configured")
        })?;

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

        let metadata = self.metadata().ok_or_else(|| {
            Status::unavailable("Metadata service not configured")
        })?;

        // Parse node UUID
        let node_uuid = Uuid::parse_str(&req.node_id).map_err(|e| {
            Status::invalid_argument(format!("Invalid node_id format: {}", e))
        })?;

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
    type StreamDataStream =
        Pin<Box<dyn Stream<Item = Result<DataChunk, Status>> + Send + 'static>>;

    #[instrument(skip(self, request), fields(dataset_id))]
    async fn stream_data(
        &self,
        request: Request<StreamDataRequest>,
    ) -> Result<Response<Self::StreamDataStream>, Status> {
        let req = request.into_inner();
        let dataset_id = req.dataset_id.clone();
        tracing::Span::current().record("dataset_id", &dataset_id);

        info!(
            dataset_id = %dataset_id,
            batch_size = req.batch_size,
            shuffle = req.shuffle,
            prefetch = req.prefetch,
            "Starting data stream"
        );

        let metadata = self.metadata().ok_or_else(|| {
            Status::unavailable("Metadata service not configured")
        })?;

        // Parse dataset ID as UUID (file ID)
        let file_uuid = Uuid::parse_str(&dataset_id).map_err(|e| {
            Status::invalid_argument(format!("Invalid dataset_id: {}", e))
        })?;

        // Get file chunks from metadata
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
            "Found dataset chunks, starting stream"
        );

        // Create channel for streaming
        let (tx, rx) = mpsc::channel(req.prefetch.max(1) as usize);

        // Get references we need for the streaming task
        let metadata_clone = self.state.metadata_service_arc();
        let node_client = self.state.node_client_arc();
        let shuffle = req.shuffle;
        let batch_size = req.batch_size as usize;

        // Spawn task to stream chunks
        tokio::spawn(async move {
            // Optionally shuffle chunk indices
            let chunk_indices: Vec<usize> = if shuffle {
                use rand::seq::SliceRandom;
                let mut indices: Vec<usize> = (0..chunks.len()).collect();
                let mut rng = rand::thread_rng();
                indices.shuffle(&mut rng);
                indices
            } else {
                (0..chunks.len()).collect()
            };

            let mut batch_index: u64 = 0;
            let mut batch_data = Vec::with_capacity(batch_size);

            for (i, &chunk_idx) in chunk_indices.iter().enumerate() {
                let chunk = &chunks[chunk_idx];
                let is_last = i == chunk_indices.len() - 1;

                // Get chunk locations from metadata
                let locations = if let Some(ref meta) = metadata_clone {
                    meta.get_chunk_locations(&chunk.chunk_id)
                        .await
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };

                if locations.is_empty() {
                    warn!(
                        chunk_id = %hex::encode(&chunk.chunk_id),
                        "No locations found for chunk, skipping"
                    );
                    continue;
                }

                // Retrieve chunk data from storage node
                match node_client.get_chunk_from_any(&locations, &chunk.chunk_id).await {
                    Ok(data) => {
                        batch_data.extend_from_slice(&data);

                        // Send batch when we have enough or at the end
                        let should_send = batch_data.len() >= batch_size * 1024 || is_last;

                        if should_send {
                            let chunk_response = DataChunk {
                                batch_index,
                                data: std::mem::take(&mut batch_data),
                                is_last,
                            };

                            if tx.send(Ok(chunk_response)).await.is_err() {
                                debug!("Client disconnected, stopping stream");
                                break;
                            }

                            batch_index += 1;
                        }
                    }
                    Err(e) => {
                        warn!(
                            chunk_id = %hex::encode(&chunk.chunk_id),
                            error = %e,
                            "Failed to retrieve chunk"
                        );
                        // Continue with other chunks
                    }
                }
            }

            // Send any remaining data
            if !batch_data.is_empty() {
                let _ = tx
                    .send(Ok(DataChunk {
                        batch_index,
                        data: batch_data,
                        is_last: true,
                    }))
                    .await;
            }

            debug!(dataset_id = %dataset_id, "Data stream completed");
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

        let metadata = self.metadata().ok_or_else(|| {
            Status::unavailable("Metadata service not configured")
        })?;

        // Parse dataset ID as UUID (file ID)
        let file_uuid = Uuid::parse_str(&req.dataset_id).map_err(|e| {
            Status::invalid_argument(format!("Invalid dataset_id: {}", e))
        })?;

        // Get file metadata
        let file = metadata.get_file(file_uuid).await.map_err(|e| {
            error!(error = %e, "Failed to get file metadata");
            Status::internal(format!("Database error: {}", e))
        })?;

        let file = file.ok_or_else(|| {
            Status::not_found(format!("Dataset not found: {}", req.dataset_id))
        })?;

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

        let metadata = self.metadata().ok_or_else(|| {
            Status::unavailable("Metadata service not configured")
        })?;

        let mut cached_chunks = 0u64;
        let mut failed_chunks = 0u64;

        // If specific chunk IDs provided, prefetch those
        // Otherwise, prefetch all chunks for the dataset
        let chunk_ids: Vec<Vec<u8>> = if req.chunk_ids.is_empty() {
            // Get all chunks for dataset
            let file_uuid = Uuid::parse_str(&req.dataset_id).map_err(|e| {
                Status::invalid_argument(format!("Invalid dataset_id: {}", e))
            })?;

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
            match self.node_client().get_chunk_from_any(&locations, chunk_id).await {
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
// gRPC AUTHENTICATION INTERCEPTOR
// =============================================================================

use crate::auth::{AuthService, Claims};
use tonic::service::Interceptor;

/// gRPC authentication interceptor
///
/// Validates JWT tokens in the `authorization` metadata header.
/// Adds validated claims to request extensions for use by service handlers.
#[derive(Clone)]
pub struct AuthInterceptor {
    auth: Arc<AuthService>,
    /// Skip authentication for these methods (e.g., health checks)
    skip_methods: Vec<String>,
}

impl AuthInterceptor {
    /// Create a new auth interceptor
    pub fn new(auth: Arc<AuthService>) -> Self {
        Self {
            auth,
            skip_methods: vec![
                // Allow unauthenticated node registration
                "/cyxcloud.node.NodeService/RegisterNode".to_string(),
            ],
        }
    }

    /// Add a method to skip authentication for
    pub fn skip_method(mut self, method: impl Into<String>) -> Self {
        self.skip_methods.push(method.into());
        self
    }
}

impl Interceptor for AuthInterceptor {
    fn call(&mut self, mut request: tonic::Request<()>) -> Result<tonic::Request<()>, Status> {
        // Check if we have an authorization header
        let auth_header = request.metadata().get("authorization");

        // If no auth header and we allow some methods to skip, check the grpc-method header
        if auth_header.is_none() {
            // Try to get the method from a custom header (set by client if needed)
            let method_name = request
                .metadata()
                .get("grpc-method")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");

            // Check if method should skip auth
            if self.skip_methods.iter().any(|m| method_name.contains(m)) {
                return Ok(request);
            }

            // For node registration, allow without auth
            // (RegisterNode is typically the first call from a node)
            return Err(Status::unauthenticated("Missing authorization header"));
        }

        // Get authorization header
        let token = auth_header
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "))
            .ok_or_else(|| Status::unauthenticated("Invalid authorization format (expected 'Bearer <token>')"))?;

        // Validate token synchronously (we can't use async in interceptor)
        // For production, consider using a tower layer instead
        let claims = validate_token_sync(token, &self.auth)?;

        // Add claims to request extensions
        request.extensions_mut().insert(claims);

        Ok(request)
    }
}

/// Synchronous token validation for use in interceptor
fn validate_token_sync(token: &str, auth: &AuthService) -> Result<Claims, Status> {
    // Create a small runtime for async validation
    // Note: In production, consider caching validated tokens or using a tower layer
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| Status::internal(format!("Runtime error: {}", e)))?;

    rt.block_on(auth.validate_token(token))
        .map_err(|e| Status::unauthenticated(format!("Invalid token: {}", e)))
}

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
