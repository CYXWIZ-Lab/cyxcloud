//! Quorum operations for CyxCloud
//!
//! Implements configurable read/write quorum for distributed storage.

use futures::future::join_all;
use std::time::Duration;
use thiserror::Error;
use tokio::time::timeout;
use tracing::{debug, info, warn};

/// Quorum error types
#[derive(Error, Debug)]
pub enum QuorumError {
    #[error("Quorum not achieved: got {got} of {needed} responses")]
    QuorumNotAchieved { got: usize, needed: usize },

    #[error("No nodes available")]
    NoNodesAvailable,

    #[error("All operations failed")]
    AllFailed,

    #[error("Timeout waiting for quorum")]
    Timeout,

    #[error("Database error: {0}")]
    Database(String),

    #[error("Network error: {0}")]
    Network(String),
}

pub type Result<T> = std::result::Result<T, QuorumError>;

/// Quorum configuration
#[derive(Debug, Clone)]
pub struct QuorumConfig {
    /// Number of nodes to read from (should be > N/2 for consistency)
    pub read_quorum: usize,
    /// Number of nodes that must confirm writes
    pub write_quorum: usize,
    /// Total number of replicas to maintain
    pub replication_factor: usize,
    /// Timeout for individual node operations
    pub node_timeout: Duration,
    /// Timeout for overall quorum operation
    pub quorum_timeout: Duration,
}

impl Default for QuorumConfig {
    fn default() -> Self {
        Self {
            read_quorum: 2,                          // Read from 2 nodes
            write_quorum: 2,                         // Write must succeed on 2 nodes
            replication_factor: 3,                   // Store on 3 nodes total
            node_timeout: Duration::from_secs(10),   // 10s per node
            quorum_timeout: Duration::from_secs(30), // 30s total
        }
    }
}

impl QuorumConfig {
    /// Create a strict consistency config (majority quorum)
    pub fn strict(replication_factor: usize) -> Self {
        let majority = (replication_factor / 2) + 1;
        Self {
            read_quorum: majority,
            write_quorum: majority,
            replication_factor,
            ..Default::default()
        }
    }

    /// Create an eventual consistency config (write to one, read from any)
    pub fn eventual(replication_factor: usize) -> Self {
        Self {
            read_quorum: 1,
            write_quorum: 1,
            replication_factor,
            ..Default::default()
        }
    }

    /// Verify configuration is valid
    pub fn validate(&self) -> Result<()> {
        if self.read_quorum == 0 {
            return Err(QuorumError::QuorumNotAchieved { got: 0, needed: 1 });
        }
        if self.write_quorum == 0 {
            return Err(QuorumError::QuorumNotAchieved { got: 0, needed: 1 });
        }
        if self.write_quorum > self.replication_factor {
            return Err(QuorumError::QuorumNotAchieved {
                got: self.replication_factor,
                needed: self.write_quorum,
            });
        }
        Ok(())
    }
}

/// Result of a single node operation
#[derive(Debug)]
pub struct NodeResult<T> {
    pub node_id: String,
    pub result: std::result::Result<T, String>,
    pub latency_ms: u64,
}

/// Aggregated quorum result
#[derive(Debug)]
pub struct QuorumResult<T> {
    /// Successful results
    pub successes: Vec<NodeResult<T>>,
    /// Failed results
    pub failures: Vec<NodeResult<T>>,
    /// Whether quorum was achieved
    pub quorum_achieved: bool,
    /// Total time taken
    pub total_latency_ms: u64,
}

impl<T> QuorumResult<T> {
    /// Get the number of successful operations
    pub fn success_count(&self) -> usize {
        self.successes.len()
    }

    /// Get the number of failed operations
    pub fn failure_count(&self) -> usize {
        self.failures.len()
    }

    /// Get the first successful result
    pub fn first_success(&self) -> Option<&T> {
        self.successes
            .first()
            .map(|r| r.result.as_ref().ok())
            .flatten()
    }
}

/// Quorum coordinator
pub struct QuorumCoordinator {
    config: QuorumConfig,
}

impl QuorumCoordinator {
    /// Create a new quorum coordinator
    pub fn new(config: QuorumConfig) -> Self {
        Self { config }
    }

