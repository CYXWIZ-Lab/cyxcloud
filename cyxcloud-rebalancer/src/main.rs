//! CyxCloud Rebalancer Service
//!
//! Monitors cluster health and performs:
//! - Failure detection
//! - Data repair (reconstruct lost chunks)
//! - Rebalancing (distribute data evenly)
//! - Hot-swap support (drain nodes before shutdown)

mod detector;
mod executor;
mod planner;

use clap::Parser;
use std::time::Duration;
use tokio::signal;
use tokio::sync::mpsc;
use tracing::{error, info, warn, Level};

use detector::{Detector, DetectorConfig};
use executor::{Executor, ExecutorConfig, ProgressUpdate};
use planner::{NodeInfo, Planner, PlannerConfig};

#[derive(Parser)]
#[command(name = "cyxcloud-rebalancer")]
#[command(about = "CyxCloud data rebalancing service")]
struct Cli {
    /// Scan interval in seconds
    #[arg(long, default_value = "60")]
    scan_interval: u64,

    /// Repair parallelism
    #[arg(long, default_value = "4")]
    parallelism: usize,

    /// Maximum bytes to repair per hour (GB)
    #[arg(long, default_value = "10")]
    rate_limit_gb: u64,

    /// Target replication factor
    #[arg(long, default_value = "3")]
    replication_factor: usize,

    /// Metadata service address
    #[arg(long, default_value = "http://localhost:50051")]
    metadata_addr: String,

    /// Dry run mode (don't actually repair)
    #[arg(long, default_value = "false")]
    dry_run: bool,
}

struct RebalancerService {
    detector: Detector,
    planner: Planner,
    executor: Executor,
    metadata_addr: String,
    dry_run: bool,
    scan_interval: Duration,
}

impl RebalancerService {
    fn new(cli: &Cli) -> (Self, mpsc::Receiver<ProgressUpdate>) {
        let detector_config = DetectorConfig {
            replication_factor: cli.replication_factor,
            batch_size: 1000,
            scan_interval: Duration::from_secs(cli.scan_interval),
            verify_integrity: false,
            health_check_timeout: Duration::from_secs(5),
        };

        let planner_config = PlannerConfig {
            replication_factor: cli.replication_factor,
            max_tasks: 100,
            max_bytes: cli.rate_limit_gb * 1024 * 1024 * 1024,
            prefer_local: true,
            max_node_load: 0.8,
            node_rate_limit: 100 * 1024 * 1024,
        };

        let executor_config = ExecutorConfig {
            max_concurrent: cli.parallelism,
            max_per_source: 3,
            max_per_target: 3,
            transfer_timeout: Duration::from_secs(300),
            max_retries: 3,
            retry_delay: Duration::from_secs(5),
            node_rate_limit: 100 * 1024 * 1024,
            report_progress: true,
        };

        let (executor, progress_rx) = Executor::with_progress(executor_config);

        let service = Self {
            detector: Detector::new(detector_config),
            planner: Planner::new(planner_config),
            executor,
            metadata_addr: cli.metadata_addr.clone(),
            dry_run: cli.dry_run,
            scan_interval: Duration::from_secs(cli.scan_interval),
        };

        (service, progress_rx)
    }

    async fn run(&mut self) -> anyhow::Result<()> {
        info!(
            scan_interval = ?self.scan_interval,
            dry_run = self.dry_run,
            "Rebalancer service started"
        );

        loop {
            // Check if we should scan
            if self.detector.should_scan() {
                if let Err(e) = self.run_scan_cycle().await {
                    error!(error = %e, "Scan cycle failed");
                }
            }

            // Wait for next cycle
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(10)) => {},
                _ = signal::ctrl_c() => {
                    info!("Shutdown signal received");
                    break;
                }
            }
        }

        // Graceful shutdown
        self.executor.shutdown().await;
        info!("Rebalancer service stopped");

        Ok(())
    }

    async fn run_scan_cycle(&mut self) -> anyhow::Result<()> {
        info!("Starting scan cycle");

        // Create mock clients for now
        // TODO: Replace with real metadata and network clients
        let metadata_client = MockMetadataClient::new(&self.metadata_addr);
        let network_client = MockNetworkClient::new();

        // Step 1: Detect issues
        let scan_result = self
            .detector
            .scan(&metadata_client, &network_client)
            .await
            .map_err(|e| anyhow::anyhow!("Scan failed: {}", e))?;

        info!(summary = %scan_result.summary(), "Scan complete");

        if scan_result.has_critical_issues() {
            warn!("Critical issues detected!");
        }

        // Step 2: Create repair plan
        let all_issues = scan_result.all_issues();
        if all_issues.is_empty() {
            info!("No issues found, skipping repair");
            return Ok(());
        }

        // Get node info
        let nodes = network_client
            .get_node_info()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get nodes: {}", e))?;

        let issues: Vec<_> = all_issues.into_iter().cloned().collect();
        let plan = self
            .planner
            .create_plan(&issues, &nodes)
            .map_err(|e| anyhow::anyhow!("Planning failed: {}", e))?;

        info!(summary = %plan.summary(), "Repair plan created");

        if self.dry_run {
            info!("Dry run mode, skipping execution");
            return Ok(());
        }

        // Step 3: Execute repairs
        let result = self
            .executor
            .execute(plan, |source, _task_id, chunk_id, targets| async move {
                // TODO: Replace with real network transfer
                info!(
                    source = source,
                    chunk = hex::encode(&chunk_id),
                    targets = ?targets,
                    "Would transfer chunk"
                );
                // Simulate success
                Ok(targets)
            })
            .await;

        info!(summary = %result.summary(), "Repair execution complete");

        Ok(())
    }
}

