//! CyxCloud CLI
//!
//! Command-line client for interacting with CyxCloud storage.
//!
//! # Commands
//! - `login` - Login to CyxCloud
//! - `logout` - Logout from CyxCloud
//! - `whoami` - Show current user profile
//! - `register` - Register a new account
//! - `dataset put | ls | get | info | rm | verify | share` - Datasets
//! - `token create` - A dataset access token for a training node
//! - `config` - Show or edit configuration
//! - `node` - Set up and run a storage node on this machine
//! - `doctor` - Check the connection to the server and the node
//!
//! # Configuration
//! Config file: ~/.cyxcloud/config.toml
//! Credentials: ~/.cyxcloud/credentials.json

// (dead code is not allowed: no orphans)

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

mod client;
mod cloud;
mod commands;
mod config;
mod cyxwiz_client;
mod exit;
mod server;
mod symbols;

use client::{GatewayClient, TlsConfig};
use commands::{auth, dataset, datasets, doctor, node, service};
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

        /// Server to sign in to (host name or IP, or a front door: https://HOST, http://HOST:PORT);
        /// remembered for every later command
        #[arg(long)]
        server: Option<String>,

        /// Read the password from this file's first line (for scripts)
        #[arg(long)]
        password_file: Option<PathBuf>,
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

    /// Show or initialize configuration
    Config {
        #[command(subcommand)]
        command: Option<ConfigCommands>,
    },

    /// Set up and run a storage node on this machine (offer disk space)
    Node {
        #[command(subcommand)]
        command: NodeCommands,
    },

    /// Check the connection to the server, the sign-in and the node
    Doctor,

    /// Your datasets: put, ls, get, info, rm, verify, share
    Dataset {
        #[command(subcommand)]
        command: DatasetCommands,
    },

    /// Dataset access tokens (what a training node streams with)
    Token {
        #[command(subcommand)]
        command: TokenCommands,
    },
}

#[derive(Subcommand)]
enum DatasetCommands {
    /// Upload files (or every file in a folder) as one dataset
    Put {
        /// Files or folders
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        /// Dataset name (what the Engine shows)
        #[arg(short, long)]
        name: String,
        /// Description
        #[arg(short, long)]
        description: Option<String>,
    },

    /// List your datasets
    Ls {
        /// Include datasets shared with you
        #[arg(short, long)]
        shared: bool,
        /// Public datasets (MNIST, CIFAR, ...) instead
        #[arg(long)]
        public: bool,
        /// Only names containing this
        #[arg(short, long)]
        filter: Option<String>,
    },

    /// Download a dataset's files (each one SHA-256 verified)
    Get {
        /// Dataset name or id
        dataset: String,
        /// Folder to write into
        #[arg(short, long, default_value = ".")]
        output: PathBuf,
    },

    /// Show a dataset and its files
    Info {
        /// Dataset name or id
        dataset: String,
    },

    /// Remove a dataset
    Rm {
        /// Dataset name or id
        dataset: String,
        /// Do not ask
        #[arg(short, long)]
        force: bool,
    },

    /// Verify dataset integrity
    Verify {
        /// Dataset ID
        dataset_id: String,

        /// Check against public registry
        #[arg(long, default_value = "true")]
        check_public: bool,

        /// Verify all file contents (slow)
        #[arg(long)]
        full: bool,
    },

    /// Share dataset with another user
    Share {
        /// Dataset ID
        dataset_id: String,

        /// User email or ID to share with
        #[arg(short, long)]
        with: String,

        /// Permissions: read, stream, reshare
        #[arg(short, long, default_value = "read,stream")]
        permissions: String,
    },
}

#[derive(Subcommand)]
enum TokenCommands {
    /// A read+stream access token for a dataset; prints only the token
    Create {
        /// Dataset name or id
        dataset: String,
        /// Lifetime: 15m, 1h, 2d (1 minute to 30 days)
        #[arg(long, default_value = "1h")]
        ttl: String,
    },
}

