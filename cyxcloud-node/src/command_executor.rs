//! Command executor for processing commands from central server
//!
//! Handles RepairChunk, DeleteChunk, and TransferChunk commands received
//! via heartbeat responses.

use bytes::Bytes;
use cyxcloud_core::chunk::ChunkId;
use cyxcloud_network::ChunkClient;
use cyxcloud_protocol::node::{
    DeleteChunkCommand, NodeCommand, RepairChunkCommand, TransferChunkCommand,
};
use cyxcloud_storage::backend::StorageBackendSync;
use cyxcloud_storage::RocksDbBackend;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::metrics::NodeMetrics;

/// Result of executing a command
#[derive(Debug, Clone)]
pub struct CommandResult {
    pub command_type: CommandType,
    pub success: bool,
    pub duration: Duration,
    pub error: Option<String>,
}

/// Type of command executed
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandType {
    RepairChunk,
    DeleteChunk,
    TransferChunk,
}

/// Executor for processing node commands
pub struct CommandExecutor {
    node_id: String,
    storage: Arc<RocksDbBackend>,
    chunk_client: Arc<ChunkClient>,
    metrics: NodeMetrics,
    /// Channel for sending command results (for monitoring)
    result_tx: Option<mpsc::Sender<CommandResult>>,
}

impl CommandExecutor {
    /// Create a new command executor
    pub fn new(
        node_id: String,
        storage: Arc<RocksDbBackend>,
        metrics: NodeMetrics,
    ) -> Self {
        Self {
            node_id,
            storage,
            chunk_client: Arc::new(ChunkClient::new()),
            metrics,
            result_tx: None,
        }
    }

    /// Create with custom chunk client (for testing or custom configuration)
    pub fn with_chunk_client(
        node_id: String,
        storage: Arc<RocksDbBackend>,
        chunk_client: Arc<ChunkClient>,
        metrics: NodeMetrics,
    ) -> Self {
        Self {
            node_id,
            storage,
            chunk_client,
            metrics,
            result_tx: None,
        }
    }

    /// Set a channel for receiving command results
    pub fn set_result_channel(&mut self, tx: mpsc::Sender<CommandResult>) {
        self.result_tx = Some(tx);
    }

    /// Execute a batch of commands
    pub async fn execute_commands(&self, commands: Vec<NodeCommand>) -> Vec<CommandResult> {
        let mut results = Vec::with_capacity(commands.len());

        for command in commands {
            let result = self.execute_command(command).await;

            // Send result to monitoring channel if configured
            if let Some(ref tx) = self.result_tx {
                let _ = tx.send(result.clone()).await;
            }

            results.push(result);
        }

        results
    }

    /// Execute a single command
    async fn execute_command(&self, command: NodeCommand) -> CommandResult {
        match command.command {
            Some(cyxcloud_protocol::node::node_command::Command::RepairChunk(cmd)) => {
                self.execute_repair(cmd).await
            }
            Some(cyxcloud_protocol::node::node_command::Command::DeleteChunk(cmd)) => {
                self.execute_delete(cmd).await
            }
            Some(cyxcloud_protocol::node::node_command::Command::TransferChunk(cmd)) => {
                self.execute_transfer(cmd).await
            }
            None => {
                warn!(node_id = %self.node_id, "Received empty command");
                CommandResult {
                    command_type: CommandType::RepairChunk, // Default
                    success: false,
                    duration: Duration::ZERO,
                    error: Some("Empty command".to_string()),
                }
            }
        }
    }

