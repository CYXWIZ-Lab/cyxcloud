//! CyxCloud Network Layer
//!
//! Provides hybrid networking for CyxCloud storage nodes:
//! - **gRPC (tonic)**: Direct node-to-node data transfer for chunks
//! - **libp2p**: Peer discovery via Kademlia DHT
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────┐
//! │                    NetworkManager                        │
//! │                                                          │
//! │  ┌───────────────────┐     ┌───────────────────────┐   │
//! │  │   gRPC Server     │     │   DiscoveryService    │   │
//! │  │   (ChunkService)  │     │   (libp2p + Kademlia) │   │
//! │  └───────────────────┘     └───────────────────────┘   │
//! │           │                          │                   │
//! │           │                          │                   │
//! │  ┌───────────────────┐     ┌───────────────────────┐   │
//! │  │   gRPC Client     │     │   Peer Registry       │   │
//! │  │   (ChunkClient)   │     │   (PeerInfo cache)    │   │
//! │  └───────────────────┘     └───────────────────────┘   │
//! │           │                          │                   │
//! └───────────│──────────────────────────│───────────────────┘
//!             │                          │
//!             ▼                          ▼
//!     ┌───────────────┐         ┌───────────────┐
//!     │ Remote Nodes  │         │ Peer Network  │
//!     │ (gRPC calls)  │         │ (DHT lookups) │
//!     └───────────────┘         └───────────────┘
//! ```
//!
//! # Usage
//!
//! ```ignore
//! use cyxcloud_network::{NetworkManager, NetworkConfig};
//! use cyxcloud_storage::RocksDbBackend;
//! use std::sync::Arc;
//!
//! // Create storage backend
//! let storage = Arc::new(RocksDbBackend::open_default("./data")?);
//!
//! // Create network manager
//! let config = NetworkConfig::default();
//! let mut manager = NetworkManager::new(config, storage).await?;
//!
//! // Start networking (blocks until shutdown)
//! manager.start().await?;
//! ```

pub mod behavior;
pub mod discovery;
pub mod grpc_client;
pub mod grpc_server;
pub mod protocol;

// Re-exports
pub use behavior::{BehaviourConfig, CyxCloudBehaviour, CyxCloudEvent};
pub use discovery::{DiscoveryConfig, DiscoveryEvent, DiscoveryService, PeerInfo};
pub use grpc_client::{ChunkClient, ChunkClientConfig};
pub use grpc_server::{ChunkServiceImpl, GrpcServerConfig};
pub use protocol::{
    ChunkLocationAnnouncement, NodeAnnouncement, NodeCapacity, NodeLocation, NodeStatus,
    PROTOCOL_VERSION,
};

use bytes::Bytes;
use cyxcloud_core::chunk::ChunkId;
use cyxcloud_core::error::{CyxCloudError, Result};
use cyxcloud_storage::RocksDbBackend;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{error, info};

/// Network configuration
#[derive(Debug, Clone)]
pub struct NetworkConfig {
    /// gRPC server configuration
    pub grpc: GrpcServerConfig,
    /// Discovery configuration
    pub discovery: DiscoveryConfig,
    /// Node ID (unique identifier for this node)
    pub node_id: String,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            grpc: GrpcServerConfig::default(),
            discovery: DiscoveryConfig::default(),
            node_id: uuid::Uuid::new_v4().to_string(),
        }
    }
}

impl NetworkConfig {
    /// Create a new config with the given node ID
    pub fn new(node_id: impl Into<String>) -> Self {
        Self {
            node_id: node_id.into(),
            ..Default::default()
        }
    }

    /// Set the gRPC listen address
    pub fn with_grpc_addr(mut self, addr: SocketAddr) -> Self {
        self.grpc.listen_addr = addr;
        self
    }

    /// Add a bootstrap peer for discovery
    pub fn with_bootstrap_peer(mut self, peer_id: libp2p::PeerId, addr: libp2p::Multiaddr) -> Self {
        self.discovery.bootstrap_peers.push((peer_id, addr));
        self
    }
}

/// Unified network manager for CyxCloud nodes
///
/// Manages both gRPC and libp2p networking in a single interface.
pub struct NetworkManager {
    /// Node configuration
    config: NetworkConfig,
    /// Local chunk storage
    storage: Arc<RocksDbBackend>,
    /// gRPC client for connecting to other nodes
    grpc_client: Arc<ChunkClient>,
    /// Shutdown signal sender
    shutdown_tx: Option<mpsc::Sender<()>>,
}