#[derive(Subcommand)]
enum NodeCommands {
    /// Set this machine up as a storage node (where, how much, which account)
    Init {
        /// Folder for the shards
        #[arg(long)]
        data_dir: Option<PathBuf>,
        /// Space offered, e.g. 700GB or 1TB
        #[arg(long)]
        offer: Option<String>,
        /// Name shown in lists (default: this machine's host name)
        #[arg(long)]
        name: Option<String>,
        /// Port the server dials for shards (default 50051)
        #[arg(long)]
        port: Option<u16>,
        /// Local port for `cyxcloud node status` (default 9090)
        #[arg(long)]
        status_port: Option<u16>,
        /// Address the server dials (default: this machine's address towards the server)
        #[arg(long)]
        public_address: Option<String>,
        /// Set the node up again (the key and its id are kept)
        #[arg(long)]
        force: bool,
    },
    /// Start the node in the background (or in this terminal with --foreground)
    Start {
        #[arg(long)]
        foreground: bool,
    },
    /// Stop the node started with `cyxcloud node start`
    Stop,
    /// Give the node this CLI's current sign-in (keeps its folder, offer and key)
    SignIn,
    /// Show what the node is doing
    Status {
        /// Refresh every 2 s
        #[arg(long)]
        watch: bool,
        /// Print the raw report
        #[arg(long)]
        json: bool,
    },
    /// Start the node with the machine (systemd, launchd, or the Windows Run key)
    Service {
        #[command(subcommand)]
        command: ServiceCommands,
    },
    /// Show the node's log
    Logs {
        /// Lines to show
        #[arg(short = 'n', long, default_value = "50")]
        lines: usize,
        /// Keep printing new lines
        #[arg(short, long)]
        follow: bool,
    },
}

