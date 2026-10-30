//! `cyxwiz-front --config front.toml`: see docs/usage/front.md.

use clap::Parser;
use cyxwiz_front::{config::FrontConfig, serve, tls_acceptor, Front};
use std::path::PathBuf;
use std::sync::Arc;
use tracing::info;

#[derive(Parser)]
#[command(
    name = "cyxwiz-front",
    about = "The CyxWiz server side's one front door"
)]
struct Cli {
    /// front.toml (without it: port 8000, the local services, no TLS)
    #[arg(long, env = "CYXWIZ_FRONT_CONFIG")]
    config: Option<PathBuf>,
    /// Shared user-token secret (the API, central and the gateway use it)
    #[arg(long, env = "JWT_SECRET", hide_env_values = true)]
    jwt_secret: String,
    /// Admin-token secret
    #[arg(
        long,
        env = "ADMIN_JWT_SECRET",
        hide_env_values = true,
        default_value = ""
    )]
    admin_jwt_secret: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let cli = Cli::parse();
    let text = match &cli.config {
        Some(p) => {
            std::fs::read_to_string(p).map_err(|e| anyhow::anyhow!("{}: {}", p.display(), e))?
        }
        None => String::new(),
    };
    let config = FrontConfig::from_toml(&text)?;
    let tls = tls_acceptor(&config)?;
    let listener = cyxwiz_front::listen(config.listen)?;
    info!(
        listen = %config.listen,
        tls = tls.is_some(),
        routes = config.routes.len(),
        "Front door open"
    );
    for r in &config.routes {
        info!(
            prefix = %r.prefix,
            upstream = %r.upstream,
            grpc = r.grpc,
            "route {}",
            r.name
        );
    }
    let front = Arc::new(Front::new(config, &[cli.jwt_secret, cli.admin_jwt_secret])?);
    serve(listener, front, tls).await
}
