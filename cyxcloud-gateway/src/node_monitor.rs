//! Node Lifecycle Monitor
//!
//! Background task that monitors node health and manages lifecycle transitions:
//! - online -> offline (5 min no heartbeat)
//! - offline -> draining (4 hours offline)
//! - offline/draining -> removed (7 days offline)
//! - recovering -> online (5 min quarantine complete)

use crate::state::AppState;
use cyxcloud_metadata::{FaultToleranceConfig, MetadataService};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::time::interval;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

/// Node lifecycle monitor configuration
#[derive(Debug, Clone)]
pub struct NodeMonitorConfig {
    /// How often to run the monitoring loop
    pub check_interval: Duration,
    /// Fault tolerance thresholds
    pub fault_tolerance: FaultToleranceConfig,
    /// Enable metrics reporting
    pub enable_metrics: bool,
}

impl Default for NodeMonitorConfig {
    fn default() -> Self {
        Self {
            check_interval: Duration::from_secs(30),
            fault_tolerance: FaultToleranceConfig::default(),
            enable_metrics: true,
        }
    }
}

impl NodeMonitorConfig {
    /// Create configuration from environment variables
    pub fn from_env() -> Self {
        Self {
            check_interval: Duration::from_secs(
                std::env::var("NODE_MONITOR_INTERVAL_SECS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(30),
            ),
            fault_tolerance: FaultToleranceConfig::from_env(),
            enable_metrics: true,
        }
    }
}

/// Metrics for node lifecycle events
#[derive(Debug, Default, Clone)]
pub struct NodeMonitorMetrics {
    pub nodes_marked_offline: u64,
    pub nodes_started_draining: u64,
    pub nodes_removed: u64,
    pub nodes_entered_recovery: u64,
    pub nodes_recovered: u64,
    pub last_check_at: Option<std::time::Instant>,
    pub last_check_duration_ms: u64,
    pub check_cycles_completed: u64,
}

/// Node lifecycle monitor
pub struct NodeMonitor {
    config: NodeMonitorConfig,
    metrics: Arc<RwLock<NodeMonitorMetrics>>,
}

impl NodeMonitor {
    /// Create a new node monitor
    pub fn new(config: NodeMonitorConfig) -> Self {
        Self {
            config,
            metrics: Arc::new(RwLock::new(NodeMonitorMetrics::default())),
        }
    }

    /// Start the background monitoring loop
    pub fn start(self: Arc<Self>, state: Arc<AppState>) -> tokio::task::JoinHandle<()> {
        let monitor = self;
        let check_interval = monitor.config.check_interval;

        tokio::spawn(async move {
            let mut check_timer = interval(check_interval);

            info!(
                interval_secs = check_interval.as_secs(),
                offline_threshold_mins = monitor.config.fault_tolerance.offline_threshold.as_secs() / 60,
                drain_threshold_hours = monitor.config.fault_tolerance.drain_threshold.as_secs() / 3600,
                remove_threshold_days = monitor.config.fault_tolerance.remove_threshold.as_secs() / 86400,
                recovery_quarantine_mins = monitor.config.fault_tolerance.recovery_quarantine.as_secs() / 60,
                "Node lifecycle monitor started"
            );

            loop {
                check_timer.tick().await;

                if let Some(metadata) = state.metadata_service() {
                    if let Err(e) = monitor.run_check_cycle(metadata).await {
                        error!(error = %e, "Node monitor check cycle failed");
                    }
                } else {
                    debug!("Metadata service not available, skipping node monitor cycle");
                }
            }
        })
    }