    /// Execute a repair chunk command
    /// Fetches the chunk from source nodes and stores it locally
    async fn execute_repair(&self, cmd: RepairChunkCommand) -> CommandResult {
        let start = Instant::now();
        let chunk_id = match parse_chunk_id(&cmd.chunk_id) {
            Some(id) => id,
            None => {
                return CommandResult {
                    command_type: CommandType::RepairChunk,
                    success: false,
                    duration: start.elapsed(),
                    error: Some("Invalid chunk ID".to_string()),
                };
            }
        };

        info!(
            node_id = %self.node_id,
            chunk_id = %chunk_id,
            source_count = cmd.source_nodes.len(),
            "Executing repair command"
        );

        // Check if we already have this chunk
        match self.storage.exists(chunk_id) {
            Ok(true) => {
                debug!(chunk_id = %chunk_id, "Chunk already exists, skipping repair");
                return CommandResult {
                    command_type: CommandType::RepairChunk,
                    success: true,
                    duration: start.elapsed(),
                    error: None,
                };
            }
            Ok(false) => {}
            Err(e) => {
                warn!(chunk_id = %chunk_id, error = %e, "Failed to check chunk existence");
            }
        }

        // Try to fetch from source nodes
        let data = match self.fetch_from_sources(&chunk_id, &cmd.source_nodes).await {
            Ok(data) => data,
            Err(e) => {
                error!(
                    chunk_id = %chunk_id,
                    error = %e,
                    "Failed to fetch chunk from any source"
                );
                return CommandResult {
                    command_type: CommandType::RepairChunk,
                    success: false,
                    duration: start.elapsed(),
                    error: Some(format!("Failed to fetch: {}", e)),
                };
            }
        };

        // Store the chunk locally
        match self.storage.put(chunk_id, data.clone()) {
            Ok(()) => {
                let duration = start.elapsed();
                self.metrics.record_store(data.len(), duration);
                info!(
                    chunk_id = %chunk_id,
                    size = data.len(),
                    duration_ms = duration.as_millis(),
                    "Repair completed successfully"
                );
                CommandResult {
                    command_type: CommandType::RepairChunk,
                    success: true,
                    duration,
                    error: None,
                }
            }
            Err(e) => {
                error!(chunk_id = %chunk_id, error = %e, "Failed to store repaired chunk");
                CommandResult {
                    command_type: CommandType::RepairChunk,
                    success: false,
                    duration: start.elapsed(),
                    error: Some(format!("Failed to store: {}", e)),
                }
            }
        }
    }

    /// Execute a delete chunk command
    async fn execute_delete(&self, cmd: DeleteChunkCommand) -> CommandResult {
        let start = Instant::now();
        let chunk_id = match parse_chunk_id(&cmd.chunk_id) {
            Some(id) => id,
            None => {
                return CommandResult {
                    command_type: CommandType::DeleteChunk,
                    success: false,
                    duration: start.elapsed(),
                    error: Some("Invalid chunk ID".to_string()),
                };
            }
        };

        info!(
            node_id = %self.node_id,
            chunk_id = %chunk_id,
            "Executing delete command"
        );

        match self.storage.delete(chunk_id) {
            Ok(deleted) => {
                let duration = start.elapsed();
                if deleted {
                    self.metrics.record_delete(duration);
                    info!(
                        chunk_id = %chunk_id,
                        duration_ms = duration.as_millis(),
                        "Delete completed successfully"
                    );
                } else {
                    debug!(chunk_id = %chunk_id, "Chunk not found for deletion");
                }
                CommandResult {
                    command_type: CommandType::DeleteChunk,
                    success: true, // Not found is still success (idempotent)
                    duration,
                    error: None,
                }
            }
            Err(e) => {
                error!(chunk_id = %chunk_id, error = %e, "Failed to delete chunk");
                CommandResult {
                    command_type: CommandType::DeleteChunk,
                    success: false,
                    duration: start.elapsed(),
                    error: Some(format!("Failed to delete: {}", e)),
                }
            }
        }
    }

    /// Execute a transfer chunk command
    /// Sends the chunk to a target node
    async fn execute_transfer(&self, cmd: TransferChunkCommand) -> CommandResult {
        let start = Instant::now();
        let chunk_id = match parse_chunk_id(&cmd.chunk_id) {
            Some(id) => id,
            None => {
                return CommandResult {
                    command_type: CommandType::TransferChunk,
                    success: false,
                    duration: start.elapsed(),
                    error: Some("Invalid chunk ID".to_string()),
                };
            }
        };

        info!(
            node_id = %self.node_id,
            chunk_id = %chunk_id,
            target = %cmd.target_node,
            "Executing transfer command"
        );

        // Get the chunk data
        let data = match self.storage.get(chunk_id) {
            Ok(Some(data)) => data,
            Ok(None) => {
                warn!(chunk_id = %chunk_id, "Chunk not found for transfer");
                return CommandResult {
                    command_type: CommandType::TransferChunk,
                    success: false,
                    duration: start.elapsed(),
                    error: Some("Chunk not found".to_string()),
                };
            }
            Err(e) => {
                error!(chunk_id = %chunk_id, error = %e, "Failed to read chunk for transfer");
                return CommandResult {
                    command_type: CommandType::TransferChunk,
                    success: false,
                    duration: start.elapsed(),
                    error: Some(format!("Failed to read: {}", e)),
                };
            }
        };

        // Send to target node
        match self.chunk_client.store_chunk(&cmd.target_node, chunk_id, data.clone()).await {
            Ok(()) => {
                let duration = start.elapsed();
                self.metrics.record_get(data.len(), duration); // Counts as outgoing transfer
                info!(
                    chunk_id = %chunk_id,
                    target = %cmd.target_node,
                    size = data.len(),
                    duration_ms = duration.as_millis(),
                    "Transfer completed successfully"
                );
                CommandResult {
                    command_type: CommandType::TransferChunk,
                    success: true,
                    duration,
                    error: None,
                }
            }
            Err(e) => {
                error!(
                    chunk_id = %chunk_id,
                    target = %cmd.target_node,
                    error = %e,
                    "Failed to transfer chunk"
                );
                CommandResult {
                    command_type: CommandType::TransferChunk,
                    success: false,
                    duration: start.elapsed(),
                    error: Some(format!("Failed to transfer: {}", e)),
                }
            }
        }
    }

