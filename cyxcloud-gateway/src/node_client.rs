//! Storage Node Client
//!
//! Manages connections to storage nodes for chunk operations.
//! Uses the ChunkService gRPC interface defined in cyxcloud-protocol.

#![allow(unused_imports)]

use bytes::Bytes;
use cyxcloud_core::chunk::ChunkId;
use cyxcloud_protocol::chunk::{
    chunk_service_client::ChunkServiceClient, ChunkMetadata as ProtoChunkMetadata, GetChunkRequest,
    StoreChunkRequest,
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tonic::transport::Channel;
use tracing::{debug, error, info, warn};

/// Error types for node client operations
#[derive(Debug, thiserror::Error)]
pub enum NodeClientError {
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),

    #[error("Store failed: {0}")]
    StoreFailed(String),

    #[error("Chunk not found: {0}")]
    ChunkNotFound(String),

    /// The node answered, but the bytes do not hash to the chunk id asked for:
    /// bit rot, a bug, or a node lying. Never decoded, always counted against the node.
    #[error("Corrupt shard from {node}: data does not match chunk id {chunk_id}")]
    CorruptShard { node: String, chunk_id: String },

    #[error("No nodes available")]
    NoNodesAvailable,

    #[error("All nodes failed to store chunk")]
    AllNodesFailed,

    #[error("gRPC error: {0}")]
    GrpcError(#[from] tonic::Status),

    #[error("Transport error: {0}")]
    TransportError(#[from] tonic::transport::Error),
}

/// Configuration for the node client
#[derive(Debug, Clone)]
pub struct NodeClientConfig {
    /// Connection timeout in seconds
    pub connect_timeout_secs: u64,

    /// Request timeout in seconds
    pub request_timeout_secs: u64,

    /// Number of retries per operation
    pub max_retries: u32,

    /// Required replicas for write operations
    pub write_replicas: usize,

    /// Maximum number of cached connections
    pub max_connections: usize,

    /// Evict connections unused for this many seconds
    pub stale_connection_secs: u64,
}

impl Default for NodeClientConfig {
    fn default() -> Self {
        Self {
            connect_timeout_secs: 10,
            request_timeout_secs: 60,
            max_retries: 3,
            write_replicas: 3, // Store each chunk on 3 nodes
            max_connections: 100,
            stale_connection_secs: 300, // 5 minutes
        }
    }
}

/// A pooled connection with last-used tracking
struct PooledConnection {
    client: ChunkServiceClient<Channel>,
    last_used: std::time::Instant,
}

/// Client for communicating with storage nodes
pub struct NodeClient {
    /// Configuration
    config: NodeClientConfig,

    /// Connection pool: node_address -> pooled connection
    connections: RwLock<HashMap<String, PooledConnection>>,

    /// Per-node access secrets: node_address -> what the node expects in
    /// `authorization: Bearer <secret>` (nodes.access_secret)
    secrets: RwLock<HashMap<String, String>>,
}

impl NodeClient {
    /// Create a new node client
    pub fn new(config: NodeClientConfig) -> Self {
        Self {
            config,
            connections: RwLock::new(HashMap::new()),
            secrets: RwLock::new(HashMap::new()),
        }
    }

    /// Remember the secret the node at `address` expects on chunk RPCs.
    pub async fn set_access_secret(&self, address: &str, secret: &str) {
        self.secrets
            .write()
            .await
            .insert(address.to_string(), secret.to_string());
    }

    /// Wrap a message in a request that carries the node's access secret.
    async fn authorized<T>(&self, address: &str, inner: T) -> tonic::Request<T> {
        let mut request = tonic::Request::new(inner);
        if let Some(secret) = self.secrets.read().await.get(address) {
            if let Ok(value) = format!("Bearer {}", secret).parse() {
                request.metadata_mut().insert("authorization", value);
            }
        }
        request
    }

    /// Get or create a connection to a storage node
    pub async fn get_connection(
        &self,
        address: &str,
    ) -> Result<ChunkServiceClient<Channel>, NodeClientError> {
        // Check if we have an existing connection
        {
            let mut connections = self.connections.write().await;
            if let Some(pooled) = connections.get_mut(address) {
                pooled.last_used = std::time::Instant::now();
                return Ok(pooled.client.clone());
            }
        }

        // Create a new connection
        let endpoint = format!("http://{}", address);
        debug!(address = %address, "Connecting to storage node");

        let channel = Channel::from_shared(endpoint)
            .map_err(|e| NodeClientError::ConnectionFailed(e.to_string()))?
            .connect_timeout(std::time::Duration::from_secs(
                self.config.connect_timeout_secs,
            ))
            .timeout(std::time::Duration::from_secs(
                self.config.request_timeout_secs,
            ))
            .connect()
            .await?;

        let client = ChunkServiceClient::new(channel);

        // Store the connection, evicting stale entries if at capacity
        {
            let mut connections = self.connections.write().await;

            // Evict stale connections
            let stale_threshold =
                std::time::Duration::from_secs(self.config.stale_connection_secs);
            let now = std::time::Instant::now();
            connections.retain(|addr, pooled| {
                let keep = now.duration_since(pooled.last_used) < stale_threshold;
                if !keep {
                    debug!(address = %addr, "Evicting stale connection");
                }
                keep
            });

            // If still at capacity, evict the oldest connection
            if connections.len() >= self.config.max_connections {
                if let Some(oldest_addr) = connections
                    .iter()
                    .min_by_key(|(_, p)| p.last_used)
                    .map(|(addr, _)| addr.clone())
                {
                    debug!(address = %oldest_addr, "Evicting oldest connection (pool full)");
                    connections.remove(&oldest_addr);
                }
            }

            connections.insert(
                address.to_string(),
                PooledConnection {
                    client: client.clone(),
                    last_used: std::time::Instant::now(),
                },
            );
        }

        info!(address = %address, "Connected to storage node");
        Ok(client)
    }

    /// Store a chunk on a storage node
    pub async fn store_chunk(
        &self,
        node_address: &str,
        chunk_id: &[u8],
        data: Bytes,
        metadata: Option<ChunkMeta>,
    ) -> Result<(), NodeClientError> {
        let mut client = self.get_connection(node_address).await?;

        let proto_metadata = metadata.map(|m| ProtoChunkMetadata {
            chunk_id: chunk_id.to_vec(),
            size: m.size,
            index: m.index,
            total_chunks: m.total_chunks,
            parent_id: m
                .parent_id
                .map(|id| id.as_bytes().to_vec())
                .unwrap_or_default(),
            created_at: m.created_at,
            encrypted: m.encrypted,
            shard_index: m.shard_index.unwrap_or(0),
        });

        let request = StoreChunkRequest {
            chunk_id: chunk_id.to_vec(),
            data: data.to_vec(),
            metadata: proto_metadata,
        };

        let response = client
            .store_chunk(self.authorized(node_address, request).await)
            .await?;
        let inner = response.into_inner();

        if inner.success {
            debug!(node = %node_address, chunk_id = %hex::encode(chunk_id), "Chunk stored successfully");
            Ok(())
        } else {
            error!(node = %node_address, error = %inner.error, "Failed to store chunk");
            Err(NodeClientError::StoreFailed(inner.error))
        }
    }

    /// Retrieve a chunk from a storage node
    pub async fn get_chunk(
        &self,
        node_address: &str,
        chunk_id: &[u8],
    ) -> Result<Bytes, NodeClientError> {
        let mut client = self.get_connection(node_address).await?;

        let request = GetChunkRequest {
            chunk_id: chunk_id.to_vec(),
        };

        let response = client
            .get_chunk(self.authorized(node_address, request).await)
            .await?;
        let inner = response.into_inner();

        if inner.found {
            // Shard ids are content hashes: a node is believed only when the bytes prove it.
            if ChunkId::from_data(&inner.data).as_bytes() != chunk_id {
                warn!(
                    node = %node_address,
                    chunk_id = %hex::encode(chunk_id),
                    size = inner.data.len(),
                    "Node returned data that does not hash to the requested chunk id"
                );
                return Err(NodeClientError::CorruptShard {
                    node: node_address.to_string(),
                    chunk_id: hex::encode(chunk_id),
                });
            }
            debug!(node = %node_address, chunk_id = %hex::encode(chunk_id), "Chunk retrieved and verified");
            Ok(Bytes::from(inner.data))
        } else {
            Err(NodeClientError::ChunkNotFound(hex::encode(chunk_id)))
        }
    }

    /// Store a chunk on multiple nodes for redundancy
    pub async fn store_chunk_replicated(
        &self,
        node_addresses: &[String],
        chunk_id: &[u8],
        data: Bytes,
        metadata: Option<ChunkMeta>,
    ) -> Result<Vec<String>, NodeClientError> {
        if node_addresses.is_empty() {
            return Err(NodeClientError::NoNodesAvailable);
        }

        let mut successful_nodes = Vec::new();
        let mut last_error = None;

        for address in node_addresses.iter().take(self.config.write_replicas) {
            match self
                .store_chunk(address, chunk_id, data.clone(), metadata.clone())
                .await
            {
                Ok(()) => {
                    successful_nodes.push(address.clone());
                }
                Err(e) => {
                    warn!(node = %address, error = %e, "Failed to store chunk on node");
                    last_error = Some(e);
                }
            }
        }

        if successful_nodes.is_empty() {
            Err(last_error.unwrap_or(NodeClientError::AllNodesFailed))
        } else {
            Ok(successful_nodes)
        }
    }

    /// Retrieve a chunk from any available node
    pub async fn get_chunk_from_any(
        &self,
        node_addresses: &[String],
        chunk_id: &[u8],
    ) -> Result<Bytes, NodeClientError> {
        let fetch = self.get_chunk_from_any_reporting(node_addresses, chunk_id).await;
        match fetch.data {
            Some(data) => Ok(data),
            None => Err(fetch
                .last_error
                .unwrap_or(NodeClientError::ChunkNotFound(hex::encode(chunk_id)))),
        }
    }

    /// Retrieve a chunk from any available node and say which nodes served
    /// corrupt bytes on the way, so the caller can count it against them.
    pub async fn get_chunk_from_any_reporting(
        &self,
        node_addresses: &[String],
        chunk_id: &[u8],
    ) -> ShardFetch {
        let mut fetch = ShardFetch::default();
        if node_addresses.is_empty() {
            fetch.last_error = Some(NodeClientError::NoNodesAvailable);
            return fetch;
        }

        for address in node_addresses {
            match self.get_chunk(address, chunk_id).await {
                Ok(data) => {
                    fetch.data = Some(data);
                    return fetch;
                }
                Err(e) => {
                    if matches!(e, NodeClientError::CorruptShard { .. }) {
                        fetch.corrupt_nodes.push(address.clone());
                    }
                    warn!(node = %address, error = %e, "Failed to get chunk from node");
                    fetch.last_error = Some(e);
                }
            }
        }

        fetch
    }

    /// Close a specific connection
    pub async fn close_connection(&self, address: &str) {
        let mut connections = self.connections.write().await;
        connections.remove(address);
        debug!(address = %address, "Closed connection to storage node");
    }

    /// Close all connections
    pub async fn close_all(&self) {
        let mut connections = self.connections.write().await;
        let count = connections.len();
        connections.clear();
        info!(count = count, "Closed all storage node connections");
    }

    /// Get current pool size
    pub async fn pool_size(&self) -> usize {
        self.connections.read().await.len()
    }
}

/// Outcome of asking several nodes for one shard
#[derive(Debug, Default)]
pub struct ShardFetch {
    /// The verified bytes, if any node had them
    pub data: Option<Bytes>,
    /// Nodes that answered with bytes that do not hash to the chunk id
    pub corrupt_nodes: Vec<String>,
    /// The last failure seen (only meaningful when `data` is None)
    pub last_error: Option<NodeClientError>,
}

/// Simplified chunk metadata for client operations
#[derive(Debug, Clone)]
pub struct ChunkMeta {
    pub size: u64,
    pub index: u32,
    pub total_chunks: u32,
    pub parent_id: Option<uuid::Uuid>,
    pub created_at: i64,
    pub encrypted: bool,
    pub shard_index: Option<u32>,
}

impl From<&cyxcloud_core::ChunkMetadata> for ChunkMeta {
    fn from(meta: &cyxcloud_core::ChunkMetadata) -> Self {
        Self {
            size: meta.size,
            index: meta.index,
            total_chunks: meta.total_chunks,
            parent_id: meta.parent_id,
            created_at: meta.created_at,
            encrypted: meta.encrypted,
            shard_index: meta.shard_index.map(|i| i as u32),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let config = NodeClientConfig::default();
        assert_eq!(config.write_replicas, 3);
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.max_connections, 100);
        assert_eq!(config.stale_connection_secs, 300);
    }

    #[test]
    fn test_chunk_meta_conversion() {
        let core_meta = cyxcloud_core::ChunkMetadata::new(
            cyxcloud_core::ChunkId::from_data(b"test"),
            100,
            0,
            1,
        );

        let meta: ChunkMeta = (&core_meta).into();
        assert_eq!(meta.size, 100);
        assert_eq!(meta.index, 0);
        assert_eq!(meta.total_chunks, 1);
    }
}

#[cfg(test)]
mod fake_node_tests {
    //! A storage node is a stranger's machine: these tests stand up a node
    //! that answers GetChunk with whatever bytes it likes.
    use super::*;
    use cyxcloud_protocol::chunk::chunk_service_server::{ChunkService, ChunkServiceServer};
    use cyxcloud_protocol::chunk::{
        ChunkData, DeleteChunkRequest, DeleteChunkResponse, GetChunkResponse, StoreChunkResponse,
        StreamChunksRequest, VerifyChunkRequest, VerifyChunkResponse,
    };
    use tonic::transport::server::TcpIncoming;
    use tonic::{Request, Response, Status};

    struct FakeNode {
        answer: Vec<u8>,
    }

    #[tonic::async_trait]
    impl ChunkService for FakeNode {
        async fn store_chunk(
            &self,
            _r: Request<StoreChunkRequest>,
        ) -> Result<Response<StoreChunkResponse>, Status> {
            Err(Status::unimplemented("fake node"))
        }
        async fn get_chunk(
            &self,
            _r: Request<GetChunkRequest>,
        ) -> Result<Response<GetChunkResponse>, Status> {
            Ok(Response::new(GetChunkResponse {
                data: self.answer.clone(),
                metadata: None,
                found: true,
            }))
        }
        async fn delete_chunk(
            &self,
            _r: Request<DeleteChunkRequest>,
        ) -> Result<Response<DeleteChunkResponse>, Status> {
            Err(Status::unimplemented("fake node"))
        }
        type StreamChunksStream = tokio_stream::wrappers::ReceiverStream<Result<ChunkData, Status>>;
        async fn stream_chunks(
            &self,
            _r: Request<StreamChunksRequest>,
        ) -> Result<Response<Self::StreamChunksStream>, Status> {
            Err(Status::unimplemented("fake node"))
        }
        async fn verify_chunk(
            &self,
            _r: Request<VerifyChunkRequest>,
        ) -> Result<Response<VerifyChunkResponse>, Status> {
            Err(Status::unimplemented("fake node"))
        }
    }

    async fn serve(answer: &[u8]) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let incoming = TcpIncoming::from_listener(listener, true, None).unwrap();
        let node = FakeNode {
            answer: answer.to_vec(),
        };
        tokio::spawn(async move {
            tonic::transport::Server::builder()
                .add_service(ChunkServiceServer::new(node))
                .serve_with_incoming(incoming)
                .await
                .unwrap();
        });
        addr.to_string()
    }

    #[tokio::test]
    async fn a_node_returning_wrong_bytes_is_a_corrupt_shard() {
        let good = b"the shard as it was written".to_vec();
        let id = ChunkId::from_data(&good).as_bytes().to_vec();
        let liar = serve(b"tampered bytes").await;
        let client = NodeClient::new(NodeClientConfig::default());

        let err = client.get_chunk(&liar, &id).await.unwrap_err();

        assert!(matches!(err, NodeClientError::CorruptShard { .. }), "{err}");
    }

    #[tokio::test]
    async fn an_honest_node_is_verified_and_served() {
        let good = b"the shard as it was written".to_vec();
        let id = ChunkId::from_data(&good).as_bytes().to_vec();
        let honest = serve(&good).await;
        let client = NodeClient::new(NodeClientConfig::default());

        let data = client.get_chunk(&honest, &id).await.unwrap();

        assert_eq!(data, Bytes::from(good));
    }

    #[tokio::test]
    async fn a_corrupt_node_is_skipped_and_reported() {
        let good = b"the shard as it was written".to_vec();
        let id = ChunkId::from_data(&good).as_bytes().to_vec();
        let liar = serve(b"tampered bytes").await;
        let honest = serve(&good).await;
        let client = NodeClient::new(NodeClientConfig::default());

        let fetch = client
            .get_chunk_from_any_reporting(&[liar.clone(), honest], &id)
            .await;

        assert_eq!(fetch.data, Some(Bytes::from(good)));
        assert_eq!(fetch.corrupt_nodes, vec![liar]);
    }

    #[tokio::test]
    async fn only_liars_means_no_data_and_every_liar_named() {
        let good = b"the shard as it was written".to_vec();
        let id = ChunkId::from_data(&good).as_bytes().to_vec();
        let liar_a = serve(b"tampered").await;
        let liar_b = serve(b"also tampered").await;
        let client = NodeClient::new(NodeClientConfig::default());

        let fetch = client
            .get_chunk_from_any_reporting(&[liar_a.clone(), liar_b.clone()], &id)
            .await;

        assert!(fetch.data.is_none());
        assert_eq!(fetch.corrupt_nodes, vec![liar_a, liar_b]);
        assert!(matches!(fetch.last_error, Some(NodeClientError::CorruptShard { .. })));
    }
}
