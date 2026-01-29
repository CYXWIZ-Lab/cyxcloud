//! Storage Node Client
//!
//! Manages connections to storage nodes for chunk operations.
//! Uses the ChunkService gRPC interface defined in cyxcloud-protocol.

#![allow(unused_imports)]

use bytes::Bytes;
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
}

impl NodeClient {
    /// Create a new node client
    pub fn new(config: NodeClientConfig) -> Self {
        Self {
            config,
            connections: RwLock::new(HashMap::new()),
        }
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

        let response = client.store_chunk(request).await?;
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

        let response = client.get_chunk(request).await?;
        let inner = response.into_inner();

        if inner.found {
            debug!(node = %node_address, chunk_id = %hex::encode(chunk_id), "Chunk retrieved");
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
        if node_addresses.is_empty() {
            return Err(NodeClientError::NoNodesAvailable);
        }

        let mut last_error = None;

        for address in node_addresses {
            match self.get_chunk(address, chunk_id).await {
                Ok(data) => return Ok(data),
                Err(e) => {
                    warn!(node = %address, error = %e, "Failed to get chunk from node");
                    last_error = Some(e);
                }
            }
        }

        Err(last_error.unwrap_or(NodeClientError::ChunkNotFound(hex::encode(chunk_id))))
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