#[derive(Subcommand)]
enum ServiceCommands {
    /// Start the node at boot (Linux), at login (macOS) or at logon (Windows)
    Install,
    /// Stop starting the node with the machine
    Uninstall,
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
async fn main() {
    if let Err(e) = run().await {
        let kind = exit::classify(&e);
        eprintln!(
            "{} {}",
            console::style(symbols::CROSS).red(),
            exit::describe(&e)
        );
        if let Some(step) = exit::next_step(kind) {
            eprintln!("  {}", step);
        }
        std::process::exit(kind.code());
    }
}

async fn run() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::WARN.into()),
        )
        .init();

    // clap exits with 2 on a usage error; here a wrong argument is the
    // user's (1) and 2 means the network (see exit.rs)
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            let code = if e.use_stderr() {
                exit::Kind::User.code()
            } else {
                0
            };
            let _ = e.print();
            std::process::exit(code);
        }
    };

    // Load configuration from ~/.cyxcloud/config.toml
    let cfg = config::load_config();

    // CLI args override config file
    let grpc_url = cfg.gateway.grpc_url.clone();
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
    let client = GatewayClient::with_tls(&gateway_url, auth_token.clone(), tls_config)?;

    match cli.command {
        // Auth commands
        Commands::Login {
            email,
            server,
            password_file,
        } => {
            let config = auth::LoginConfig {
                email,
                server,
                password_file,
            };
            auth::login(&cyxwiz_client, config).await?;
        }

        Commands::Logout => {
            auth::logout().await?;
        }

        Commands::Whoami => {
            auth::whoami(&cyxwiz_client).await?;
        }

        Commands::Register { email, username } => {
            let config = auth::RegisterConfig { email, username };
            auth::register(&cyxwiz_client, config).await?;
        }

        Commands::Config { command } => {
            handle_config_command(command)?;
        }

        Commands::Node { command } => match command {
            NodeCommands::Init {
                data_dir,
                offer,
                name,
                port,
                status_port,
                public_address,
                force,
            } => {
                node::init(node::InitConfig {
                    data_dir,
                    offer,
                    name,
                    port,
                    status_port,
                    public_address,
                    force,
                })
                .await?
            }
            NodeCommands::Start { foreground } => node::start(foreground).await?,
            NodeCommands::Stop => node::stop().await?,
            NodeCommands::SignIn => node::sign_in().await?,
            NodeCommands::Status { watch, json } => node::status(watch, json).await?,
            NodeCommands::Logs { lines, follow } => node::logs(lines, follow).await?,
            NodeCommands::Service { command } => match command {
                ServiceCommands::Install => service::install().await?,
                ServiceCommands::Uninstall => service::uninstall().await?,
            },
        },

        Commands::Doctor => {
            let code = doctor::run(&cyxwiz_client).await?;
            std::process::exit(code);
        }

        Commands::Dataset { command } => {
            require_auth(&auth_token)?;
            match command {
                DatasetCommands::Verify {
                    dataset_id,
                    check_public,
                    full,
                } => {
                    let config = dataset::VerifyConfig {
                        dataset_id,
                        check_public,
                        full_verification: full,
                    };
                    dataset::verify(&client, config).await?;
                }
                DatasetCommands::Share {
                    dataset_id,
                    with,
                    permissions,
                } => {
                    let config = dataset::ShareConfig {
                        dataset_id,
                        share_with: with,
                        permissions: permissions
                            .split(',')
                            .map(|s| s.trim().to_string())
                            .collect(),
                    };
                    dataset::share(&client, config).await?;
                }
                other => {
                    let cloud =
                        cloud::Cloud::connect(&grpc_url, auth_token.as_deref().unwrap_or(""))
                            .await?;
                    match other {
                        DatasetCommands::Put {
                            paths,
                            name,
                            description,
                        } => datasets::put(&cloud, paths, name, description).await?,
                        DatasetCommands::Ls {
                            shared,
                            public,
                            filter,
                        } => datasets::ls(&cloud, shared, public, filter).await?,
                        DatasetCommands::Get { dataset, output } => {
                            datasets::get(&cloud, &dataset, output).await?
                        }
                        DatasetCommands::Info { dataset } => {
                            datasets::info(&cloud, &dataset).await?
                        }
                        DatasetCommands::Rm { dataset, force } => {
                            datasets::rm(&cloud, &dataset, force).await?
                        }
                        DatasetCommands::Verify { .. } | DatasetCommands::Share { .. } => {
                            unreachable!("handled above")
                        }
                    }
                }
            }
        }

        Commands::Token { command } => {
            require_auth(&auth_token)?;
            let cloud =
                cloud::Cloud::connect(&grpc_url, auth_token.as_deref().unwrap_or("")).await?;
            match command {
                TokenCommands::Create { dataset, ttl } => {
                    datasets::token_create(&cloud, &dataset, &ttl).await?
                }
            }
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
            println!("{}", style("[server]").cyan());
            match &cfg.server.host {
                Some(host) => println!("  host = \"{}\"", host),
                None => println!("  (not set: cyxcloud login --server HOST)"),
            }
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
                "server" => {
                    cfg.set_server(&value).map_err(anyhow::Error::msg)?;
                    println!("  gateway  {}", cfg.gateway.http_url);
                    println!("  gRPC     {}", cfg.gateway.grpc_url);
                    println!("  sign-in  {}", cfg.auth.api_url);
                }
                "gateway.http_url" => cfg.gateway.http_url = value,
                "gateway.grpc_url" => cfg.gateway.grpc_url = value,
                "auth.api_url" => cfg.auth.api_url = value,
                "blockchain.solana_rpc_url" => cfg.blockchain.solana_rpc_url = value,
                "cli.default_bucket" => cfg.cli.default_bucket = Some(value),
                _ => {
                    anyhow::bail!(
                        "Unknown config key: {}. Valid keys: server, gateway.http_url, gateway.grpc_url, auth.api_url, blockchain.solana_rpc_url, cli.default_bucket",
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
