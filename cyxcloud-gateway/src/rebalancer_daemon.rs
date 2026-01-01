//! Rebalancer Daemon
//!
//! Background task that monitors chunk replication and repairs under-replicated data.
//! Runs automatically when the gateway starts with a metadata service configured.

use crate::state::AppState;
use cyxcloud_metadata::postgres::Database;
use cyxcloud_rebalancer::{
    Detector, DetectorConfig, Executor, ExecutorConfig, GrpcNetworkClient, Planner, PlannerConfig,
    PostgresMetadataClient,
};
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

/// Rebalancer daemon configuration
#[derive(Debug, Clone)]
pub struct RebalancerDaemonConfig {
    /// How often to scan for under-replicated chunks
    pub scan_interval: Duration,
    /// Target replication factor
    pub replication_factor: usize,
    /// Maximum concurrent repair tasks
    pub repair_parallelism: usize,
    /// Maximum bytes to repair per hour (GB)
    pub rate_limit_gb: u64,
    /// Dry run mode (scan but don't repair)
    pub dry_run: bool,
}

impl Default for RebalancerDaemonConfig {
    fn default() -> Self {
        Self {
            scan_interval: Duration::from_secs(60),
            replication_factor: 3,
            repair_parallelism: 4,
            rate_limit_gb: 10,
            dry_run: false,
        }
    }
}

impl RebalancerDaemonConfig {
    /// Create configuration from environment variables
    pub fn from_env() -> Self {
        Self {
            scan_interval: Duration::from_secs(
                std::env::var("REBALANCER_SCAN_INTERVAL_SECS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(60),
            ),
            replication_factor: std::env::var("REBALANCER_REPLICATION_FACTOR")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(3),
            repair_parallelism: std::env::var("REBALANCER_PARALLELISM")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(4),
            rate_limit_gb: std::env::var("REBALANCER_RATE_LIMIT_GB")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(10),
            dry_run: std::env::var("REBALANCER_DRY_RUN")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(false),
        }
    }
}

/// Rebalancer daemon for automatic chunk replication
pub struct RebalancerDaemon {
    config: RebalancerDaemonConfig,
}

impl RebalancerDaemon {
    /// Create a new rebalancer daemon
    pub fn new(config: RebalancerDaemonConfig) -> Self {
        Self { config }
    }

    /// Start the rebalancer daemon as a background task
    pub fn start(self: Arc<Self>, state: Arc<AppState>) -> JoinHandle<()> {
        let config = self.config.clone();

        tokio::spawn(async move {
            // Get database from metadata service
            let db = match state.metadata_service() {
                Some(meta) => meta.database_arc(),
                None => {
                    warn!("Rebalancer daemon disabled: no metadata service configured");
                    return;
                }
            };

            info!(
                scan_interval = ?config.scan_interval,
                replication_factor = config.replication_factor,
                parallelism = config.repair_parallelism,
                dry_run = config.dry_run,
                "Starting rebalancer daemon"
            );

            // Create rebalancer components
            let metadata_client = Arc::new(PostgresMetadataClient::new(db.clone()));
            let network_client = Arc::new(GrpcNetworkClient::new(db.clone()));

            let detector_config = DetectorConfig {
                replication_factor: config.replication_factor,
                batch_size: 1000,
                scan_interval: config.scan_interval,
                verify_integrity: false,
                health_check_timeout: Duration::from_secs(5),
            };

            let planner_config = PlannerConfig {
                replication_factor: config.replication_factor,
                max_tasks: 100,
                max_bytes: config.rate_limit_gb * 1024 * 1024 * 1024,
                prefer_local: true,
                max_node_load: 0.8,
                node_rate_limit: 100 * 1024 * 1024,
            };

            let executor_config = ExecutorConfig {
                max_concurrent: config.repair_parallelism,
                max_per_source: 3,
                max_per_target: 3,
                transfer_timeout: Duration::from_secs(300),
                max_retries: 3,
                retry_delay: Duration::from_secs(5),
                node_rate_limit: 100 * 1024 * 1024,
                report_progress: false,
            };

            let mut detector = Detector::new(detector_config);
            let mut planner = Planner::new(planner_config);
            let (executor, _progress_rx) = Executor::with_progress(executor_config);

            // Main loop
            loop {
                if detector.should_scan() {
                    if let Err(e) = run_scan_cycle(
                        &mut detector,
                        &mut planner,
                        &executor,
                        &metadata_client,
                        &network_client,
                        &db,
                        config.dry_run,
                    )
                    .await
                    {
                        error!(error = %e, "Rebalancer scan cycle failed");
                    }
                }

                // Wait before next check
                tokio::time::sleep(Duration::from_secs(10)).await;
            }
        })
    }
}

/// Run a single scan and repair cycle
async fn run_scan_cycle(
    detector: &mut Detector,
    planner: &mut Planner,
    executor: &Executor,
    metadata_client: &Arc<PostgresMetadataClient>,
    network_client: &Arc<GrpcNetworkClient>,
    db: &Arc<Database>,
    dry_run: bool,
) -> anyhow::Result<()> {
    debug!("Starting rebalancer scan cycle");

    // Step 1: Detect issues
    let scan_result = detector
        .scan(metadata_client.as_ref(), network_client.as_ref())
        .await
        .map_err(|e| anyhow::anyhow!("Scan failed: {}", e))?;

    debug!(summary = %scan_result.summary(), "Scan complete");

    if scan_result.has_critical_issues() {
        warn!("Critical replication issues detected!");
    }

    // Step 2: Create repair plan if there are issues
    let all_issues = scan_result.all_issues();
    if all_issues.is_empty() {
        debug!("No replication issues found");
        return Ok(());
    }

    info!(
        under_replicated = scan_result.under_replicated.len(),
        "Found chunks needing repair"
    );

    // Get node info
    let nodes = network_client
        .get_node_info()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to get nodes: {}", e))?;

    let issues: Vec<_> = all_issues.into_iter().cloned().collect();
    let plan = planner
        .create_plan(&issues, &nodes)
        .map_err(|e| anyhow::anyhow!("Planning failed: {}", e))?;

    info!(summary = %plan.summary(), "Repair plan created");

    if dry_run {
        info!("Dry run mode, skipping execution");
        return Ok(());
    }

    // Step 3: Execute repairs
    let transfer_fn = cyxcloud_rebalancer::transfer::create_transfer_fn(db.clone());
    let result = executor.execute(plan, transfer_fn).await;

    info!(summary = %result.summary(), "Repair execution complete");

    Ok(())
}
