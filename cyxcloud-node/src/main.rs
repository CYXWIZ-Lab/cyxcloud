//! CyxCloud Storage Node Daemon
//!
//! Runs a storage node that:
//! - Stores chunk data locally using RocksDB
//! - Participates in P2P network for peer discovery
//! - Serves chunk requests via gRPC
//! - Reports health metrics via Prometheus endpoint

use clap::Parser;
use cyxcloud_node::{
    init_metrics, HealthChecker, HealthState, HeartbeatService, MachineService, MetricsServer,
    NodeConfig, NodeMetrics,
};
use cyxcloud_storage::RocksDbBackend;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

#[cfg(feature = "blockchain")]
use cyxcloud_node::{
    blockchain::{heartbeat::BlockchainHeartbeatService, NodeBlockchainConfig},
    DiskType, StorageNodeBlockchainClient, StorageSpec,
};

#[cfg(feature = "blockchain")]
use solana_sdk::signer::Signer;

#[derive(Parser)]
#[command(name = "cyxcloud-node")]
#[command(about = "CyxCloud storage node daemon")]
#[command(version)]
struct Cli {
    /// Configuration file path
    #[arg(short, long, default_value = "config.toml")]
    config: PathBuf,

    /// Storage directory (overrides config file)
    #[arg(short, long)]
    data_dir: Option<PathBuf>,

    /// gRPC listen port (overrides config file)
    #[arg(short, long)]
    port: Option<u16>,

    /// Metrics HTTP port (overrides config file)
    #[arg(short, long)]
    metrics_port: Option<u16>,

    /// Enable verbose logging
    #[arg(short, long)]
    verbose: bool,

    /// Logout and clear saved credentials
    #[arg(long)]
    logout: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Install rustls crypto provider (required for rustls 0.23+)
    // This must be done before any TLS operations
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

    // Parse CLI arguments
    let cli = Cli::parse();

    // Handle logout command
    if cli.logout {
        let creds_path = cyxcloud_node::config::node_credentials_path();
        if creds_path.exists() {
            std::fs::remove_file(&creds_path)?;
            println!(
                "Logged out successfully. Credentials removed from {:?}",
                creds_path
            );
        } else {
            println!("No saved credentials found.");
        }
        return Ok(());
    }

    // Initialize tracing
    if cli.verbose {
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .init();
    }

    info!("CyxCloud Storage Node starting...");

    // Load configuration
    // Priority: CLI args > node config.toml > shared config (~/.cyxcloud/config.toml) > defaults
    let mut config = NodeConfig::load_or_default(&cli.config)
        .with_shared_config() // Apply shared config from ~/.cyxcloud/config.toml
        .with_overrides(cli.data_dir, cli.port)
        .with_env_overrides();

    // Check if storage capacity is configured
    if config.storage.max_capacity_gb == 0 {
        // Check if running in interactive mode (TTY)
        if atty::is(atty::Stream::Stdin) {
            use cyxcloud_node::symbols;
            use std::io::{self, Write};

            println!();
            println!("{}", symbols::BOX_TOP);
            println!(
                "{}  Storage Capacity Not Configured                        {}",
                symbols::BOX_SIDE,
                symbols::BOX_SIDE
            );
            println!("{}", symbols::BOX_BOTTOM);
            println!();
            println!("Your storage capacity (max_capacity_gb) is not set in config.toml.");
            println!("This determines how much disk space you're offering to the network.");
            println!();
            println!("Please enter the storage capacity you want to allocate:");
            println!();

            loop {
                print!("Storage capacity in GB (e.g., 100): ");
                io::stdout().flush()?;

                let mut input = String::new();
                io::stdin().read_line(&mut input)?;
                let input = input.trim();

                match input.parse::<u64>() {
                    Ok(gb) if gb >= 1 => {
                        config.storage.max_capacity_gb = gb;
                        println!();
                        println!("{} Storage capacity set to {} GB", symbols::CHECK, gb);

                        // Calculate reserved and available
                        let reserved_gb = 2;
                        let available_gb = if gb > reserved_gb {
                            gb - reserved_gb
                        } else {
                            0
                        };
                        println!("  System reserved: {} GB", reserved_gb);
                        println!("  Available for storage: {} GB", available_gb);
                        println!();

                        // Suggest saving to config
                        println!("TIP: Add this to your config.toml to skip this prompt:");
                        println!();
                        println!("  [storage]");
                        println!("  max_capacity_gb = {}", gb);
                        println!();
                        break;
                    }
                    Ok(_) => {
                        println!("{} Please enter at least 1 GB", symbols::CROSS);
                    }
                    Err(_) => {
                        println!(
                            "{} Invalid input. Please enter a number (e.g., 100)",
                            symbols::CROSS
                        );
                    }
                }
            }
        } else {
            // Non-interactive mode, warn but continue with 0 (unlimited)
            warn!("Storage capacity not configured (max_capacity_gb = 0)");
            warn!("Set max_capacity_gb in config.toml or run interactively to configure");
        }
    }