    /// Execute a read operation with quorum
    ///
    /// Reads from multiple nodes and returns the result once quorum is achieved.
    pub async fn read_with_quorum<F, Fut, T>(
        &self,
        nodes: Vec<String>,
        operation: F,
    ) -> Result<QuorumResult<T>>
    where
        F: Fn(String) -> Fut + Clone,
        Fut: std::future::Future<Output = std::result::Result<T, String>> + Send,
        T: Clone + Send + 'static,
    {
        if nodes.is_empty() {
            return Err(QuorumError::NoNodesAvailable);
        }

        let needed = self.config.read_quorum.min(nodes.len());
        let start = std::time::Instant::now();

        debug!(nodes = nodes.len(), quorum = needed, "Starting quorum read");

        // Launch reads to all nodes concurrently
        let futures: Vec<_> = nodes
            .into_iter()
            .map(|node| {
                let op = operation.clone();
                let node_timeout = self.config.node_timeout;
                async move {
                    let node_start = std::time::Instant::now();
                    let result = timeout(node_timeout, op(node.clone())).await;
                    let latency_ms = node_start.elapsed().as_millis() as u64;

                    match result {
                        Ok(Ok(value)) => NodeResult {
                            node_id: node,
                            result: Ok(value),
                            latency_ms,
                        },
                        Ok(Err(e)) => NodeResult {
                            node_id: node,
                            result: Err(e),
                            latency_ms,
                        },
                        Err(_) => NodeResult {
                            node_id: node,
                            result: Err("timeout".to_string()),
                            latency_ms,
                        },
                    }
                }
            })
            .collect();

        // Wait for quorum (or all to complete)
        let results = timeout(self.config.quorum_timeout, join_all(futures))
            .await
            .map_err(|_| QuorumError::Timeout)?;

        let total_latency_ms = start.elapsed().as_millis() as u64;

        let (successes, failures): (Vec<_>, Vec<_>) =
            results.into_iter().partition(|r| r.result.is_ok());

        let quorum_achieved = successes.len() >= needed;

        if !quorum_achieved {
            warn!(
                successes = successes.len(),
                needed = needed,
                "Read quorum not achieved"
            );
        }

        Ok(QuorumResult {
            successes,
            failures,
            quorum_achieved,
            total_latency_ms,
        })
    }

    /// Execute a write operation with quorum
    ///
    /// Writes to multiple nodes and waits for quorum confirmations.
    pub async fn write_with_quorum<F, Fut>(
        &self,
        nodes: Vec<String>,
        operation: F,
    ) -> Result<QuorumResult<()>>
    where
        F: Fn(String) -> Fut + Clone,
        Fut: std::future::Future<Output = std::result::Result<(), String>> + Send,
    {
        if nodes.is_empty() {
            return Err(QuorumError::NoNodesAvailable);
        }

        let needed = self.config.write_quorum.min(nodes.len());
        let start = std::time::Instant::now();

        debug!(
            nodes = nodes.len(),
            quorum = needed,
            "Starting quorum write"
        );

        // Launch writes to all nodes concurrently
        let futures: Vec<_> = nodes
            .into_iter()
            .map(|node| {
                let op = operation.clone();
                let node_timeout = self.config.node_timeout;
                async move {
                    let node_start = std::time::Instant::now();
                    let result = timeout(node_timeout, op(node.clone())).await;
                    let latency_ms = node_start.elapsed().as_millis() as u64;

                    match result {
                        Ok(Ok(())) => NodeResult {
                            node_id: node,
                            result: Ok(()),
                            latency_ms,
                        },
                        Ok(Err(e)) => NodeResult {
                            node_id: node,
                            result: Err(e),
                            latency_ms,
                        },
                        Err(_) => NodeResult {
                            node_id: node,
                            result: Err("timeout".to_string()),
                            latency_ms,
                        },
                    }
                }
            })
            .collect();

        // Wait for all writes to complete
        let results = timeout(self.config.quorum_timeout, join_all(futures))
            .await
            .map_err(|_| QuorumError::Timeout)?;

        let total_latency_ms = start.elapsed().as_millis() as u64;

        let (successes, failures): (Vec<_>, Vec<_>) =
            results.into_iter().partition(|r| r.result.is_ok());

        let quorum_achieved = successes.len() >= needed;

        if !quorum_achieved {
            warn!(
                successes = successes.len(),
                needed = needed,
                "Write quorum not achieved"
            );
            return Err(QuorumError::QuorumNotAchieved {
                got: successes.len(),
                needed,
            });
        }

        info!(
            successes = successes.len(),
            failures = failures.len(),
            latency_ms = total_latency_ms,
            "Write quorum achieved"
        );

        Ok(QuorumResult {
            successes,
            failures,
            quorum_achieved,
            total_latency_ms,
        })
    }

