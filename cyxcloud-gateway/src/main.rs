//! CyxCloud API Gateway
//!
//! Provides:
//! - S3-compatible REST API (HTTP/HTTPS)
//! - gRPC API for ecosystem integration (NodeService, DataService)
//! - WebSocket for real-time sync
//! - Authentication (JWT, wallet signatures)
//! - mTLS support for secure node communication

pub mod auth;
mod auth_api;
#[cfg(feature = "blockchain")]
pub mod blockchain;
mod grpc_api;
mod node_client;
mod node_monitor;
mod payment_daemon;
mod rebalancer_daemon;
mod s3_api;
mod state;
mod websocket;

pub use auth::{AuthConfig, AuthService};
#[cfg(feature = "blockchain")]
pub use blockchain::{BlockchainConfig, CyxCloudBlockchainClient};
pub use state::{AppState, GatewayConfig};

use axum::{routing::get, Router};
use clap::Parser;
use cyxcloud_core::tls::{TlsServerConfig, create_tonic_server_tls};
use cyxcloud_protocol::data::data_service_server::DataServiceServer;
use cyxcloud_protocol::node::node_service_server::NodeServiceServer;
use grpc_api::{AuthInterceptor, DataServiceImpl, NodeServiceImpl};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::signal;
use tonic::transport::Server as TonicServer;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing::{error, info, warn, Level};

#[derive(Parser)]
#[command(name = "cyxcloud-gateway")]
#[command(about = "CyxCloud API gateway")]
struct Cli {
    /// HTTP listen address
    #[arg(long, default_value = "0.0.0.0:8180")]
    http_addr: String,

    /// gRPC listen address
    #[arg(long, default_value = "0.0.0.0:50052")]
    grpc_addr: String,

    /// Enable CORS for all origins (development only)
    #[arg(long, default_value = "false")]
    cors_permissive: bool,

    /// PostgreSQL database URL (enables persistent storage)
    #[arg(long, env = "DATABASE_URL")]
    database_url: Option<String>,

    /// Redis URL for caching
    #[arg(long, env = "REDIS_URL")]
    redis_url: Option<String>,

    /// Force in-memory storage even if database is configured
    #[arg(long, default_value = "false")]
    memory_only: bool,

    /// Enable gRPC authentication (requires JWT)
    #[arg(long, default_value = "false")]
    grpc_auth: bool,

    // ===== TLS Configuration =====
    /// Path to server TLS certificate (PEM format)
    #[arg(long, env = "TLS_CERT")]
    tls_cert: Option<PathBuf>,

    /// Path to server TLS private key (PEM format)
    #[arg(long, env = "TLS_KEY")]
    tls_key: Option<PathBuf>,

    /// Path to CA certificate for client verification (mTLS)
    #[arg(long, env = "TLS_CA_CERT")]
    tls_ca_cert: Option<PathBuf>,

    /// Require client certificates for mTLS (node authentication)
    #[arg(long, default_value = "false", env = "TLS_REQUIRE_CLIENT_CERT")]
    tls_require_client_cert: bool,

    /// Solana RPC URL for blockchain operations (requires 'blockchain' feature)
    #[cfg(feature = "blockchain")]
    #[arg(long, env = "SOLANA_RPC_URL", default_value = "https://api.devnet.solana.com")]
    solana_rpc_url: String,

    /// Path to gateway authority keypair file (requires 'blockchain' feature)
    #[cfg(feature = "blockchain")]
    #[arg(long, env = "GATEWAY_KEYPAIR_PATH")]
    keypair_path: Option<String>,

    /// Enable blockchain integration (requires 'blockchain' feature)
    #[cfg(feature = "blockchain")]
    #[arg(long, default_value = "false")]
    enable_blockchain: bool,
}

async fn health() -> &'static str {
    "OK"
}