    info!(
        node_id = %config.node.id,
        node_name = %config.node.name,
        grpc_port = config.network.grpc_port,
        data_dir = ?config.storage.data_dir,
        max_capacity_gb = config.storage.max_capacity_gb,
        "Configuration loaded"
    );

    // Validate configuration
    if let Err(e) = config.validate() {
        error!(error = %e, "Configuration validation failed");
        return Err(e.into());
    }

    // Initialize Prometheus metrics
    init_metrics();

    // Initialize storage backend
    info!("Initializing storage backend...");
    let storage_config = config.storage.to_storage_config();
    let storage = match RocksDbBackend::open(storage_config) {
        Ok(backend) => Arc::new(backend),
        Err(e) => {
            error!(error = %e, "Failed to initialize storage backend");
            return Err(anyhow::anyhow!("Storage initialization failed: {}", e));
        }
    };
    info!("Storage backend initialized");

    // Create shared state
    let health_state = Arc::new(RwLock::new(HealthState::default()));
    let node_metrics = NodeMetrics::new(&config.node.id);

    // Start metrics HTTP server
    let metrics_port = cli.metrics_port.unwrap_or(config.metrics.port);
    if config.metrics.enabled {
        let metrics_server = MetricsServer::new(metrics_port)
            .map_err(|e| anyhow::anyhow!("Failed to create metrics server: {}", e))?;
        let health_path = config.metrics.health_path.clone();
        let metrics_path = config.metrics.metrics_path.clone();
        let health_state_clone = health_state.clone();

        tokio::spawn(async move {
            if let Err(e) = metrics_server
                .start(health_path, metrics_path, health_state_clone)
                .await
            {
                error!(error = %e, "Metrics server failed");
            }
        });

        info!(port = metrics_port, "Metrics server started");
    }

    // Start health checker
    let health_checker = HealthChecker::new(
        config.node.id.clone(),
        storage.clone(),
        node_metrics.clone(),
        health_state.clone(),
    );

    tokio::spawn(async move {
        health_checker.run().await;
    });
    info!("Health checker started");

    // ========================================
    // Authentication Flow:
    // 1. Login to CyxWiz API (port 3002) to get JWT token
    // 2. Use JWT token for Gateway authentication (port 50052)
    // ========================================

    // Create machine service for CyxWiz API authentication
    let machine_service = Arc::new(
        MachineService::new(config.clone(), node_metrics.clone())
            .map_err(|e| anyhow::anyhow!("Failed to create machine service: {}", e))?,
    );

    // Login to CyxWiz API first (blocks until successful login)
    let already_registered = if config.cyxwiz_api.register {
        info!(api_url = %config.cyxwiz_api.base_url, "Authenticating with CyxWiz API...");
        match machine_service.ensure_logged_in().await {
            Ok(registered) => {
                info!("CyxWiz API authentication successful");
                registered
            }
            Err(e) => {
                error!(error = %e, "Failed to authenticate with CyxWiz API");
                return Err(anyhow::anyhow!("Authentication failed: {}", e));
            }
        }
    } else {
        info!("CyxWiz API registration disabled");
        false
    };

    // Get JWT token for Gateway authentication
    let jwt_token = machine_service.get_jwt_token().await;
    if jwt_token.is_none() && config.central.register {
        error!("No JWT token available for Gateway authentication");
        return Err(anyhow::anyhow!(
            "JWT token required for Gateway registration"
        ));
    }

