//! CyxCloud CLI
//!
//! Command-line client for interacting with CyxCloud storage.
//!
//! # Commands
//! - `login` - Login to CyxCloud
//! - `logout` - Logout from CyxCloud
//! - `whoami` - Show current user profile
//! - `register` - Register a new account
//! - `upload` - Upload a file or directory
//! - `download` - Download a file or directory
//! - `list` - List stored files
//! - `delete` - Delete a file from storage
//! - `status` - Show storage status
//! - `config` - Show or edit configuration
//!
//! # Configuration
//! Config file: ~/.cyxcloud/config.toml
//! Credentials: ~/.cyxcloud/credentials.json

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

mod client;
mod commands;
mod config;
mod cyxwiz_client;
mod symbols;

use client::{GatewayClient, TlsConfig};
use commands::{auth, delete, download, list, status, upload};
use cyxwiz_client::CyxWizClient;

#[derive(Parser)]
#[command(name = "cyxcloud")]
#[command(about = "CyxCloud distributed storage CLI")]
#[command(version)]
struct Cli {
    /// Gateway HTTP URL (overrides config file)
    #[arg(long, global = true)]
    gateway: Option<String>,

    /// CyxWiz API URL for authentication (overrides config file)
    #[arg(long, global = true)]
    api_url: Option<String>,

    // ===== TLS Configuration =====
    /// Path to CA certificate for verifying the gateway (PEM format)
    #[arg(long, global = true, env = "CYXCLOUD_CA_CERT")]
    ca_cert: Option<PathBuf>,

    /// Path to client certificate for mTLS (PEM format)
    #[arg(long, global = true, env = "CYXCLOUD_CLIENT_CERT")]
    client_cert: Option<PathBuf>,

    /// Path to client private key for mTLS (PEM format)
    #[arg(long, global = true, env = "CYXCLOUD_CLIENT_KEY")]
    client_key: Option<PathBuf>,

    /// Skip server certificate verification (DANGEROUS - development only)
    #[arg(long, global = true, default_value = "false")]
    insecure: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Login to CyxCloud
    Login {
        /// Email address (will prompt if not provided)
        #[arg(short, long)]
        email: Option<String>,
    },

    /// Logout from CyxCloud
    Logout,

    /// Show current user profile
    Whoami,

    /// Register a new account
    Register {
        /// Email address
        #[arg(short, long)]
        email: String,

        /// Username (optional)
        #[arg(short, long)]
        username: Option<String>,
    },

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

    /// Show or initialize configuration
    Config {
        #[command(subcommand)]
        command: Option<ConfigCommands>,
    },
}

#[derive(Subcommand)]
enum ConfigCommands {
    /// Show current configuration
    Show,

    /// Show config file path
    Path,

    /// Initialize config file with defaults
    Init {
        /// Overwrite existing config
        #[arg(short, long)]
        force: bool,
    },

    /// Set a configuration value
    Set {
        /// Configuration key (e.g., gateway.http_url, auth.api_url)
        key: String,
        /// Value to set
        value: String,
    },
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

    // Load configuration from ~/.cyxcloud/config.toml
    let cfg = config::load_config();

    // CLI args override config file
    let gateway_url = cli.gateway.unwrap_or(cfg.gateway.http_url);
    let api_url = cli.api_url.unwrap_or(cfg.auth.api_url);

    // Create CyxWiz API client for auth commands
    let cyxwiz_client = CyxWizClient::new(&api_url);

    // Get auth token for gateway commands
    let auth_token = config::get_valid_credentials().map(|c| c.access_token);

    // Build TLS configuration from CLI args
    let tls_config = if cli.ca_cert.is_some() || cli.client_cert.is_some() || cli.insecure {
        Some(TlsConfig {
            ca_cert: cli.ca_cert,
            client_cert: cli.client_cert,
            client_key: cli.client_key,
            danger_accept_invalid_certs: cli.insecure,
        })
    } else {
        None
    };

    // Create gateway client with auth token and optional TLS
    let client = GatewayClient::with_tls(&gateway_url, auth_token.clone(), tls_config);