async fn version() -> &'static str {
    concat!("cyxcloud-gateway/", env!("CARGO_PKG_VERSION"))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Install rustls crypto provider (required for rustls 0.23+)
    // This must be done before any TLS operations
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

    // Initialize tracing
    tracing_subscriber::fmt()
        .with_max_level(Level::DEBUG)
        .with_target(true)
        .init();

    let cli = Cli::parse();

    // Check TLS configuration
    let tls_enabled = cli.tls_cert.is_some() && cli.tls_key.is_some();
    if cli.tls_cert.is_some() != cli.tls_key.is_some() {
        anyhow::bail!("Both --tls-cert and --tls-key must be provided for TLS");
    }
    if cli.tls_require_client_cert && cli.tls_ca_cert.is_none() {
        anyhow::bail!("--tls-ca-cert is required when --tls-require-client-cert is enabled");
    }

    info!(
        http = %cli.http_addr,
        grpc = %cli.grpc_addr,
        database = ?cli.database_url,
        tls = tls_enabled,
        "Starting CyxCloud gateway"
    );

    // Create gateway configuration
    #[cfg(feature = "blockchain")]
    let config = GatewayConfig {
        database_url: cli.database_url,
        redis_url: cli.redis_url,
        use_memory_storage: cli.memory_only,
        enable_blockchain: cli.enable_blockchain,
        solana_rpc_url: Some(cli.solana_rpc_url),
        keypair_path: cli.keypair_path,
    };

    #[cfg(not(feature = "blockchain"))]
    let config = GatewayConfig {
        database_url: cli.database_url,
        redis_url: cli.redis_url,
        use_memory_storage: cli.memory_only,
    };

    // Create shared application state
    let state = Arc::new(
        AppState::with_config(config)
            .await
            .expect("Failed to initialize application state"),
    );

    // Start node lifecycle monitor (background task)
    if state.metadata_service().is_some() {
        let monitor_config = node_monitor::NodeMonitorConfig::from_env();
        let monitor = Arc::new(node_monitor::NodeMonitor::new(monitor_config));
        let _monitor_handle = monitor.start(state.clone());
        info!("Node lifecycle monitor started");

        // Start payment daemon (background task)
        let payment_config = payment_daemon::PaymentDaemonConfig::from_env();
        let payment_daemon = Arc::new(payment_daemon::PaymentDaemon::new(payment_config));
        let _payment_handle = payment_daemon.start(state.clone());
        info!("Payment daemon started");

        // Start rebalancer daemon (background task)
        let rebalancer_config = rebalancer_daemon::RebalancerDaemonConfig::from_env();
        let rebalancer = Arc::new(rebalancer_daemon::RebalancerDaemon::new(rebalancer_config));
        let _rebalancer_handle = rebalancer.start(state.clone());
        info!("Rebalancer daemon started");
    } else {
        info!("Metadata service not configured, node monitor, payment daemon, and rebalancer disabled");
    }

    // Build CORS layer
    let cors = if cli.cors_permissive {
        CorsLayer::permissive()
    } else {
        CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any)
    };

    // Build HTTP router
    let app = Router::new()
        // Health and version endpoints
        .route("/health", get(health))
        .route("/version", get(version))
        // Authentication API
        .nest("/api/v1/auth", auth_api::routes())
        // S3-compatible API
        .nest("/s3", s3_api::routes())
        // WebSocket endpoint
        .merge(websocket::routes())
        // Add middleware
        .layer(TraceLayer::new_for_http())
        .layer(cors)
        .with_state(state.clone());

    // Parse addresses
    let http_addr: SocketAddr = cli.http_addr.parse()?;
    let grpc_addr: SocketAddr = cli.grpc_addr.parse()?;

    // Build TLS config if enabled
    let tls_server_config = if tls_enabled {
        Some(TlsServerConfig {
            cert_path: cli.tls_cert.clone().unwrap(),
            key_path: cli.tls_key.clone().unwrap(),
            ca_cert_path: cli.tls_ca_cert.clone(),
            require_client_cert: cli.tls_require_client_cert,
        })
    } else {
        None
    };

    // Start gRPC server in separate task
    let grpc_state = state.clone();
    let enable_grpc_auth = cli.grpc_auth;
    let grpc_tls_config = tls_server_config.clone();
    tokio::spawn(async move {
        // Node service for node registration and heartbeat
        let node_service = NodeServiceImpl::new(grpc_state.clone());

        // Data service for ML training data streaming
        let data_service = DataServiceImpl::new(grpc_state.clone());

        let tls_status = if grpc_tls_config.is_some() { "enabled" } else { "disabled" };
        info!(
            "gRPC server listening on {} (auth: {}, TLS: {})",
            grpc_addr,
            if enable_grpc_auth { "enabled" } else { "disabled" },
            tls_status
        );

        let mut builder = TonicServer::builder();

        // Configure TLS if enabled
        if let Some(ref tls_cfg) = grpc_tls_config {
            match create_tonic_server_tls(tls_cfg) {
                Ok(tls) => {
                    builder = builder.tls_config(tls).expect("Failed to configure gRPC TLS");
                    info!("gRPC TLS configured successfully");
                }
                Err(e) => {
                    error!(error = %e, "Failed to load gRPC TLS config");
                    return;
                }
            }
        }

        if enable_grpc_auth {
            // Create auth interceptor
            let auth_interceptor = AuthInterceptor::new(grpc_state.auth_service_arc());

            // Wrap services with authentication
            let node_server = NodeServiceServer::with_interceptor(node_service, auth_interceptor.clone());
            let data_server = DataServiceServer::with_interceptor(data_service, auth_interceptor);

            if let Err(e) = builder
                .add_service(node_server)
                .add_service(data_server)
                .serve(grpc_addr)
                .await
            {
                error!(error = %e, "gRPC server error");
            }
        } else {
            // No authentication
            let node_server = NodeServiceServer::new(node_service);
            let data_server = DataServiceServer::new(data_service);

            if let Err(e) = builder
                .add_service(node_server)
                .add_service(data_server)
                .serve(grpc_addr)
                .await
            {
                error!(error = %e, "gRPC server error");
            }
        }
    });

    // Start HTTP/HTTPS server based on TLS configuration
    if let Some(ref tls_cfg) = tls_server_config {
        // HTTPS mode with axum-server
        use axum_server::tls_rustls::RustlsConfig;

        let rustls_config = RustlsConfig::from_pem_file(&tls_cfg.cert_path, &tls_cfg.key_path)
            .await
            .expect("Failed to load TLS certificates for HTTPS");

        info!("HTTPS server listening on {} (TLS enabled)", http_addr);

        axum_server::bind_rustls(http_addr, rustls_config)
            .serve(app.into_make_service())
            .await?;
    } else {
        // Plain HTTP mode
        let http_listener = tokio::net::TcpListener::bind(http_addr).await?;
        info!("HTTP server listening on {} (TLS disabled)", http_addr);
        warn!("Running without TLS - use --tls-cert and --tls-key for production");

        axum::serve(http_listener, app)
            .with_graceful_shutdown(shutdown_signal())
            .await?;
    }

    info!("Gateway shutdown complete");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("Failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {
            info!("Received Ctrl+C, shutting down");
        },
        _ = terminate => {
            info!("Received terminate signal, shutting down");
        },
    }
}