    // Create heartbeat service for Gateway registration
    let heartbeat_service = Arc::new(HeartbeatService::new(
        config.clone(),
        node_metrics.clone(),
        storage.clone(),
    ));

    // Set JWT token for Gateway authentication
    if let Some(token) = jwt_token {
        heartbeat_service.set_jwt_token(token).await;
    }

    // Set wallet address for Gateway registration (from CyxWiz login)
    if let Some(wallet) = machine_service.get_credentials_wallet().await {
        heartbeat_service.set_credentials_wallet(wallet).await;
    }

    // Start Gateway heartbeat service
    if config.central.register {
        let heartbeat_clone = heartbeat_service.clone();
        tokio::spawn(async move {
            heartbeat_clone.run().await;
        });
        info!(
            central_addr = %config.central.address,
            "Gateway heartbeat service started (with JWT auth)"
        );
    }

    // Start CyxWiz API machine service (for heartbeats to CyxWiz API)
    if config.cyxwiz_api.register {
        let machine_service_clone = machine_service.clone();
        tokio::spawn(async move {
            machine_service_clone.run(already_registered).await;
        });
        info!(
            api_url = %config.cyxwiz_api.base_url,
            "CyxWiz API machine service started"
        );
    }

    // Start blockchain service (optional, enabled via feature and config)
    #[cfg(feature = "blockchain")]
    {
        if config.blockchain.enabled {
            match initialize_blockchain_service(&config).await {
                Ok(Some((client, heartbeat_handle))) => {
                    info!(
                        rpc_url = %config.blockchain.rpc_url,
                        heartbeat = config.blockchain.heartbeat_enabled,
                        "Blockchain service started"
                    );
                    // Store client and handle for later use
                    // The heartbeat service runs in the background
                    let _ = (client, heartbeat_handle);
                }
                Ok(None) => {
                    warn!("Blockchain service enabled but no keypair configured");
                }
                Err(e) => {
                    error!(error = %e, "Failed to initialize blockchain service");
                }
            }
        }
    }

    // Start gRPC server for chunk operations
    let grpc_addr = config.network.grpc_addr();
    info!(addr = %grpc_addr, "Starting gRPC server...");

    let grpc_server = start_grpc_server(grpc_addr, storage.clone(), config.node.id.clone());

    // Print startup summary
    info!("========================================");
    info!("  CyxCloud Storage Node Running");
    info!("========================================");
    info!("  Node ID:     {}", config.node.id);
    info!("  Node Name:   {}", config.node.name);
    info!("  gRPC:        {}", grpc_addr);
    if config.metrics.enabled {
        info!(
            "  Metrics:     http://{}:{}{}",
            config.network.bind_address, metrics_port, config.metrics.metrics_path
        );
        info!(
            "  Health:      http://{}:{}{}",
            config.network.bind_address, metrics_port, config.metrics.health_path
        );
    }
    if let Some(ref region) = config.node.region {
        info!("  Region:      {}", region);
    }
    if let Some(ref wallet) = config.node.wallet_address {
        info!("  Wallet:      {}...", &wallet[..wallet.len().min(16)]);
    }
    if config.cyxwiz_api.register {
        info!("  CyxWiz API:  {}", config.cyxwiz_api.base_url);
        if let Some(ref owner_id) = config.cyxwiz_api.owner_id {
            info!("  Owner ID:    {}...", &owner_id[..owner_id.len().min(16)]);
        }
    }
    info!("========================================");
    info!("Press Ctrl+C to shut down");

    // Wait for shutdown signal
    tokio::select! {
        result = grpc_server => {
            if let Err(e) = result {
                error!(error = %e, "gRPC server error");
            }
        }
        _ = tokio::signal::ctrl_c() => {
            info!("Received shutdown signal");
        }
    }

    // Graceful shutdown
    info!("Shutting down...");

    // Give services time to finish
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    info!("CyxCloud node stopped");
    Ok(())
}

/// Start the gRPC server for chunk operations
async fn start_grpc_server(
    addr: std::net::SocketAddr,
    storage: Arc<RocksDbBackend>,
    node_id: String,
) -> anyhow::Result<()> {
    use cyxcloud_network::grpc_server::ChunkServiceImpl;
    use cyxcloud_protocol::ChunkServiceServer;
    use tonic::transport::Server;

    let chunk_service = ChunkServiceImpl::new(storage, node_id);

    Server::builder()
        .add_service(ChunkServiceServer::new(chunk_service))
        .serve(addr)
        .await?;

    Ok(())
}