    /// Run a single check cycle
    async fn run_check_cycle(&self, metadata: &MetadataService) -> anyhow::Result<()> {
        let start = std::time::Instant::now();
        let db = metadata.database();

        let mut stale_count = 0;
        let mut draining_count = 0;
        let mut removed_count = 0;
        let mut recovered_count = 0;

        // Step 1: Mark stale online nodes as offline
        let stale_nodes = db
            .get_stale_online_nodes(self.config.fault_tolerance.offline_threshold)
            .await?;

        for node in &stale_nodes {
            info!(
                node_id = %node.id,
                peer_id = %node.peer_id,
                last_heartbeat = ?node.last_heartbeat,
                "Marking node as offline (no heartbeat)"
            );

            if let Err(e) = db.mark_node_offline(node.id).await {
                error!(error = %e, node_id = %node.id, "Failed to mark node offline");
            } else {
                stale_count += 1;
            }
        }

        // Step 2: Start draining for nodes offline > threshold
        let nodes_to_drain = db
            .get_nodes_for_draining(self.config.fault_tolerance.drain_threshold)
            .await?;

        for node in &nodes_to_drain {
            info!(
                node_id = %node.id,
                peer_id = %node.peer_id,
                first_offline_at = ?node.first_offline_at,
                "Starting node drain (offline too long)"
            );

            if let Err(e) = db.mark_node_draining(node.id).await {
                error!(error = %e, node_id = %node.id, "Failed to mark node as draining");
            } else {
                draining_count += 1;

                // Trigger chunk evacuation
                self.trigger_chunk_evacuation(metadata, node.id).await;
            }
        }

        // Step 3: Remove nodes offline > threshold
        let nodes_to_remove = db
            .get_nodes_for_removal(self.config.fault_tolerance.remove_threshold)
            .await?;

        for node in &nodes_to_remove {
            warn!(
                node_id = %node.id,
                peer_id = %node.peer_id,
                first_offline_at = ?node.first_offline_at,
                "Auto-removing node (offline > {} days)",
                self.config.fault_tolerance.remove_threshold.as_secs() / 86400
            );

            if let Err(e) = db.delete_node(node.id).await {
                error!(error = %e, node_id = %node.id, "Failed to delete node");
            } else {
                removed_count += 1;
            }
        }

        // Step 4: Promote recovered nodes from quarantine to online
        let recovered_nodes = db
            .get_recovered_nodes(self.config.fault_tolerance.recovery_quarantine)
            .await?;

        for node in &recovered_nodes {
            info!(
                node_id = %node.id,
                peer_id = %node.peer_id,
                "Node completed recovery quarantine, marking online"
            );

            if let Err(e) = db.mark_node_online(node.id).await {
                error!(error = %e, node_id = %node.id, "Failed to mark node online");
            } else {
                recovered_count += 1;
            }
        }

        // Update metrics
        let duration = start.elapsed();
        {
            let mut metrics = self.metrics.write().await;
            metrics.nodes_marked_offline += stale_count;
            metrics.nodes_started_draining += draining_count;
            metrics.nodes_removed += removed_count;
            metrics.nodes_recovered += recovered_count;
            metrics.last_check_at = Some(start);
            metrics.last_check_duration_ms = duration.as_millis() as u64;
            metrics.check_cycles_completed += 1;
        }

        // Log summary if anything changed
        if stale_count > 0 || draining_count > 0 || removed_count > 0 || recovered_count > 0 {
            info!(
                duration_ms = duration.as_millis(),
                marked_offline = stale_count,
                started_draining = draining_count,
                removed = removed_count,
                recovered = recovered_count,
                "Node monitor check cycle complete with changes"
            );
        } else {
            debug!(
                duration_ms = duration.as_millis(),
                "Node monitor check cycle complete (no changes)"
            );
        }

        Ok(())
    }

    /// Trigger chunk evacuation for a draining node
    async fn trigger_chunk_evacuation(&self, metadata: &MetadataService, node_id: Uuid) {
        let db = metadata.database();

        // Get all chunks on this node
        match db.get_chunks_on_node(node_id).await {
            Ok(chunks) => {
                if chunks.is_empty() {
                    debug!(node_id = %node_id, "No chunks to evacuate from draining node");
                    return;
                }

                info!(
                    node_id = %node_id,
                    chunk_count = chunks.len(),
                    "Creating repair jobs for chunk evacuation"
                );

                // Get available target nodes
                let online_nodes = match db.get_online_nodes().await {
                    Ok(nodes) => nodes
                        .into_iter()
                        .filter(|n| n.id != node_id)
                        .collect::<Vec<_>>(),
                    Err(e) => {
                        error!(error = %e, "Failed to get online nodes for evacuation");
                        return;
                    }
                };

                if online_nodes.is_empty() {
                    warn!(
                        node_id = %node_id,
                        chunk_count = chunks.len(),
                        "No online nodes available for chunk evacuation - chunks may be lost!"
                    );
                    return;
                }

                // Create repair jobs for each chunk
                let mut created = 0;
                let mut failed = 0;
                for (i, chunk_loc) in chunks.iter().enumerate() {
                    // Round-robin target selection
                    let target_node = &online_nodes[i % online_nodes.len()];

                    match db
                        .create_repair_job(
                            &chunk_loc.chunk_id,
                            Some(node_id), // Source is the draining node
                            target_node.id,
                            100, // High priority for evacuation
                        )
                        .await
                    {
                        Ok(_) => created += 1,
                        Err(e) => {
                            warn!(
                                error = %e,
                                chunk_id = %hex::encode(&chunk_loc.chunk_id),
                                "Failed to create repair job for evacuation"
                            );
                            failed += 1;
                        }
                    }
                }

                info!(
                    node_id = %node_id,
                    created = created,
                    failed = failed,
                    "Chunk evacuation repair jobs created"
                );
            }
            Err(e) => {
                error!(error = %e, node_id = %node_id, "Failed to get chunks for evacuation");
            }
        }
    }

    /// Get current metrics
    pub async fn get_metrics(&self) -> NodeMonitorMetrics {
        self.metrics.read().await.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_monitor_config_default() {
        let config = NodeMonitorConfig::default();
        assert_eq!(config.check_interval.as_secs(), 30);
        assert_eq!(config.fault_tolerance.offline_threshold.as_secs(), 5 * 60);
    }

    #[test]
    fn test_node_monitor_metrics_default() {
        let metrics = NodeMonitorMetrics::default();
        assert_eq!(metrics.nodes_marked_offline, 0);
        assert_eq!(metrics.check_cycles_completed, 0);
    }
}