    /// Fetch a chunk from one of the source nodes
    async fn fetch_from_sources(
        &self,
        chunk_id: &ChunkId,
        sources: &[String],
    ) -> Result<Bytes, String> {
        if sources.is_empty() {
            return Err("No source nodes provided".to_string());
        }

        // Try each source in order
        for source in sources {
            debug!(chunk_id = %chunk_id, source = %source, "Attempting to fetch from source");

            match self.chunk_client.get_chunk(source, *chunk_id).await {
                Ok(Some(data)) => {
                    info!(
                        chunk_id = %chunk_id,
                        source = %source,
                        size = data.len(),
                        "Successfully fetched chunk from source"
                    );
                    return Ok(data);
                }
                Ok(None) => {
                    warn!(chunk_id = %chunk_id, source = %source, "Chunk not found on source");
                }
                Err(e) => {
                    warn!(
                        chunk_id = %chunk_id,
                        source = %source,
                        error = %e,
                        "Failed to fetch from source"
                    );
                }
            }
        }

        Err(format!(
            "Failed to fetch chunk from any of {} sources",
            sources.len()
        ))
    }
}

/// Parse a chunk ID from bytes
fn parse_chunk_id(bytes: &[u8]) -> Option<ChunkId> {
    if bytes.len() != 32 {
        return None;
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(bytes);
    Some(ChunkId::from_bytes(arr))
}

/// Summary of command execution results
#[derive(Debug, Default)]
pub struct CommandBatchSummary {
    pub total: usize,
    pub successful: usize,
    pub failed: usize,
    pub repairs: usize,
    pub deletes: usize,
    pub transfers: usize,
    pub total_duration: Duration,
}

impl CommandBatchSummary {
    /// Create a summary from a list of results
    pub fn from_results(results: &[CommandResult]) -> Self {
        let mut summary = Self::default();
        summary.total = results.len();

        for result in results {
            if result.success {
                summary.successful += 1;
            } else {
                summary.failed += 1;
            }

            match result.command_type {
                CommandType::RepairChunk => summary.repairs += 1,
                CommandType::DeleteChunk => summary.deletes += 1,
                CommandType::TransferChunk => summary.transfers += 1,
            }

            summary.total_duration += result.duration;
        }

        summary
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_chunk_id_valid() {
        let bytes = [1u8; 32];
        let id = parse_chunk_id(&bytes);
        assert!(id.is_some());
    }

    #[test]
    fn test_parse_chunk_id_invalid_length() {
        let bytes = [1u8; 16];
        let id = parse_chunk_id(&bytes);
        assert!(id.is_none());
    }

    #[test]
    fn test_command_batch_summary() {
        let results = vec![
            CommandResult {
                command_type: CommandType::RepairChunk,
                success: true,
                duration: Duration::from_millis(100),
                error: None,
            },
            CommandResult {
                command_type: CommandType::DeleteChunk,
                success: true,
                duration: Duration::from_millis(50),
                error: None,
            },
            CommandResult {
                command_type: CommandType::TransferChunk,
                success: false,
                duration: Duration::from_millis(200),
                error: Some("Network error".to_string()),
            },
        ];

        let summary = CommandBatchSummary::from_results(&results);
        assert_eq!(summary.total, 3);
        assert_eq!(summary.successful, 2);
        assert_eq!(summary.failed, 1);
        assert_eq!(summary.repairs, 1);
        assert_eq!(summary.deletes, 1);
        assert_eq!(summary.transfers, 1);
        assert_eq!(summary.total_duration, Duration::from_millis(350));
    }
}
