//! Chunk Detector
//!
//! Scans the cluster for:
//! - Under-replicated chunks (below target replication factor)
//! - Over-replicated chunks (above target replication factor)
//! - Orphaned chunks (no longer referenced by any file)
//! - Corrupt chunks (failed integrity check)

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};
use thiserror::Error;
use tracing::{debug, info, instrument};

/// Detector errors
#[derive(Error, Debug)]
pub enum DetectorError {
    #[error("Metadata error: {0}")]
    Metadata(String),

    #[error("Network error: {0}")]
    Network(String),

    #[error("Storage error: {0}")]
    Storage(String),
}

pub type Result<T> = std::result::Result<T, DetectorError>;

/// Chunk health status
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChunkHealth {
    /// Chunk is healthy with correct replication
    Healthy,
    /// Chunk has fewer replicas than target
    UnderReplicated { current: usize, target: usize },
    /// Chunk has more replicas than needed
    OverReplicated { current: usize, target: usize },
    /// Chunk has no replicas (data loss risk)
    Critical,
    /// Chunk is orphaned (not referenced by any file)
    Orphaned,
    /// Chunk failed integrity check
    Corrupt { node_ids: Vec<String> },
}

/// Information about a chunk that needs attention
#[derive(Debug, Clone)]
pub struct ChunkIssue {
    /// Chunk ID (content hash)
    pub chunk_id: Vec<u8>,
    /// Current health status
    pub health: ChunkHealth,
    /// Nodes currently holding this chunk
    pub current_nodes: Vec<String>,
    /// File ID this chunk belongs to (if any)
    pub file_id: Option<String>,
    /// Priority score (higher = more urgent)
    pub priority: u32,
    /// When the issue was detected
    pub detected_at: Instant,
}

impl ChunkIssue {
    /// Calculate priority based on health status
    pub fn calculate_priority(health: &ChunkHealth) -> u32 {
        match health {
            ChunkHealth::Critical => 1000,
            ChunkHealth::UnderReplicated { current, target } => {
                if *current == 0 {
                    900
                } else if *current == 1 {
                    800
                } else {
                    500 + (target - current) as u32 * 100
                }
            }
            ChunkHealth::Corrupt { .. } => 700,
            ChunkHealth::OverReplicated { .. } => 100,
            ChunkHealth::Orphaned => 50,
            ChunkHealth::Healthy => 0,
        }
    }
}

/// Detector configuration
#[derive(Debug, Clone)]
pub struct DetectorConfig {
    /// Target replication factor
    pub replication_factor: usize,
    /// Maximum chunks to scan per iteration
    pub batch_size: usize,
    /// Minimum time between full scans
    pub scan_interval: Duration,
    /// Enable integrity checking
    pub verify_integrity: bool,
    /// Timeout for node health checks
    pub health_check_timeout: Duration,
}

impl Default for DetectorConfig {
    fn default() -> Self {
        Self {
            replication_factor: 3,
            batch_size: 1000,
            scan_interval: Duration::from_secs(60),
            verify_integrity: false, // Expensive, enable in production
            health_check_timeout: Duration::from_secs(5),
        }
    }
}

/// Scan results
#[derive(Debug, Default)]
pub struct ScanResult {
    /// Total chunks scanned
    pub total_scanned: usize,
    /// Under-replicated chunks found
    pub under_replicated: Vec<ChunkIssue>,
    /// Over-replicated chunks found
    pub over_replicated: Vec<ChunkIssue>,
    /// Orphaned chunks found
    pub orphaned: Vec<ChunkIssue>,
    /// Corrupt chunks found
    pub corrupt: Vec<ChunkIssue>,
    /// Scan duration
    pub duration: Duration,
    /// Any errors encountered
    pub errors: Vec<String>,
}

impl ScanResult {
    /// Get all issues sorted by priority
    pub fn all_issues(&self) -> Vec<&ChunkIssue> {
        let mut issues: Vec<&ChunkIssue> = self
            .under_replicated
            .iter()
            .chain(self.over_replicated.iter())
            .chain(self.orphaned.iter())
            .chain(self.corrupt.iter())
            .collect();

        issues.sort_by(|a, b| b.priority.cmp(&a.priority));
        issues
    }

    /// Check if there are any critical issues
    pub fn has_critical_issues(&self) -> bool {
        self.under_replicated
            .iter()
            .any(|i| matches!(i.health, ChunkHealth::Critical))
            || !self.corrupt.is_empty()
    }

    /// Get summary statistics
    pub fn summary(&self) -> String {
        format!(
            "Scanned {} chunks in {:?}: {} under-replicated, {} over-replicated, {} orphaned, {} corrupt",
            self.total_scanned,
            self.duration,
            self.under_replicated.len(),
            self.over_replicated.len(),
            self.orphaned.len(),
            self.corrupt.len()
        )
    }
}