impl NetworkManager {
    /// Create a new NetworkManager
    pub fn new(config: NetworkConfig, storage: Arc<RocksDbBackend>) -> Self {
        let grpc_client = Arc::new(ChunkClient::with_config(ChunkClientConfig::default()));

        info!(
            node_id = %config.node_id,
            grpc_addr = %config.grpc.listen_addr,
            "Created network manager"
        );

        Self {
            config,
            storage,
            grpc_client,
            shutdown_tx: None,
        }
    }

    /// Get the node ID
    pub fn node_id(&self) -> &str {
        &self.config.node_id
    }

    /// Get the gRPC client for making requests to other nodes
    pub fn grpc_client(&self) -> Arc<ChunkClient> {
        self.grpc_client.clone()
    }

    /// Start the network manager
    ///
    /// This starts both the gRPC server and the libp2p discovery service.
    /// The method blocks until shutdown is signaled.
    pub async fn start(&mut self) -> Result<()> {
        let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<()>(1);
        self.shutdown_tx = Some(shutdown_tx);

        // Start gRPC server in background
        let grpc_config = self.config.grpc.clone();
        let storage = self.storage.clone();
        let node_id = self.config.node_id.clone();

        let grpc_handle = tokio::spawn(async move {
            if let Err(e) = grpc_server::start_server(grpc_config, storage, node_id).await {
                error!(error = %e, "gRPC server error");
            }
        });

        info!(
            grpc_addr = %self.config.grpc.listen_addr,
            "Network manager started"
        );

        // Wait for shutdown signal
        shutdown_rx.recv().await;

        info!("Shutting down network manager");

        // Abort gRPC server
        grpc_handle.abort();

        Ok(())
    }

    /// Signal shutdown
    pub async fn shutdown(&self) -> Result<()> {
        if let Some(ref tx) = self.shutdown_tx {
            tx.send(()).await.map_err(|_| {
                CyxCloudError::Internal("Failed to send shutdown signal".to_string())
            })?;
        }
        Ok(())
    }

    /// Store a chunk to multiple remote nodes
    pub async fn store_chunk_to_nodes(
        &self,
        chunk_id: ChunkId,
        data: Bytes,
        target_nodes: &[String],
    ) -> Result<Vec<String>> {
        grpc_client::store_to_multiple_nodes(&self.grpc_client, chunk_id, data, target_nodes).await
    }

    /// Get a chunk from any of the provided nodes
    pub async fn get_chunk_from_any(
        &self,
        chunk_id: ChunkId,
        source_nodes: &[String],
    ) -> Result<Bytes> {
        grpc_client::get_from_any_node(&self.grpc_client, chunk_id, source_nodes).await
    }

    /// Check if a remote node is reachable
    pub async fn is_node_reachable(&self, addr: &str) -> bool {
        self.grpc_client.is_reachable(addr).await
    }

    /// Get the number of active connections
    pub fn connection_count(&self) -> usize {
        self.grpc_client.connection_count()
    }
}

/// Create and run a standalone network node
///
/// This is a convenience function for running a complete CyxCloud node.
pub async fn run_node(
    config: NetworkConfig,
    storage: Arc<RocksDbBackend>,
) -> Result<()> {
    let mut manager = NetworkManager::new(config, storage);
    manager.start().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use cyxcloud_storage::StorageConfig;
    use tempfile::TempDir;

    fn create_test_storage() -> (Arc<RocksDbBackend>, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let config = StorageConfig::new(temp_dir.path());
        let storage = Arc::new(RocksDbBackend::open(config).unwrap());
        (storage, temp_dir)
    }

    #[test]
    fn test_network_config_default() {
        let config = NetworkConfig::default();
        assert!(!config.node_id.is_empty());
        assert_eq!(config.grpc.listen_addr.port(), 50051);
    }

    #[test]
    fn test_network_config_builder() {
        let addr: SocketAddr = "127.0.0.1:9000".parse().unwrap();
        let config = NetworkConfig::new("test-node").with_grpc_addr(addr);

        assert_eq!(config.node_id, "test-node");
        assert_eq!(config.grpc.listen_addr.port(), 9000);
    }

    #[test]
    fn test_network_manager_creation() {
        let (storage, _dir) = create_test_storage();
        let config = NetworkConfig::new("test-node");
        let manager = NetworkManager::new(config, storage);

        assert_eq!(manager.node_id(), "test-node");
        assert_eq!(manager.connection_count(), 0);
    }
}