// =============================================================================
// MOCK CLIENTS (for development/testing)
// =============================================================================

struct MockMetadataClient {
    _addr: String,
}

impl MockMetadataClient {
    fn new(addr: &str) -> Self {
        Self {
            _addr: addr.to_string(),
        }
    }
}

#[async_trait::async_trait]
impl detector::MetadataClient for MockMetadataClient {
    async fn get_under_replicated_chunks(
        &self,
        _limit: usize,
    ) -> Result<Vec<detector::ChunkInfo>, Box<dyn std::error::Error + Send + Sync>> {
        // Return empty for now
        Ok(Vec::new())
    }

    async fn get_orphaned_chunks(
        &self,
        _limit: usize,
    ) -> Result<Vec<detector::ChunkInfo>, Box<dyn std::error::Error + Send + Sync>> {
        Ok(Vec::new())
    }
}

struct MockNetworkClient;

impl MockNetworkClient {
    fn new() -> Self {
        Self
    }

    async fn get_node_info(
        &self,
    ) -> Result<Vec<NodeInfo>, Box<dyn std::error::Error + Send + Sync>> {
        // Return mock nodes
        Ok(vec![
            NodeInfo {
                id: "node1".to_string(),
                address: "localhost:50051".to_string(),
                available_storage: 100 * 1024 * 1024 * 1024,
                load: 0.3,
                datacenter: Some("dc1".to_string()),
                is_healthy: true,
            },
            NodeInfo {
                id: "node2".to_string(),
                address: "localhost:50052".to_string(),
                available_storage: 100 * 1024 * 1024 * 1024,
                load: 0.4,
                datacenter: Some("dc1".to_string()),
                is_healthy: true,
            },
            NodeInfo {
                id: "node3".to_string(),
                address: "localhost:50053".to_string(),
                available_storage: 100 * 1024 * 1024 * 1024,
                load: 0.5,
                datacenter: Some("dc2".to_string()),
                is_healthy: true,
            },
        ])
    }
}

#[async_trait::async_trait]
impl detector::NetworkClient for MockNetworkClient {
    async fn get_all_nodes(
        &self,
    ) -> Result<Vec<String>, Box<dyn std::error::Error + Send + Sync>> {
        Ok(vec![
            "node1".to_string(),
            "node2".to_string(),
            "node3".to_string(),
        ])
    }

    async fn check_node_health(
        &self,
        _node_id: &str,
        _timeout: Duration,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        Ok(true)
    }

    async fn verify_chunk_integrity(
        &self,
        _node_id: &str,
        _chunk_id: &[u8],
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        Ok(true)
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_target(true)
        .init();

    let cli = Cli::parse();

    info!(
        scan_interval = cli.scan_interval,
        parallelism = cli.parallelism,
        rate_limit_gb = cli.rate_limit_gb,
        replication_factor = cli.replication_factor,
        dry_run = cli.dry_run,
        "Starting CyxCloud rebalancer"
    );

    let (mut service, mut progress_rx) = RebalancerService::new(&cli);

    // Spawn progress reporter
    tokio::spawn(async move {
        while let Some(update) = progress_rx.recv().await {
            info!(
                task_id = update.task_id,
                percent = update.percent,
                status = ?update.status,
                "Repair progress"
            );
        }
    });

    // Run service
    service.run().await?;

    Ok(())
}
