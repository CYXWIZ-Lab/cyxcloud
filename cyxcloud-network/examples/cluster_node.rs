//! Simple CyxCloud cluster node for testing
//!
//! Usage:
//!   cargo run --example cluster_node -- --port 50051 --data-dir ./data1 --node-id node1
//!
//! This starts a gRPC ChunkService server and optionally connects to discovery peers.

use bytes::Bytes;
use clap::Parser;
use cyxcloud_core::chunk::ChunkId;
use cyxcloud_network::{
    grpc_client::ChunkClient,
    grpc_server::{start_server, GrpcServerConfig},
};
use cyxcloud_storage::{RocksDbBackend, StorageConfig};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::signal;
use tracing::{info, Level};
use tracing_subscriber::FmtSubscriber;

#[derive(Parser, Debug)]
#[command(name = "cluster_node", about = "CyxCloud cluster test node")]
struct Args {
    /// gRPC port to listen on
    #[arg(short, long, default_value = "50051")]
    port: u16,

    /// Data directory for chunk storage
    #[arg(short, long, default_value = "./data")]
    data_dir: PathBuf,

    /// Node identifier
    #[arg(short, long, default_value = "node1")]
    node_id: String,

    /// Bootstrap peer address (e.g., 127.0.0.1:50051)
    #[arg(short, long)]
    bootstrap: Option<String>,

    /// Run a test after startup
    #[arg(long)]
    test: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    // Setup logging
    FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .with_target(false)
        .compact()
        .init();

    info!("Starting CyxCloud node: {}", args.node_id);
    info!("  Port: {}", args.port);
    info!("  Data directory: {:?}", args.data_dir);

    // Create data directory if it doesn't exist
    std::fs::create_dir_all(&args.data_dir)?;

    // Initialize storage
    let storage_config = StorageConfig::new(&args.data_dir);
    let storage = Arc::new(RocksDbBackend::open(storage_config)?);
    info!("Storage initialized at {:?}", args.data_dir);

    // Configure gRPC server
    let addr: SocketAddr = format!("0.0.0.0:{}", args.port).parse()?;
    let grpc_config = GrpcServerConfig::new(addr);

    // Start gRPC server in background
    let storage_clone = storage.clone();
    let node_id = args.node_id.clone();
    let server_handle = tokio::spawn(async move {
        if let Err(e) = start_server(grpc_config, storage_clone, node_id).await {
            eprintln!("Server error: {}", e);
        }
    });

    info!("gRPC server started on port {}", args.port);

    // If test mode, run a quick self-test
    if args.test {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        run_self_test(args.port).await?;
    }

    // If bootstrap peer provided, test connection
    if let Some(bootstrap_addr) = &args.bootstrap {
        info!("Testing connection to bootstrap peer: {}", bootstrap_addr);
        let client = ChunkClient::new();
        if client.is_reachable(bootstrap_addr).await {
            info!("Bootstrap peer is reachable!");

            // Try to store a test chunk on bootstrap peer
            let test_data = format!("Hello from {}", args.node_id);
            let chunk_id = ChunkId::from_data(test_data.as_bytes());
            match client
                .store_chunk(bootstrap_addr, chunk_id, Bytes::from(test_data.clone()))
                .await
            {
                Ok(()) => info!("Successfully stored test chunk on bootstrap peer"),
                Err(e) => info!("Failed to store chunk on bootstrap: {}", e),
            }
        } else {
            info!("Warning: Bootstrap peer not reachable");
        }
    }

    info!("Node {} is ready. Press Ctrl+C to stop.", args.node_id);

    // Wait for shutdown signal
    signal::ctrl_c().await?;
    info!("Shutting down...");

    server_handle.abort();

    Ok(())
}

async fn run_self_test(port: u16) -> Result<(), Box<dyn std::error::Error>> {
    info!("Running self-test...");

    let client = ChunkClient::new();
    let addr = format!("127.0.0.1:{}", port);

    // Test 1: Store a chunk
    let test_data = b"self-test data chunk";
    let chunk_id = ChunkId::from_data(test_data);

    client
        .store_chunk(&addr, chunk_id, Bytes::from_static(test_data))
        .await?;
    info!("  [PASS] Store chunk");

    // Test 2: Get the chunk
    let retrieved = client.get_chunk(&addr, chunk_id).await?;
    assert!(retrieved.is_some());
    assert_eq!(retrieved.unwrap().as_ref(), test_data);
    info!("  [PASS] Get chunk");

    // Test 3: Verify chunk
    let (valid, size) = client.verify_chunk(&addr, chunk_id).await?;
    assert!(valid);
    assert_eq!(size, test_data.len() as u64);
    info!("  [PASS] Verify chunk");

    // Test 4: Delete chunk
    let deleted = client.delete_chunk(&addr, chunk_id).await?;
    assert!(deleted);
    info!("  [PASS] Delete chunk");

    // Test 5: Verify deleted
    let gone = client.get_chunk(&addr, chunk_id).await?;
    assert!(gone.is_none());
    info!("  [PASS] Verify deleted");

    info!("All self-tests passed!");

    Ok(())
}
