//! CyxCloud Rebalancer Service
//!
//! Monitors cluster health and performs:
//! - Failure detection
//! - Data repair (reconstruct lost chunks)
//! - Rebalancing (distribute data evenly)
//! - Hot-swap support (drain nodes before shutdown)

#![allow(dead_code)]

mod config;
mod detector;
mod executor;
mod metadata_client;
mod network_client;
mod planner;
mod transfer;

use clap::Parser;
use std::sync::Arc;
use std::time::Duration;
use tokio::signal;
use tokio::sync::mpsc;
use tracing::{error, info, warn, Level};
use detector::{Detector, DetectorConfig};
use executor::{Executor, ExecutorConfig, ProgressUpdate};
use metadata_client::PostgresMetadataClient;
use network_client::GrpcNetworkClient;
use planner::{NodeInfo, Planner, PlannerConfig};
use transfer::create_transfer_fn;

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

    /// PostgreSQL database URL (enables production mode)
    #[arg(long, env = "DATABASE_URL")]
    database_url: Option<String>,

    /// Dry run mode (don't actually repair)
    #[arg(long, default_value = "false")]
    dry_run: bool,
}

/// Client mode for the rebalancer
enum ClientMode {
    /// Use mock clients for development/testing
    Mock,
    /// Use real clients connected to PostgreSQL
    Production {
        db: Arc<cyxcloud_metadata::postgres::Database>,
        metadata_client: Arc<PostgresMetadataClient>,
        network_client: Arc<GrpcNetworkClient>,
    },
}

struct RebalancerService {
    detector: Detector,
    planner: Planner,
    executor: Executor,
    client_mode: ClientMode,
    dry_run: bool,
    scan_interval: Duration,
}

impl RebalancerService {
    async fn new(cli: &Cli) -> anyhow::Result<(Self, mpsc::Receiver<ProgressUpdate>)> {
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

        // Determine client mode based on database URL
        let client_mode = if let Some(ref db_url) = cli.database_url {
            info!("Production mode: connecting to PostgreSQL");

            let db_config = cyxcloud_metadata::postgres::DbConfig {
                url: db_url.clone(),
                ..Default::default()
            };

            let db = Arc::new(
                cyxcloud_metadata::postgres::Database::new(db_config)
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to connect to database: {}", e))?,
            );

            let metadata_client = Arc::new(PostgresMetadataClient::new(db.clone()));
            let network_client = Arc::new(GrpcNetworkClient::new(db.clone()));

            info!("Connected to PostgreSQL database");

            ClientMode::Production {
                db,
                metadata_client,
                network_client,
            }
        } else {
            info!("Development mode: using mock clients");
            ClientMode::Mock
        };

        let service = Self {
            detector: Detector::new(detector_config),
            planner: Planner::new(planner_config),
            executor,
            client_mode,
            dry_run: cli.dry_run,
            scan_interval: Duration::from_secs(cli.scan_interval),
        };

        Ok((service, progress_rx))
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

        match &self.client_mode {
            ClientMode::Production {
                db,
                metadata_client,
                network_client,
            } => {
                self.run_production_cycle(
                    db.clone(),
                    metadata_client.clone(),
                    network_client.clone(),
                )
                .await
            }
            ClientMode::Mock => self.run_mock_cycle().await,
        }
    }

    async fn run_production_cycle(
        &mut self,
        db: Arc<cyxcloud_metadata::postgres::Database>,
        metadata_client: Arc<PostgresMetadataClient>,
        network_client: Arc<GrpcNetworkClient>,
    ) -> anyhow::Result<()> {
        // Step 1: Detect issues
        let scan_result = self
            .detector
            .scan(metadata_client.as_ref(), network_client.as_ref())
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

        // Get node info from network client
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

        // Step 3: Execute repairs with real transfer function
        let transfer_fn = create_transfer_fn(db);
        let result = self.executor.execute(plan, transfer_fn).await;

        info!(summary = %result.summary(), "Repair execution complete");

        Ok(())
    }

    async fn run_mock_cycle(&mut self) -> anyhow::Result<()> {
        // Use mock clients for development/testing
        let metadata_client = MockMetadataClient::new();
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

        // Step 3: Execute repairs (mock - just log)
        let result = self
            .executor
            .execute(plan, |source, _task_id, chunk_id, targets| async move {
                info!(
                    source = source,
                    chunk = hex::encode(&chunk_id),
                    targets = ?targets,
                    "Would transfer chunk (mock mode)"
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

struct MockMetadataClient;

impl MockMetadataClient {
    fn new() -> Self {
        Self
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
    async fn get_all_nodes(&self) -> Result<Vec<String>, Box<dyn std::error::Error + Send + Sync>> {
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

    let mode = if cli.database_url.is_some() {
        "production"
    } else {
        "development (mock)"
    };

    info!(
        scan_interval = cli.scan_interval,
        parallelism = cli.parallelism,
        rate_limit_gb = cli.rate_limit_gb,
        replication_factor = cli.replication_factor,
        dry_run = cli.dry_run,
        mode = mode,
        "Starting CyxCloud rebalancer"
    );

    let (mut service, mut progress_rx) = RebalancerService::new(&cli).await?;

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