    match cli.command {
        // Auth commands
        Commands::Login { email } => {
            let config = auth::LoginConfig { email };
            auth::login(&cyxwiz_client, config).await?;
        }

        Commands::Logout => {
            auth::logout(&cyxwiz_client).await?;
        }

        Commands::Whoami => {
            auth::whoami(&cyxwiz_client).await?;
        }

        Commands::Register { email, username } => {
            let config = auth::RegisterConfig { email, username };
            auth::register(&cyxwiz_client, config).await?;
        }

        // Storage commands (require auth)
        Commands::Upload {
            path,
            bucket,
            prefix,
            encrypt,
        } => {
            require_auth(&auth_token)?;
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
            require_auth(&auth_token)?;
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
            require_auth(&auth_token)?;
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
            // Status doesn't require auth (health check)
            let config = status::StatusConfig { bucket, verbose };
            status::run(&client, config).await?;
        }

        Commands::Delete { bucket, key, force } => {
            require_auth(&auth_token)?;
            let config = delete::DeleteConfig { bucket, key, force };
            delete::run(&client, config).await?;
        }

        Commands::Config { command } => {
            handle_config_command(command)?;
        }
    }

    Ok(())
}

/// Handle config subcommands
fn handle_config_command(command: Option<ConfigCommands>) -> Result<()> {
    use console::style;

    match command {
        None | Some(ConfigCommands::Show) => {
            // Show current configuration
            let cfg = config::load_config();
            println!();
            println!("{}", style("CyxCloud Configuration").bold().underlined());
            println!();
            println!("{}", style("[gateway]").cyan());
            println!("  http_url = \"{}\"", cfg.gateway.http_url);
            println!("  grpc_url = \"{}\"", cfg.gateway.grpc_url);
            println!();
            println!("{}", style("[auth]").cyan());
            println!("  api_url = \"{}\"", cfg.auth.api_url);
            println!();
            println!("{}", style("[blockchain]").cyan());
            println!("  solana_rpc_url = \"{}\"", cfg.blockchain.solana_rpc_url);
            println!();
            if let Some(bucket) = &cfg.cli.default_bucket {
                println!("{}", style("[cli]").cyan());
                println!("  default_bucket = \"{}\"", bucket);
                println!();
            }

            // Show config file path
            if let Ok(path) = config::config_file_path() {
                println!("{} {}", style("Config file:").dim(), path.display());
                if !path.exists() {
                    println!(
                        "{} Run '{}' to create it",
                        style("(not created yet)").yellow(),
                        style("cyxcloud config init").green()
                    );
                }
            }
        }

        Some(ConfigCommands::Path) => {
            if let Ok(path) = config::config_file_path() {
                println!("{}", path.display());
            }
        }

        Some(ConfigCommands::Init { force }) => {
            let path = config::config_file_path()?;
            if path.exists() && !force {
                println!(
                    "{} Config file already exists at {}",
                    style("!").yellow(),
                    path.display()
                );
                println!("Use --force to overwrite");
                return Ok(());
            }

            config::save_config(&config::CyxCloudConfig::default())?;
            println!(
                "{} Config file created at {}",
                style(symbols::CHECK).green(),
                path.display()
            );
        }

        Some(ConfigCommands::Set { key, value }) => {
            let mut cfg = config::load_config();

            match key.as_str() {
                "gateway.http_url" => cfg.gateway.http_url = value,
                "gateway.grpc_url" => cfg.gateway.grpc_url = value,
                "auth.api_url" => cfg.auth.api_url = value,
                "blockchain.solana_rpc_url" => cfg.blockchain.solana_rpc_url = value,
                "cli.default_bucket" => cfg.cli.default_bucket = Some(value),
                _ => {
                    anyhow::bail!(
                        "Unknown config key: {}. Valid keys: gateway.http_url, gateway.grpc_url, auth.api_url, blockchain.solana_rpc_url, cli.default_bucket",
                        key
                    );
                }
            }

            config::save_config(&cfg)?;
            println!("{} Configuration updated", style(symbols::CHECK).green());
        }
    }

    Ok(())
}

/// Check if user is authenticated, return error if not
fn require_auth(token: &Option<String>) -> Result<()> {
    if token.is_none() {
        anyhow::bail!("Not logged in. Run 'cyxcloud login' first.");
    }
    Ok(())
}