/// Chunk detector service
pub struct Detector {
    config: DetectorConfig,
    /// Last scan time
    last_scan: Option<Instant>,
    /// Cached node health status (legacy, for backwards compatibility)
    node_health: HashMap<String, bool>,
    /// Cached node availability status
    node_availability: HashMap<String, NodeAvailability>,
}

impl Detector {
    /// Create a new detector
    pub fn new(config: DetectorConfig) -> Self {
        Self {
            config,
            last_scan: None,
            node_health: HashMap::new(),
            node_availability: HashMap::new(),
        }
    }

    /// Scan for chunk issues
    ///
    /// Returns a list of chunks that need attention.
    #[instrument(skip(self, metadata_client, network_client))]
    pub async fn scan<M, N>(
        &mut self,
        metadata_client: &M,
        network_client: &N,
    ) -> Result<ScanResult>
    where
        M: MetadataClient,
        N: NetworkClient,
    {
        let start = Instant::now();
        let mut result = ScanResult::default();

        info!("Starting chunk scan");

        // Step 1: Get all healthy nodes
        let healthy_nodes = self.get_healthy_nodes(network_client).await?;
        let healthy_node_ids: HashSet<_> = healthy_nodes.iter().cloned().collect();

        debug!(healthy_nodes = healthy_nodes.len(), "Got healthy nodes");

        // Step 2: Get under-replicated chunks from metadata
        let under_rep_chunks = metadata_client
            .get_under_replicated_chunks(self.config.batch_size)
            .await
            .map_err(|e| DetectorError::Metadata(e.to_string()))?;

        for chunk in under_rep_chunks {
            // Filter out nodes that are unhealthy
            let available_nodes: Vec<_> = chunk
                .node_ids
                .iter()
                .filter(|n| healthy_node_ids.contains(*n))
                .cloned()
                .collect();

            let health = if available_nodes.is_empty() {
                ChunkHealth::Critical
            } else {
                ChunkHealth::UnderReplicated {
                    current: available_nodes.len(),
                    target: self.config.replication_factor,
                }
            };

            let priority = ChunkIssue::calculate_priority(&health);

            result.under_replicated.push(ChunkIssue {
                chunk_id: chunk.chunk_id,
                health,
                current_nodes: available_nodes,
                file_id: chunk.file_id,
                priority,
                detected_at: Instant::now(),
            });
        }

        result.total_scanned += result.under_replicated.len();

        // Step 3: Check for over-replicated chunks (optional)
        // This is less critical and can be done less frequently

        // Step 4: Update stats
        result.duration = start.elapsed();
        self.last_scan = Some(Instant::now());

        info!(
            under_replicated = result.under_replicated.len(),
            duration = ?result.duration,
            "Scan complete"
        );

        Ok(result)
    }

    /// Get list of healthy nodes (can read from these nodes)
    /// This includes both 'online' and 'recovering' nodes since they can serve existing chunks.
    async fn get_healthy_nodes<N: NetworkClient>(&mut self, client: &N) -> Result<Vec<String>> {
        let all_nodes = client
            .get_all_nodes()
            .await
            .map_err(|e| DetectorError::Network(e.to_string()))?;

        let mut healthy = Vec::new();

        for node_id in all_nodes {
            let availability = client
                .check_node_availability(&node_id, self.config.health_check_timeout)
                .await
                .unwrap_or(NodeAvailability::Unavailable);

            // Update both caches for backwards compatibility
            self.node_availability.insert(node_id.clone(), availability);
            self.node_health.insert(
                node_id.clone(),
                availability != NodeAvailability::Unavailable,
            );

            // For reading chunks, both online and recovering nodes are acceptable
            if availability == NodeAvailability::Online
                || availability == NodeAvailability::Recovering
            {
                healthy.push(node_id);
            }
        }

        Ok(healthy)
    }

    /// Get list of write-healthy nodes (can write new chunks to these nodes)
    /// This only includes 'online' nodes since 'recovering' nodes are in quarantine.
    pub fn get_write_healthy_nodes(&self) -> Vec<String> {
        self.node_availability
            .iter()
            .filter(|(_, avail)| **avail == NodeAvailability::Online)
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Get node availability status
    pub fn get_node_availability(&self, node_id: &str) -> NodeAvailability {
        self.node_availability
            .get(node_id)
            .copied()
            .unwrap_or(NodeAvailability::Unavailable)
    }

    /// Check if enough time has passed since last scan
    pub fn should_scan(&self) -> bool {
        match self.last_scan {
            None => true,
            Some(last) => last.elapsed() >= self.config.scan_interval,
        }
    }

    /// Get node health cache (legacy)
    pub fn node_health(&self) -> &HashMap<String, bool> {
        &self.node_health
    }

    /// Get node availability cache
    pub fn node_availability_cache(&self) -> &HashMap<String, NodeAvailability> {
        &self.node_availability
    }
}

// =============================================================================
// TRAITS FOR DEPENDENCY INJECTION
// =============================================================================

/// Metadata client trait for testing
#[async_trait::async_trait]
pub trait MetadataClient: Send + Sync {
    async fn get_under_replicated_chunks(
        &self,
        limit: usize,
    ) -> std::result::Result<Vec<ChunkInfo>, Box<dyn std::error::Error + Send + Sync>>;