/// Initialize blockchain service for Solana integration
#[cfg(feature = "blockchain")]
async fn initialize_blockchain_service(
    config: &NodeConfig,
) -> anyhow::Result<
    Option<(
        Arc<StorageNodeBlockchainClient>,
        tokio::task::JoinHandle<()>,
    )>,
> {
    use cyxcloud_node::blockchain::heartbeat::HeartbeatOps;
    use solana_sdk::signature::Keypair;
    use std::path::Path;

    // Check if keypair is configured
    let keypair_path = match &config.blockchain.keypair_path {
        Some(path) => path.clone(),
        None => {
            // Try default path
            let default_path = std::env::var("HOME")
                .or_else(|_| std::env::var("USERPROFILE"))
                .map(|home| format!("{}/.config/solana/id.json", home))
                .unwrap_or_default();

            if !Path::new(&default_path).exists() {
                return Ok(None);
            }
            default_path
        }
    };

    // Load keypair
    let keypair_json = std::fs::read_to_string(&keypair_path)?;
    let keypair_bytes: Vec<u8> = serde_json::from_str(&keypair_json)?;
    #[allow(deprecated)]
    let owner = Keypair::from_bytes(&keypair_bytes)
        .map_err(|e| anyhow::anyhow!("Failed to parse keypair: {}", e))?;

    info!(
        pubkey = %owner.pubkey(),
        keypair_path = %keypair_path,
        "Loaded blockchain keypair"
    );

    // Create blockchain config
    let blockchain_config = NodeBlockchainConfig {
        rpc_url: config.blockchain.rpc_url.clone(),
        keypair_path: Some(keypair_path.clone()),
        ..NodeBlockchainConfig::default()
    };

    // Create client
    let client = Arc::new(StorageNodeBlockchainClient::new(blockchain_config, owner)?);

    // Check if node is registered
    let is_registered = client.is_registered(&config.node.id).await?;

    if !is_registered {
        info!(
            node_id = %config.node.id,
            "Node not registered on-chain, registering..."
        );

        // Create storage spec from config
        let spec = StorageSpec::new(
            config.storage.max_capacity_gb * 1024 * 1024 * 1024,
            DiskType::SSD, // Default to SSD, could be configurable
            config
                .node
                .region
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
            1000, // Default bandwidth, could be configurable
        );

        // Register node
        match client.register_node(&config.node.id, &spec).await {
            Ok(sig) => {
                info!(signature = %sig, "Node registered on blockchain");
            }
            Err(e) => {
                warn!(error = %e, "Failed to register node on blockchain");
            }
        }
    } else {
        info!(node_id = %config.node.id, "Node already registered on blockchain");
    }

    // Start blockchain heartbeat service if enabled
    let heartbeat_handle = if config.blockchain.heartbeat_enabled {
        let node_registry_program_id = client.config().node_registry_program_id;
        let rpc_client = Arc::new(solana_client::rpc_client::RpcClient::new(
            config.blockchain.rpc_url.clone(),
        ));

        let heartbeat_ops = HeartbeatOps::new(rpc_client, node_registry_program_id);

        let owner_keypair = {
            let json = std::fs::read_to_string(&keypair_path)?;
            let bytes: Vec<u8> = serde_json::from_str(&json)?;
            #[allow(deprecated)]
            Keypair::from_bytes(&bytes).map_err(|e| anyhow::anyhow!("Keypair error: {}", e))?
        };

        let heartbeat_service = BlockchainHeartbeatService::new(
            heartbeat_ops,
            Arc::new(owner_keypair),
            config.node.id.clone(),
            config.blockchain.heartbeat_interval_secs,
        );

        let handle = tokio::spawn(async move {
            heartbeat_service.run().await;
        });

        info!(
            interval_secs = config.blockchain.heartbeat_interval_secs,
            "Blockchain heartbeat service started"
        );

        handle
    } else {
        // Return a dummy handle that completes immediately
        tokio::spawn(async {})
    };

    Ok(Some((client, heartbeat_handle)))
}
