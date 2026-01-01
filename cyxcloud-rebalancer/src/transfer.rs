//! Chunk transfer functionality for rebalancer
//!
//! Handles the actual transfer of chunks between storage nodes using gRPC.

#![allow(clippy::type_complexity)]

use cyxcloud_core::chunk::ChunkId;
use cyxcloud_metadata::postgres::Database;
use cyxcloud_network::grpc_client::ChunkClient;
use std::sync::Arc;
use thiserror::Error;
use tracing::{debug, error, info, instrument, warn};

/// Transfer errors
#[derive(Error, Debug)]
pub enum TransferError {
    #[error("Source node not found: {0}")]
    SourceNotFound(String),

    #[error("Target node not found: {0}")]
    TargetNotFound(String),

    #[error("Chunk not found on source: {0}")]
    ChunkNotFound(String),

    #[error("Transfer failed: {0}")]
    TransferFailed(String),

    #[error("Verification failed after transfer")]
    VerificationFailed,

    #[error("Database error: {0}")]
    Database(String),

    #[error("Network error: {0}")]
    Network(String),
}

/// Result type for transfer operations
pub type Result<T> = std::result::Result<T, TransferError>;

/// Chunk transfer service
pub struct ChunkTransferService {
    db: Arc<Database>,
    chunk_client: ChunkClient,
}

impl ChunkTransferService {
    /// Create a new transfer service
    pub fn new(db: Arc<Database>) -> Self {
        Self {
            db,
            chunk_client: ChunkClient::new(),
        }
    }

    /// Transfer a chunk from source to target node
    ///
    /// 1. Get chunk data from source node
    /// 2. Store chunk on target node
    /// 3. Verify chunk on target node
    /// 4. Update metadata database
    #[instrument(skip(self), fields(chunk_id = hex::encode(chunk_id)))]
    pub async fn transfer_chunk(
        &self,
        chunk_id: &[u8],
        source_peer_id: &str,
        target_peer_id: &str,
    ) -> Result<()> {
        // Get source node info
        let source_node = self
            .db
            .get_node_by_peer_id(source_peer_id)
            .await
            .map_err(|e| TransferError::Database(e.to_string()))?
            .ok_or_else(|| TransferError::SourceNotFound(source_peer_id.to_string()))?;

        // Get target node info
        let target_node = self
            .db
            .get_node_by_peer_id(target_peer_id)
            .await
            .map_err(|e| TransferError::Database(e.to_string()))?
            .ok_or_else(|| TransferError::TargetNotFound(target_peer_id.to_string()))?;

        info!(
            source = %source_peer_id,
            target = %target_peer_id,
            source_addr = %source_node.grpc_address,
            target_addr = %target_node.grpc_address,
            "Transferring chunk"
        );

        // Step 1: Get chunk from source
        let chunk_id_obj = self.bytes_to_chunk_id(chunk_id)?;

        let chunk_data = self
            .chunk_client
            .get_chunk(&source_node.grpc_address, chunk_id_obj)
            .await
            .map_err(|e| TransferError::Network(e.to_string()))?
            .ok_or_else(|| TransferError::ChunkNotFound(hex::encode(chunk_id)))?;

        debug!(size = chunk_data.len(), "Retrieved chunk from source node");

        // Step 2: Store chunk on target
        self.chunk_client
            .store_chunk(&target_node.grpc_address, chunk_id_obj, chunk_data)
            .await
            .map_err(|e| TransferError::Network(e.to_string()))?;

        debug!("Stored chunk on target node");

        // Step 3: Verify chunk on target
        let (valid, _size) = self
            .chunk_client
            .verify_chunk(&target_node.grpc_address, chunk_id_obj)
            .await
            .map_err(|e| TransferError::Network(e.to_string()))?;

        if !valid {
            error!(
                chunk_id = hex::encode(chunk_id),
                target = %target_peer_id,
                "Chunk verification failed after transfer"
            );
            return Err(TransferError::VerificationFailed);
        }

        debug!("Chunk verified on target node");

        // Step 4: Update metadata database
        self.db
            .add_chunk_location(chunk_id, target_node.id)
            .await
            .map_err(|e| TransferError::Database(e.to_string()))?;

        info!(
            chunk_id = hex::encode(chunk_id),
            source = %source_peer_id,
            target = %target_peer_id,
            "Chunk transfer completed successfully"
        );

        Ok(())
    }

    /// Transfer a chunk to multiple targets
    #[instrument(skip(self), fields(chunk_id = hex::encode(chunk_id), target_count = target_peer_ids.len()))]
    pub async fn transfer_to_multiple(
        &self,
        chunk_id: &[u8],
        source_peer_id: &str,
        target_peer_ids: Vec<String>,
    ) -> Vec<String> {
        let mut successful = Vec::new();

        for target in &target_peer_ids {
            match self.transfer_chunk(chunk_id, source_peer_id, target).await {
                Ok(()) => {
                    successful.push(target.clone());
                }
                Err(e) => {
                    warn!(
                        chunk_id = hex::encode(chunk_id),
                        target = %target,
                        error = %e,
                        "Transfer to target failed"
                    );
                }
            }
        }

        successful
    }

    /// Convert byte slice to ChunkId
    fn bytes_to_chunk_id(&self, bytes: &[u8]) -> Result<ChunkId> {
        if bytes.len() != 32 {
            return Err(TransferError::TransferFailed(format!(
                "Invalid chunk ID length: {} (expected 32)",
                bytes.len()
            )));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(bytes);
        Ok(ChunkId::from_bytes(arr))
    }
}

/// Create a transfer function for use with the executor
///
/// This returns a closure that can be used with Executor::execute()
pub fn create_transfer_fn(
    db: Arc<Database>,
) -> impl Fn(
    String,
    String,
    Vec<u8>,
    Vec<String>,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = std::result::Result<Vec<String>, String>> + Send>,
> + Clone
       + Send
       + Sync
       + 'static {
    let service = Arc::new(ChunkTransferService::new(db));

    move |source_node: String, _task_id: String, chunk_id: Vec<u8>, target_nodes: Vec<String>| {
        let service = service.clone();

        Box::pin(async move {
            let successful = service
                .transfer_to_multiple(&chunk_id, &source_node, target_nodes)
                .await;

            if successful.is_empty() {
                Err("All transfers failed".to_string())
            } else {
                Ok(successful)
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunk_id_hex_conversion() {
        // Test hex encoding/decoding for chunk IDs
        let chunk_id = vec![0xde, 0xad, 0xbe, 0xef];
        let hex_str = hex::encode(&chunk_id);
        assert_eq!(hex_str, "deadbeef");

        let decoded = hex::decode(&hex_str).unwrap();
        assert_eq!(decoded, chunk_id);
    }

    #[test]
    fn test_transfer_error_display() {
        let err = TransferError::ChunkNotFound("abc123".to_string());
        assert!(err.to_string().contains("abc123"));
    }
}
