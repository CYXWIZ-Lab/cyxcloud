//! Network client for rebalancer
//!
//! Implements the NetworkClient trait using database for status
//! and gRPC for reachability checks.

use crate::detector::{NetworkClient, NodeAvailability};
use crate::planner::NodeInfo;
use cyxcloud_core::chunk::ChunkId;
use cyxcloud_metadata::postgres::Database;
use cyxcloud_network::grpc_client::ChunkClient;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, instrument, warn};

/// Network client that uses database for status and gRPC for health checks
pub struct GrpcNetworkClient {
    db: Arc<Database>,
    chunk_client: ChunkClient,
    /// Maps node_id (peer_id) to grpc_address for health checks
    node_addresses: tokio::sync::RwLock<std::collections::HashMap<String, String>>,
}

impl GrpcNetworkClient {
    /// Create a new network client
    pub fn new(db: Arc<Database>) -> Self {
        Self {
            db,
            chunk_client: ChunkClient::new(),
            node_addresses: tokio::sync::RwLock::new(std::collections::HashMap::new()),
        }
    }

    /// Refresh the node address cache from database
    async fn refresh_node_addresses(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let nodes = self
            .db
            .get_all_nodes()
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;

        let mut addrs = self.node_addresses.write().await;
        addrs.clear();

        for node in nodes {
            addrs.insert(node.peer_id, node.grpc_address);
        }

        Ok(())
    }

    /// Get the gRPC address for a node ID
    async fn get_node_address(&self, node_id: &str) -> Option<String> {
        // Try cache first
        {
            let addrs = self.node_addresses.read().await;
            if let Some(addr) = addrs.get(node_id) {
                return Some(addr.clone());
            }
        }

        // Refresh cache and try again
        if self.refresh_node_addresses().await.is_ok() {
            let addrs = self.node_addresses.read().await;
            return addrs.get(node_id).cloned();
        }

        None
    }

    /// Get node info for the planner
    pub async fn get_node_info(&self) -> Result<Vec<NodeInfo>, Box<dyn std::error::Error + Send + Sync>> {
        let nodes = self
            .db
            .get_all_nodes()
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;

        let result: Vec<NodeInfo> = nodes
            .into_iter()
            .map(|n| {
                let is_healthy = n.status == "online" || n.status == "recovering";
                let available = n.storage_available() as u64;
                NodeInfo {
                    id: n.peer_id,
                    address: n.grpc_address,
                    available_storage: available,
                    load: 0.0, // Could calculate from usage/capacity
                    datacenter: n.datacenter,
                    is_healthy,
                }
            })
            .collect();

        Ok(result)
    }
}

#[async_trait::async_trait]
impl NetworkClient for GrpcNetworkClient {
    #[instrument(skip(self))]
    async fn get_all_nodes(
        &self,
    ) -> Result<Vec<String>, Box<dyn std::error::Error + Send + Sync>> {
        let nodes = self
            .db
            .get_all_nodes()
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;

        // Update address cache while we're at it
        let mut addrs = self.node_addresses.write().await;
        addrs.clear();

        let result: Vec<String> = nodes
            .into_iter()
            .map(|n| {
                let id = n.peer_id.clone();
                addrs.insert(n.peer_id, n.grpc_address);
                id
            })
            .collect();

        Ok(result)
    }

    #[instrument(skip(self))]
    async fn get_all_nodes_with_status(
        &self,
    ) -> Result<Vec<(String, String)>, Box<dyn std::error::Error + Send + Sync>> {
        let nodes = self
            .db
            .get_all_nodes()
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;

        let result: Vec<(String, String)> = nodes
            .into_iter()
            .map(|n| (n.peer_id, n.status))
            .collect();

        Ok(result)
    }

    #[instrument(skip(self))]
    async fn check_node_health(
        &self,
        node_id: &str,
        timeout: Duration,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        // First check database status
        if let Ok(Some(node)) = self.db.get_node_by_peer_id(node_id).await {
            if node.status != "online" && node.status != "recovering" {
                debug!(node_id = node_id, status = %node.status, "Node not healthy per database");
                return Ok(false);
            }

            // Node is "online" or "recovering" in DB, verify it's actually reachable
            let is_reachable = tokio::time::timeout(
                timeout,
                self.chunk_client.is_reachable(&node.grpc_address),
            )
            .await
            .unwrap_or(false);

            if !is_reachable {
                warn!(
                    node_id = node_id,
                    address = %node.grpc_address,
                    "Node marked healthy but not reachable via gRPC"
                );
            }

            Ok(is_reachable)
        } else {
            debug!(node_id = node_id, "Node not found in database");
            Ok(false)
        }
    }

    #[instrument(skip(self))]
    async fn check_node_availability(
        &self,
        node_id: &str,
        timeout: Duration,
    ) -> Result<NodeAvailability, Box<dyn std::error::Error + Send + Sync>> {
        if let Ok(Some(node)) = self.db.get_node_by_peer_id(node_id).await {
            match node.status.as_str() {
                "online" => {
                    // Verify reachability for online nodes
                    let is_reachable = tokio::time::timeout(
                        timeout,
                        self.chunk_client.is_reachable(&node.grpc_address),
                    )
                    .await
                    .unwrap_or(false);

                    if is_reachable {
                        Ok(NodeAvailability::Online)
                    } else {
                        Ok(NodeAvailability::Unavailable)
                    }
                }
                "recovering" => Ok(NodeAvailability::Recovering),
                _ => Ok(NodeAvailability::Unavailable),
            }
        } else {
            Ok(NodeAvailability::Unavailable)
        }
    }

    #[instrument(skip(self))]
    async fn verify_chunk_integrity(
        &self,
        node_id: &str,
        chunk_id: &[u8],
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let address = match self.get_node_address(node_id).await {
            Some(addr) => addr,
            None => {
                debug!(node_id = node_id, "Node address not found");
                return Ok(false);
            }
        };

        // Convert bytes to ChunkId
        if chunk_id.len() != 32 {
            debug!(
                node_id = node_id,
                len = chunk_id.len(),
                "Invalid chunk ID length"
            );
            return Ok(false);
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(chunk_id);
        let chunk_id_obj = ChunkId::from_bytes(arr);

        // Use the ChunkClient to verify the chunk
        match self.chunk_client.verify_chunk(&address, chunk_id_obj).await {
            Ok((valid, _size)) => Ok(valid),
            Err(e) => {
                warn!(
                    node_id = node_id,
                    chunk_id = hex::encode(chunk_id),
                    error = %e,
                    "Chunk verification failed"
                );
                Ok(false)
            }
        }
    }
}