    /// Get the fastest N nodes from a list
    pub async fn select_fastest_nodes<F, Fut>(
        &self,
        nodes: Vec<String>,
        count: usize,
        ping: F,
    ) -> Vec<String>
    where
        F: Fn(String) -> Fut + Clone,
        Fut: std::future::Future<Output = Option<u64>> + Send, // Returns latency in ms
    {
        let futures: Vec<_> = nodes
            .into_iter()
            .map(|node| {
                let ping_fn = ping.clone();
                async move {
                    let latency = ping_fn(node.clone()).await;
                    (node, latency)
                }
            })
            .collect();

        let mut results = join_all(futures).await;

        // Filter out unreachable nodes and sort by latency
        results.retain(|(_, latency)| latency.is_some());
        results.sort_by_key(|(_, latency)| latency.unwrap_or(u64::MAX));

        results
            .into_iter()
            .take(count)
            .map(|(node, _)| node)
            .collect()
    }
}

/// Read repair coordinator
///
/// Handles read repair when inconsistencies are detected during quorum reads.
pub struct ReadRepair {
    coordinator: QuorumCoordinator,
}

impl ReadRepair {
    /// Create a new read repair handler
    pub fn new(config: QuorumConfig) -> Self {
        Self {
            coordinator: QuorumCoordinator::new(config),
        }
    }

    /// Repair a chunk by copying from source to targets
    pub async fn repair_chunk<F, Fut>(
        &self,
        chunk_id: &[u8],
        source_node: String,
        target_nodes: Vec<String>,
        transfer: F,
    ) -> Result<Vec<String>>
    where
        F: Fn(String, String, Vec<u8>) -> Fut + Clone + Send,
        Fut: std::future::Future<Output = std::result::Result<(), String>> + Send,
    {
        let chunk_id_vec = chunk_id.to_vec();

        let result = self
            .coordinator
            .write_with_quorum(target_nodes.clone(), |target| {
                let transfer_fn = transfer.clone();
                let source = source_node.clone();
                let chunk = chunk_id_vec.clone();
                async move { transfer_fn(source, target, chunk).await }
            })
            .await?;

        let repaired: Vec<String> = result.successes.into_iter().map(|r| r.node_id).collect();

        Ok(repaired)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quorum_config_default() {
        let config = QuorumConfig::default();
        assert_eq!(config.read_quorum, 2);
        assert_eq!(config.write_quorum, 2);
        assert_eq!(config.replication_factor, 3);
    }

    #[test]
    fn test_quorum_config_strict() {
        let config = QuorumConfig::strict(5);
        assert_eq!(config.read_quorum, 3);
        assert_eq!(config.write_quorum, 3);
        assert_eq!(config.replication_factor, 5);
    }

    #[test]
    fn test_quorum_config_eventual() {
        let config = QuorumConfig::eventual(3);
        assert_eq!(config.read_quorum, 1);
        assert_eq!(config.write_quorum, 1);
    }

    #[test]
    fn test_quorum_config_validate() {
        let config = QuorumConfig::default();
        assert!(config.validate().is_ok());

        let invalid = QuorumConfig {
            write_quorum: 10,
            replication_factor: 3,
            ..Default::default()
        };
        assert!(invalid.validate().is_err());
    }

    #[tokio::test]
    async fn test_quorum_coordinator_no_nodes() {
        let coordinator = QuorumCoordinator::new(QuorumConfig::default());
        let result: Result<QuorumResult<()>> = coordinator
            .read_with_quorum(vec![], |_node: String| async { Ok(()) })
            .await;
        assert!(matches!(result, Err(QuorumError::NoNodesAvailable)));
    }
}
