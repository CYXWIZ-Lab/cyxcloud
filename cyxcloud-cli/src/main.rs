//! CyxCloud CLI
//!
//! Command-line client for interacting with CyxCloud storage.
//!
//! # Commands
//! - `upload` - Upload a file or directory
//! - `download` - Download a file or directory
//! - `list` - List stored files
//! - `status` - Show storage status
//! - `node` - Node management commands

use anyhow::Result;
use clap::{Parser, Subcommand};

mod client;
mod commands;

use client::GatewayClient;
use commands::{delete, download, list, status, upload};

#[derive(Parser)]
#[command(name = "cyxcloud")]
#[command(about = "CyxCloud decentralized storage CLI")]
#[command(version)]
struct Cli {
    /// Gateway URL
    #[arg(long, default_value = "http://localhost:8080", global = true)]
    gateway: String,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Upload a file or directory to storage
    Upload {
        /// Path to file or directory
        path: String,

        /// Target bucket name
        #[arg(short, long, default_value = "default")]
        bucket: String,

        /// Key prefix for uploaded files
        #[arg(short, long)]
        prefix: Option<String>,

        /// Enable encryption (not yet implemented)
        #[arg(short, long)]
        encrypt: bool,
    },

    /// Download a file or directory from storage
    Download {
        /// Bucket name
        bucket: String,

        /// Object key (if not provided, downloads all with prefix)
        #[arg(short, long)]
        key: Option<String>,

        /// Key prefix for bulk download
        #[arg(long)]
        prefix: Option<String>,

        /// Output path (file or directory)
        #[arg(short, long, default_value = ".")]
        output: String,
    },

    /// List objects in a bucket
    List {
        /// Bucket name
        bucket: String,

        /// Filter by prefix
        #[arg(short, long)]
        prefix: Option<String>,

        /// Show detailed information
        #[arg(short, long)]
        long: bool,

        /// Human-readable sizes
        #[arg(short = 'H', long)]
        human_readable: bool,

        /// Maximum number of keys to return
        #[arg(long)]
        max_keys: Option<i32>,
    },

    /// Show storage status
    Status {
        /// Show specific bucket status
        #[arg(short, long)]
        bucket: Option<String>,

        /// Show verbose output
        #[arg(short, long)]
        verbose: bool,
    },

    /// Delete an object from storage
    Delete {
        /// Bucket name
        bucket: String,

        /// Object key to delete
        key: String,

        /// Delete without confirmation
        #[arg(short, long)]
        force: bool,
    },

    /// Node management
    Node {
        #[command(subcommand)]
        command: NodeCommands,
    },
}

#[derive(Subcommand)]
enum NodeCommands {
    /// Show node status
    Status,

    /// Start local node
    Start {
        /// Storage allocation in GB
        #[arg(long, default_value = "100")]
        storage_gb: u64,
    },

    /// Stop local node
    Stop,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::WARN.into()),
        )
        .init();

    let cli = Cli::parse();
    let client = GatewayClient::new(&cli.gateway);

    match cli.command {
        Commands::Upload {
            path,
            bucket,
            prefix,
            encrypt,
        } => {
            let config = upload::UploadConfig {
                path,
                bucket,
                prefix,
                encrypt,
            };
            upload::run(&client, config).await?;
        }

        Commands::Download {
            bucket,
            key,
            prefix,
            output,
        } => {
            let config = download::DownloadConfig {
                bucket,
                key,
                output,
                prefix,
            };
            download::run(&client, config).await?;
        }

        Commands::List {
            bucket,
            prefix,
            long,
            human_readable,
            max_keys,
        } => {
            let config = list::ListConfig {
                bucket,
                prefix,
                long_format: long,
                human_readable,
                max_keys,
            };
            list::run(&client, config).await?;
        }

        Commands::Status { bucket, verbose } => {
            let config = status::StatusConfig { bucket, verbose };
            status::run(&client, config).await?;
        }

        Commands::Delete { bucket, key, force } => {
            let config = delete::DeleteConfig { bucket, key, force };
            delete::run(&client, config).await?;
        }

        Commands::Node { command } => match command {
            NodeCommands::Status => {
                println!("Node status: Not implemented yet");
                println!("Local node management will be available in a future release.");
            }
            NodeCommands::Start { storage_gb } => {
                println!(
                    "Starting node with {} GB storage: Not implemented yet",
                    storage_gb
                );
                println!("Use cyxcloud-node binary to run a storage node.");
            }
            NodeCommands::Stop => {
                println!("Stopping node: Not implemented yet");
            }
        },
    }

    Ok(())
}