    async fn get_orphaned_chunks(
        &self,
        limit: usize,
    ) -> std::result::Result<Vec<ChunkInfo>, Box<dyn std::error::Error + Send + Sync>>;
}

/// Node availability status for rebalancing
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeAvailability {
    /// Node is fully online - can read and write
    Online,
    /// Node is recovering (quarantine) - can read only
    Recovering,
    /// Node is not available
    Unavailable,
}

/// Network client trait for testing
#[async_trait::async_trait]
pub trait NetworkClient: Send + Sync {
    async fn get_all_nodes(
        &self,
    ) -> std::result::Result<Vec<String>, Box<dyn std::error::Error + Send + Sync>>;

    /// Get all nodes with their status
    /// Returns (node_id, status) pairs
    async fn get_all_nodes_with_status(
        &self,
    ) -> std::result::Result<Vec<(String, String)>, Box<dyn std::error::Error + Send + Sync>> {
        // Default implementation: all nodes are online
        let nodes = self.get_all_nodes().await?;
        Ok(nodes
            .into_iter()
            .map(|n| (n, "online".to_string()))
            .collect())
    }

    async fn check_node_health(
        &self,
        node_id: &str,
        timeout: Duration,
    ) -> std::result::Result<bool, Box<dyn std::error::Error + Send + Sync>>;

    /// Check node availability (online, recovering, or unavailable)
    async fn check_node_availability(
        &self,
        node_id: &str,
        timeout: Duration,
    ) -> std::result::Result<NodeAvailability, Box<dyn std::error::Error + Send + Sync>> {
        // Default implementation: healthy = online, not healthy = unavailable
        match self.check_node_health(node_id, timeout).await {
            Ok(true) => Ok(NodeAvailability::Online),
            _ => Ok(NodeAvailability::Unavailable),
        }
    }

    async fn verify_chunk_integrity(
        &self,
        node_id: &str,
        chunk_id: &[u8],
    ) -> std::result::Result<bool, Box<dyn std::error::Error + Send + Sync>>;
}

/// Chunk information from metadata
#[derive(Debug, Clone)]
pub struct ChunkInfo {
    pub chunk_id: Vec<u8>,
    pub node_ids: Vec<String>,
    pub file_id: Option<String>,
    pub size: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_priority_calculation() {
        assert_eq!(ChunkIssue::calculate_priority(&ChunkHealth::Critical), 1000);
        assert_eq!(
            ChunkIssue::calculate_priority(&ChunkHealth::UnderReplicated {
                current: 1,
                target: 3
            }),
            800
        );
        assert_eq!(
            ChunkIssue::calculate_priority(&ChunkHealth::UnderReplicated {
                current: 2,
                target: 3
            }),
            600
        );
        assert_eq!(
            ChunkIssue::calculate_priority(&ChunkHealth::OverReplicated {
                current: 5,
                target: 3
            }),
            100
        );
        assert_eq!(ChunkIssue::calculate_priority(&ChunkHealth::Healthy), 0);
    }

    #[test]
    fn test_detector_config_default() {
        let config = DetectorConfig::default();
        assert_eq!(config.replication_factor, 3);
        assert_eq!(config.batch_size, 1000);
        assert!(!config.verify_integrity);
    }

    #[test]
    fn test_scan_result_summary() {
        let mut result = ScanResult::default();
        result.total_scanned = 100;
        result.duration = Duration::from_millis(500);
        result.under_replicated.push(ChunkIssue {
            chunk_id: vec![1, 2, 3],
            health: ChunkHealth::UnderReplicated {
                current: 1,
                target: 3,
            },
            current_nodes: vec!["n1".to_string()],
            file_id: None,
            priority: 800,
            detected_at: Instant::now(),
        });

        let summary = result.summary();
        assert!(summary.contains("100 chunks"));
        assert!(summary.contains("1 under-replicated"));
    }

    #[test]
    fn test_scan_result_all_issues_sorted() {
        let mut result = ScanResult::default();

        result.under_replicated.push(ChunkIssue {
            chunk_id: vec![1],
            health: ChunkHealth::UnderReplicated {
                current: 2,
                target: 3,
            },
            current_nodes: vec![],
            file_id: None,
            priority: 600,
            detected_at: Instant::now(),
        });

        result.corrupt.push(ChunkIssue {
            chunk_id: vec![2],
            health: ChunkHealth::Corrupt {
                node_ids: vec!["n1".to_string()],
            },
            current_nodes: vec![],
            file_id: None,
            priority: 700,
            detected_at: Instant::now(),
        });

        let issues = result.all_issues();
        assert_eq!(issues.len(), 2);
        assert_eq!(issues[0].priority, 700); // Corrupt first (higher priority)
        assert_eq!(issues[1].priority, 600);
    }
}
