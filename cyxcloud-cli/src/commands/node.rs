//! `cyxcloud node ...`: set up and run a storage node from the CLI (TOFIX177).
//!
//! Files, all under ~/.cyxcloud/node/ unless noted:
//! - config.toml     the node daemon's configuration (`cyxcloud node init`)
//! - key             the node's Ed25519 key; it owns the node id (owner-only)
//! - access_secret   what the gateway presents on chunk RPCs (owner-only)
//! - node.pid, node.log   the running daemon (`cyxcloud node start`)
//! - ~/.cyxcloud/node_credentials.json   the account the node earns for

use crate::commands::service;
use crate::config;
use crate::server::{self, human_bytes, human_duration};
use crate::symbols;
use anyhow::{bail, Context, Result};
use console::style;
use cyxcloud_core::node_status::NodeStatusReport;
use std::io::{BufRead, Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Default ports of a node set up by `cyxcloud node init`
pub const DEFAULT_GRPC_PORT: u16 = 50051;
pub const DEFAULT_STATUS_PORT: u16 = 9090;

// ============================================================================
// Paths
// ============================================================================

pub fn node_dir() -> Result<PathBuf> {
    Ok(config::config_dir()?.join("node"))
}
pub fn node_config_path() -> Result<PathBuf> {
    Ok(node_dir()?.join("config.toml"))
}
fn pid_path() -> Result<PathBuf> {
    Ok(node_dir()?.join("node.pid"))
}
pub fn log_path() -> Result<PathBuf> {
    Ok(node_dir()?.join("node.log"))
}
fn node_credentials_path() -> Result<PathBuf> {
    Ok(config::config_dir()?.join("node_credentials.json"))
}

// ============================================================================
// Pure pieces (tested)
// ============================================================================

/// What `cyxcloud node init` writes.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeSetup {
    pub name: String,
    pub data_dir: PathBuf,
    pub offer_gb: u64,
    pub grpc_port: u16,
    pub public_address: String,
    pub status_port: u16,
    pub gateway_grpc: String,
    pub api_url: String,
}

/// The node daemon's config.toml for this setup. No `[node] id`: the node key
/// decides the id.
pub fn node_config_toml(setup: &NodeSetup) -> String {
    let q = |s: &str| toml::Value::String(s.to_string()).to_string();
    format!(
        "# Written by `cyxcloud node init`. The node id comes from ~/.cyxcloud/node/key.\n\
         [node]\n\
         name = {name}\n\
         \n\
         [storage]\n\
         data_dir = {data_dir}\n\
         max_capacity_gb = {offer}\n\
         compression = true\n\
         \n\
         [network]\n\
         bind_address = \"0.0.0.0\"\n\
         grpc_port = {grpc}\n\
         public_address = {public}\n\
         \n\
         [metrics]\n\
         enabled = true\n\
         port = {status}\n\
         \n\
         [central]\n\
         address = {gateway}\n\
         register = true\n\
         heartbeat_interval_secs = 30\n\
         \n\
         [cyxwiz_api]\n\
         base_url = {api}\n\
         register = true\n",
        name = q(&setup.name),
        data_dir = q(&setup.data_dir.to_string_lossy().replace('\\', "/")),
        offer = setup.offer_gb,
        grpc = setup.grpc_port,
        public = q(&setup.public_address),
        status = setup.status_port,
        gateway = q(&setup.gateway_grpc),
        api = q(&setup.api_url),
    )
}

/// The status port a node config uses (its `[metrics] port`).
pub fn status_port_of(config_toml: &str) -> u16 {
    toml::from_str::<toml::Value>(config_toml)
        .ok()
        .and_then(|v| v.get("metrics")?.get("port")?.as_integer())
        .and_then(|p| u16::try_from(p).ok())
        .unwrap_or(DEFAULT_STATUS_PORT)
}

/// A node counts as online when the server acknowledged a heartbeat within
/// three intervals.
pub const ONLINE_WITHIN_SECS: i64 = 90;

/// The lines of `cyxcloud node status` (no colours; the caller styles them).
pub fn status_lines(r: &NodeStatusReport, now: i64) -> Vec<(String, String)> {
    let short_id: String = r.node_id.chars().take(8).collect();
    let online = r.server.registered
        && r.server.last_heartbeat_unix > 0
        && now - r.server.last_heartbeat_unix <= ONLINE_WITHIN_SECS;
    let state = if online {
        format!("ONLINE for {}", human_duration(now - r.started_unix))
    } else if r.server.registered {
        "REGISTERED, waiting for the first heartbeat".to_string()
    } else {
        "NOT REGISTERED".to_string()
    };

    let mut lines = vec![(
        String::new(),
        format!("{} · {} · {}", r.name, short_id, state),
    )];

    let m = &r.machine;
    let mut machine = Vec::new();
    if !m.cpu_model.is_empty() {
        machine.push(format!(
            "{} ({} cores, {} threads)",
            m.cpu_model, m.cpu_cores, m.cpu_threads
        ));
    }
    if m.ram_bytes > 0 {
        machine.push(format!("{} RAM", human_bytes(m.ram_bytes)));
    }
    if !m.os.is_empty() {
        machine.push(m.os.clone());
    }
    if !m.hostname.is_empty() {
        machine.push(format!("host {}", m.hostname));
    }
    lines.push(("machine".into(), machine.join(" · ")));

    let s = &r.storage;
    let mut storage = format!(
        "{} · {} used of {} offered",
        s.data_dir,
        human_bytes(s.used_bytes),
        human_bytes(s.offered_bytes)
    );
    if s.disk_total_bytes > 0 {
        storage.push_str(&format!(
            " ({} free on the disk)",
            human_bytes(s.disk_free_bytes)
        ));
    }
    lines.push(("storage".into(), storage));
    lines.push(("shards".into(), format!("{} held", s.chunks)));

    let mut srv = r.server.address.clone();
    if r.server.last_heartbeat_unix > 0 {
        srv.push_str(&format!(
            " · heartbeat {} ago",
            human_duration(now - r.server.last_heartbeat_unix)
        ));
    }
    lines.push(("server".into(), srv));
    lines.push(("listening".into(), r.listen_addr.clone()));
    if !r.server.last_error.is_empty() {
        lines.push(("last error".into(), r.server.last_error.clone()));
    }
    lines.push(("version".into(), format!("cyxcloud-node {}", r.version)));
    lines
}

/// Remove ANSI colour codes from a log line.
pub fn strip_ansi(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// The node's credentials for the signed-in account (what the daemon reads
/// from ~/.cyxcloud/node_credentials.json). Keeps an existing machine
/// registration when it belongs to the same account; a file left by another
/// account is replaced.
pub fn node_credentials_json(
    existing: Option<&serde_json::Value>,
    creds: &config::Credentials,
    node_id: &str,
) -> serde_json::Value {
    let same_account = existing
        .and_then(|e| e.get("email"))
        .and_then(|e| e.as_str())
        .map(|e| e.eq_ignore_ascii_case(&creds.email))
        .unwrap_or(false);
    let keep = |key: &str| -> serde_json::Value {
        if same_account {
            existing
                .and_then(|e| e.get(key))
                .cloned()
                .unwrap_or(serde_json::Value::Null)
        } else {
            serde_json::Value::Null
        }
    };
    serde_json::json!({
        "user_id": creds.user_id,
        "email": creds.email,
        "username": creds.username.clone().unwrap_or_else(|| creds.email.split('@').next().unwrap_or("").to_string()),
        "auth_token": creds.access_token,
        "machine_id": keep("machine_id"),
        "api_key": keep("api_key"),
        "node_id": node_id,
        "wallet_address": keep("wallet_address"),
    })
}

// ============================================================================
// Talking to a running node
// ============================================================================

fn status_url(port: u16) -> String {
    format!("http://127.0.0.1:{}/status", port)
}

async fn fetch_status(port: u16) -> Option<NodeStatusReport> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .ok()?;
    let resp = client.get(status_url(port)).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.json::<NodeStatusReport>().await.ok()
}

fn configured_status_port() -> Result<u16> {
    let path = node_config_path()?;
    let text = std::fs::read_to_string(&path).with_context(|| {
        format!(
            "no node is set up on this machine ({} is missing): run `cyxcloud node init`",
            path.display()
        )
    })?;
    Ok(status_port_of(&text))
}

// ============================================================================
// Processes (our own pid file only; the image name is checked before a kill)
// ============================================================================

pub fn read_pid() -> Option<u32> {
    std::fs::read_to_string(pid_path().ok()?)
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// True when `pid` is a running cyxcloud-node.
pub fn is_our_node(pid: u32) -> bool {
    #[cfg(windows)]
    {
        std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {}", pid), "/FO", "CSV", "/NH"])
            .output()
            .map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .to_lowercase()
                    .contains("cyxcloud-node")
            })
            .unwrap_or(false)
    }
    #[cfg(not(windows))]
    {
        std::process::Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "comm="])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains("cyxcloud-node"))
            .unwrap_or(false)
    }
}

fn kill(pid: u32, force: bool) -> Result<()> {
    #[cfg(windows)]
    let status = {
        let _ = force;
        std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/F"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()?
    };
    #[cfg(not(windows))]
    let status = std::process::Command::new("kill")
        .args(if force {
            vec!["-9".to_string(), pid.to_string()]
        } else {
            vec![pid.to_string()]
        })
        .status()?;
    if !status.success() {
        bail!("could not stop process {}", pid);
    }
    Ok(())
}

/// The cyxcloud-node binary: CYXCLOUD_NODE_BIN, next to this cyxcloud, or on PATH.
pub fn node_binary() -> PathBuf {
    if let Ok(path) = std::env::var("CYXCLOUD_NODE_BIN") {
        return PathBuf::from(path);
    }
    let name = if cfg!(windows) {
        "cyxcloud-node.exe"
    } else {
        "cyxcloud-node"
    };
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join(name);
            if candidate.exists() {
                return candidate;
            }
        }
    }
    PathBuf::from(name)
}

fn tail_lines(path: &Path, n: usize) -> Vec<String> {
    let Ok(file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let lines: Vec<String> = std::io::BufReader::new(file)
        .lines()
        .map_while(|l| l.ok())
        .map(|l| strip_ansi(&l))
        .filter(|l| !l.to_lowercase().contains("password"))
        .collect();
    lines[lines.len().saturating_sub(n)..].to_vec()
}

// ============================================================================
// Commands
// ============================================================================

pub struct InitConfig {
    pub data_dir: Option<PathBuf>,
    pub offer: Option<String>,
    pub name: Option<String>,
    pub port: Option<u16>,
    pub status_port: Option<u16>,
    pub public_address: Option<String>,
    pub force: bool,
}

/// `cyxcloud node init`
pub async fn init(cfg: InitConfig) -> Result<()> {
    let creds = config::get_valid_credentials().context(
        "sign in first: `cyxcloud login --server HOST` (the node earns for the signed-in account)",
    )?;
    let shared = config::load_config();
    let config_path = node_config_path()?;
    if config_path.exists() && !cfg.force {
        println!(
            "{} A node is already set up here ({}).",
            style("!").yellow(),
            config_path.display()
        );
        println!("  Start it with `cyxcloud node start`, or run `cyxcloud node init --force` to set it up again.");
        return Ok(());
    }

    let interactive = console::user_attended() && cfg.data_dir.is_none() && cfg.offer.is_none();

    // Where
    let default_dir = node_dir()?.join("data");
    let data_dir = match cfg.data_dir {
        Some(d) => d,
        None if interactive => PathBuf::from(prompt(
            "Where should shards live?",
            &default_dir.to_string_lossy(),
        )?),
        None => default_dir,
    };
    std::fs::create_dir_all(&data_dir)
        .with_context(|| format!("cannot create {}", data_dir.display()))?;
    let (free, _) = disk_space_for(&data_dir);
    let free_gb = free >> 30;

    // How much
    let suggested = if free_gb > 0 {
        (free_gb * 9 / 10).clamp(1, 500)
    } else {
        100
    };
    let offer_gb = match cfg.offer {
        Some(o) => server::parse_offer_gb(&o).map_err(anyhow::Error::msg)?,
        None if interactive => {
            let hint = if free_gb > 0 {
                format!("How much space do you offer? ({} GB free)", free_gb)
            } else {
                "How much space do you offer?".to_string()
            };
            server::parse_offer_gb(&prompt(&hint, &format!("{} GB", suggested))?)
                .map_err(anyhow::Error::msg)?
        }
        None => suggested,
    };
    if free_gb > 0 && offer_gb > free_gb {
        bail!(
            "you offer {} GB but the disk holding {} has {} GB free",
            offer_gb,
            data_dir.display(),
            free_gb
        );
    }

    // Identity
    let key_path = node_dir()?.join("key");
    let key = cyxcloud_core::NodeKey::load_or_generate(&key_path)
        .map_err(|e| anyhow::anyhow!("node key: {}", e))?;
    cyxcloud_core::load_or_generate_access_secret(&node_dir()?.join("access_secret"))
        .map_err(|e| anyhow::anyhow!("node access secret: {}", e))?;

    // Where the gateway reaches this node
    let grpc_port = cfg.port.unwrap_or(DEFAULT_GRPC_PORT);
    let public_address = match cfg.public_address {
        Some(a) => a,
        None => local_address_towards(&shared.gateway.grpc_url)
            .unwrap_or_else(|| "127.0.0.1".to_string()),
    };
    let name = cfg.name.unwrap_or_else(|| {
        sysinfo::System::host_name().unwrap_or_else(|| "cyxcloud-node".to_string())
    });

    let setup = NodeSetup {
        name: name.clone(),
        data_dir: data_dir.clone(),
        offer_gb,
        grpc_port,
        public_address: public_address.clone(),
        status_port: cfg.status_port.unwrap_or(DEFAULT_STATUS_PORT),
        gateway_grpc: shared.gateway.grpc_url.clone(),
        api_url: shared.auth.api_url.clone(),
    };
    std::fs::create_dir_all(node_dir()?)?;
    std::fs::write(&config_path, node_config_toml(&setup))
        .with_context(|| format!("cannot write {}", config_path.display()))?;

    // The account the node earns for
    let creds_path = node_credentials_path()?;
    let existing = std::fs::read_to_string(&creds_path)
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok());
    let node_creds = node_credentials_json(existing.as_ref(), &creds, &key.node_id());
    cyxcloud_core::owner_only::write_owner_only(
        &creds_path,
        serde_json::to_string_pretty(&node_creds)?,
    )
    .context("cannot save the node's credentials")?;

    let short: String = key.node_id().chars().take(8).collect();
    println!(
        "{} Node {} ready · id {} · offers {} GB · server {}",
        style(symbols::CHECK).green(),
        style(&name).cyan(),
        short,
        offer_gb,
        shared.gateway.grpc_url
    );
    println!("  shards in {}", data_dir.display());
    println!(
        "  reachable at {}:{} (the server dials this)",
        public_address, grpc_port
    );
    println!("  key {} (owner-only)", key_path.display());
    println!("  Start it: cyxcloud node start");
    Ok(())
}

/// `cyxcloud node start`
pub async fn start(foreground: bool) -> Result<()> {
    let config_path = node_config_path()?;
    let port = configured_status_port()?;
    if let Some(r) = fetch_status(port).await {
        println!(
            "{} Node {} is already running (cyxcloud node status)",
            style("!").yellow(),
            r.name
        );
        return Ok(());
    }

    if !foreground && service::start_via_service()? {
        println!(
            "{} Starting the node through its start-at-boot service",
            style("*").blue()
        );
        return wait_until_registered(port, None).await;
    }

    let binary = node_binary();
    if foreground {
        let status = std::process::Command::new(&binary)
            .arg("--config")
            .arg(&config_path)
            .status()
            .with_context(|| format!("cannot run {}", binary.display()))?;
        if !status.success() {
            bail!("cyxcloud-node exited with {}", status);
        }
        return Ok(());
    }

    let log = log_path()?;
    if std::fs::metadata(&log)
        .map(|m| m.len() > 10 << 20)
        .unwrap_or(false)
    {
        let _ = std::fs::rename(&log, log.with_extension("log.1"));
    }
    let out = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log)?;
    let err = out.try_clone()?;
    let mut command = std::process::Command::new(&binary);
    command
        .arg("--config")
        .arg(&config_path)
        .stdin(std::process::Stdio::null())
        .stdout(out)
        .stderr(err);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
        // Windows hands every inheritable handle to the child; our own
        // stdout/stderr would then stay open in the daemon and a caller that
        // reads this command's output (a script, a pipe) would wait forever.
        stop_inheriting_std_handles();
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.spawn().with_context(|| {
        format!(
            "cannot start {} (install cyxcloud-node next to cyxcloud, or set CYXCLOUD_NODE_BIN)",
            binary.display()
        )
    })?;
    std::fs::write(pid_path()?, child.id().to_string())?;
    println!(
        "{} Starting the node (log: {})",
        style("*").blue(),
        log.display()
    );

    wait_until_registered(port, Some(&mut child)).await
}

/// Wait until the node answers and has registered, or say why not.
async fn wait_until_registered(
    port: u16,
    mut child: Option<&mut std::process::Child>,
) -> Result<()> {
    let log = log_path()?;
    let deadline = std::time::Instant::now() + Duration::from_secs(120);
    loop {
        if let Some(Ok(Some(code))) = child.as_mut().map(|c| c.try_wait()) {
            let _ = std::fs::remove_file(pid_path()?);
            eprintln!(
                "{} The node stopped right away ({}). Last log lines:",
                style(symbols::CROSS).red(),
                code
            );
            for line in tail_lines(&log, 15) {
                eprintln!("  {}", line);
            }
            std::process::exit(3);
        }
        if let Some(r) = fetch_status(port).await {
            if r.server.registered {
                println!(
                    "{} Node {} is running and registered with {}",
                    style(symbols::CHECK).green(),
                    style(&r.name).cyan(),
                    r.server.address
                );
                return Ok(());
            }
            if !r.server.last_error.is_empty() {
                println!(
                    "{} The node is running but not registered: {}",
                    style("!").yellow(),
                    r.server.last_error
                );
                println!("  It keeps retrying; see `cyxcloud node logs -f` or `cyxcloud doctor`.");
                return Ok(());
            }
        }
        if std::time::Instant::now() > deadline {
            println!(
                "{} The node is starting slowly (still opening its storage?). Check `cyxcloud node status` in a minute.",
                style("!").yellow()
            );
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// `cyxcloud node stop`
pub async fn stop() -> Result<()> {
    let port = configured_status_port()?;
    if service::stop_via_service()? {
        println!(
            "{} Node stopped (its start-at-boot service starts it again at the next boot or login)",
            style(symbols::CHECK).green()
        );
        return Ok(());
    }
    let Some(pid) = read_pid() else {
        if fetch_status(port).await.is_some() {
            bail!("a node is running but was not started by `cyxcloud node start` (no pid file); stop it where it was started");
        }
        println!("{} The node is not running", style("!").yellow());
        return Ok(());
    };
    if !is_our_node(pid) {
        let _ = std::fs::remove_file(pid_path()?);
        println!(
            "{} The node is not running (stale pid {} removed)",
            style("!").yellow(),
            pid
        );
        return Ok(());
    }
    kill(pid, false)?;
    for _ in 0..40 {
        if !is_our_node(pid) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    if is_our_node(pid) {
        kill(pid, true)?;
    }
    let _ = std::fs::remove_file(pid_path()?);
    println!("{} Node stopped", style(symbols::CHECK).green());
    Ok(())
}

/// `cyxcloud node status`
pub async fn status(watch: bool, json: bool) -> Result<()> {
    let port = configured_status_port()?;
    loop {
        let now = chrono::Utc::now().timestamp();
        match fetch_status(port).await {
            Some(r) if json => println!("{}", serde_json::to_string_pretty(&r)?),
            Some(r) => {
                if watch {
                    let _ = console::Term::stdout().clear_screen();
                }
                for (label, value) in status_lines(&r, now) {
                    if label.is_empty() {
                        let styled = if value.contains("ONLINE") {
                            style(value).green().bold()
                        } else {
                            style(value).yellow().bold()
                        };
                        println!("  {}", styled);
                    } else if label == "last error" {
                        println!("  {:<11} {}", style(label).red(), value);
                    } else {
                        println!("  {:<11} {}", style(label).dim(), value);
                    }
                }
                let boot = match service::installed() {
                    Some(service::Installed::Systemd) => {
                        "yes (systemd user unit, restarts on failure)"
                    }
                    Some(service::Installed::Launchd) => {
                        "at login (launchd agent, restarts on failure)"
                    }
                    Some(service::Installed::WindowsRunKey) => "at logon (Run key)",
                    None => "no · cyxcloud node service install",
                };
                println!("  {:<11} {}", style("at boot").dim(), boot);
            }
            None => {
                println!(
                    "{} The node is not running · start it with `cyxcloud node start`",
                    style("!").yellow()
                );
                if !watch {
                    std::process::exit(1);
                }
            }
        }
        if !watch {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

/// `cyxcloud node logs`
pub async fn logs(lines: usize, follow: bool) -> Result<()> {
    let path = log_path()?;
    if !path.exists() {
        println!(
            "{} No log yet ({}): the node has not been started with `cyxcloud node start`",
            style("!").yellow(),
            path.display()
        );
        return Ok(());
    }
    for line in tail_lines(&path, lines) {
        println!("{}", line);
    }
    if !follow {
        return Ok(());
    }
    let mut file = std::fs::File::open(&path)?;
    let mut pos = file.seek(std::io::SeekFrom::End(0))?;
    let mut pending = String::new();
    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let len = std::fs::metadata(&path)?.len();
        if len < pos {
            file = std::fs::File::open(&path)?;
            pos = 0;
        }
        if len == pos {
            continue;
        }
        file.seek(std::io::SeekFrom::Start(pos))?;
        let mut chunk = String::new();
        file.read_to_string(&mut chunk)?;
        pos = len;
        pending.push_str(&chunk);
        while let Some(i) = pending.find('\n') {
            let line: String = pending.drain(..=i).collect();
            let line = strip_ansi(line.trim_end());
            if !line.to_lowercase().contains("password") {
                println!("{}", line);
            }
        }
        std::io::stdout().flush()?;
    }
}

// ============================================================================
// Helpers
// ============================================================================

/// Mark this process's stdin/stdout/stderr handles non-inheritable, so a
/// detached child does not keep the caller's pipes open.
#[cfg(windows)]
fn stop_inheriting_std_handles() {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetStdHandle(n_std_handle: u32) -> isize;
        fn SetHandleInformation(h_object: isize, dw_mask: u32, dw_flags: u32) -> i32;
    }
    const HANDLE_FLAG_INHERIT: u32 = 0x0000_0001;
    // STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE
    for n in [-10i32, -11, -12] {
        // SAFETY: plain Win32 calls on this process's own standard handles;
        // invalid or absent handles are skipped and failures are harmless.
        unsafe {
            let handle = GetStdHandle(n as u32);
            if handle != 0 && handle != -1 {
                SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0);
            }
        }
    }
}

fn prompt(question: &str, default: &str) -> Result<String> {
    print!("  {} [{}] ", question, default);
    std::io::stdout().flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    let answer = answer.trim();
    Ok(if answer.is_empty() {
        default.to_string()
    } else {
        answer.to_string()
    })
}

/// This machine's address on the route to `url`'s host (what the server
/// should dial back). None when the route cannot be found.
pub fn local_address_towards(url: &str) -> Option<String> {
    let host = server::url_host(url)?;
    let port = server::url_port(url).unwrap_or(443);
    if host == "localhost" || host.starts_with("127.") {
        return Some("127.0.0.1".to_string());
    }
    let target = std::net::ToSocketAddrs::to_socket_addrs(&(host, port))
        .ok()?
        .find(|a| a.is_ipv4())?;
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect(target).ok()?;
    Some(socket.local_addr().ok()?.ip().to_string())
}

/// Free and total bytes on the disk holding `path` (0, 0 when unknown).
pub fn disk_space_for(path: &Path) -> (u64, u64) {
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let text = target.to_string_lossy().to_string();
    let target = PathBuf::from(text.strip_prefix(r"\\?\").unwrap_or(&text));
    let disks = sysinfo::Disks::new_with_refreshed_list();
    disks
        .list()
        .iter()
        .filter(|d| target.starts_with(d.mount_point()))
        .max_by_key(|d| d.mount_point().as_os_str().len())
        .map(|d| (d.available_space(), d.total_space()))
        .unwrap_or((0, 0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cyxcloud_core::node_status::{MachineFacts, ServerFacts, StorageFacts};

    fn hp_setup() -> NodeSetup {
        NodeSetup {
            name: "hp-linux".into(),
            data_dir: PathBuf::from("/home/mrcj/cyxwiz/node-data"),
            offer_gb: 700,
            grpc_port: 50051,
            public_address: "192.168.1.180".into(),
            status_port: 9090,
            gateway_grpc: "http://192.168.1.116:50060".into(),
            api_url: "http://192.168.1.116:3002".into(),
        }
    }

    #[test]
    fn the_written_config_is_valid_and_has_no_node_id() {
        let text = node_config_toml(&hp_setup());
        let v: toml::Value = toml::from_str(&text).unwrap();

        assert!(v["node"].get("id").is_none(), "the key decides the id");
        assert_eq!(v["node"]["name"].as_str(), Some("hp-linux"));
        assert_eq!(v["storage"]["max_capacity_gb"].as_integer(), Some(700));
        assert_eq!(
            v["storage"]["data_dir"].as_str(),
            Some("/home/mrcj/cyxwiz/node-data")
        );
        assert_eq!(
            v["network"]["public_address"].as_str(),
            Some("192.168.1.180")
        );
        assert_eq!(
            v["central"]["address"].as_str(),
            Some("http://192.168.1.116:50060")
        );
        assert_eq!(
            v["cyxwiz_api"]["base_url"].as_str(),
            Some("http://192.168.1.116:3002")
        );
        assert_eq!(status_port_of(&text), 9090);
    }

    #[test]
    fn windows_paths_and_quotes_survive_the_config() {
        let mut s = hp_setup();
        s.data_dir = PathBuf::from(r"D:\cyxcloud data");
        s.name = r#"my "pc""#.into();
        let v: toml::Value = toml::from_str(&node_config_toml(&s)).unwrap();
        assert_eq!(v["storage"]["data_dir"].as_str(), Some("D:/cyxcloud data"));
        assert_eq!(v["node"]["name"].as_str(), Some(r#"my "pc""#));
    }

    fn report(now: i64) -> NodeStatusReport {
        NodeStatusReport {
            version: "0.1.0".into(),
            node_id: "4856c6439493904b62111f6cc3af3c46".into(),
            name: "hp-linux".into(),
            listen_addr: "192.168.1.180:50051".into(),
            started_unix: now - (3 * 86_400 + 2 * 3600),
            machine: MachineFacts {
                hostname: "cyxwiz".into(),
                os: "Arch Linux 6.17".into(),
                cpu_model: "Intel(R) Core(TM) i7-2620M CPU @ 2.70GHz".into(),
                cpu_cores: 2,
                cpu_threads: 4,
                ram_bytes: 7_846 * 1024 * 1024,
            },
            storage: StorageFacts {
                data_dir: "/home/mrcj/cyxwiz/node-data".into(),
                offered_bytes: 700 << 30,
                used_bytes: 6_815_744,
                chunks: 42,
                disk_free_bytes: 830 << 30,
                disk_total_bytes: 915 << 30,
            },
            server: ServerFacts {
                address: "http://192.168.1.116:50060".into(),
                registered: true,
                last_heartbeat_unix: now - 12,
                last_error: String::new(),
            },
        }
    }

    #[test]
    fn an_online_node_reads_like_the_mockup() {
        let now = 1_800_000_000;
        let lines = status_lines(&report(now), now);
        let text: Vec<String> = lines.iter().map(|(l, v)| format!("{} {}", l, v)).collect();

        assert_eq!(text[0].trim(), "hp-linux · 4856c643 · ONLINE for 3 d 02 h");
        assert!(text.iter().any(|l| l.contains("i7-2620M")
            && l.contains("2 cores, 4 threads")
            && l.contains("7.7 GB RAM")));
        assert!(text
            .iter()
            .any(|l| l.contains("6.5 MB used of 700 GB offered") && l.contains("830 GB free")));
        assert!(text.iter().any(|l| l == "shards 42 held"));
        assert!(text.iter().any(|l| l.contains("heartbeat 12 s ago")));
        assert!(!text.iter().any(|l| l.starts_with("last error")));
    }

    #[test]
    fn a_node_that_lost_its_server_says_so_and_why() {
        let now = 1_800_000_000;
        let mut r = report(now);
        r.server.registered = false;
        r.server.last_heartbeat_unix = now - 600;
        r.server.last_error = "registration failed: connection refused".into();

        let lines = status_lines(&r, now);

        assert!(lines[0].1.ends_with("NOT REGISTERED"));
        assert!(lines
            .iter()
            .any(|(l, v)| l == "last error" && v.contains("connection refused")));
    }

    #[test]
    fn a_stale_heartbeat_is_not_online() {
        let now = 1_800_000_000;
        let mut r = report(now);
        r.server.last_heartbeat_unix = now - ONLINE_WITHIN_SECS - 1;
        assert!(!status_lines(&r, now)[0].1.contains("ONLINE"));
    }

    #[test]
    fn log_lines_lose_their_colours() {
        assert_eq!(
            strip_ansi("\u{1b}[2m2026-10-22T10:43:13Z\u{1b}[0m \u{1b}[32m INFO\u{1b}[0m started"),
            "2026-10-22T10:43:13Z  INFO started"
        );
        assert_eq!(strip_ansi("plain"), "plain");
    }

    fn creds(email: &str) -> config::Credentials {
        config::Credentials {
            access_token: "jwt".into(),
            refresh_token: None,
            expires_at: 0,
            user_id: "6ac29f910171abdbcfdf8a04".into(),
            email: email.into(),
            username: Some("e2e_user".into()),
        }
    }

    #[test]
    fn the_same_account_keeps_its_machine_registration() {
        let existing = serde_json::json!({
            "email": "e2e-user@local.test", "machine_id": "machine-579abb0b", "api_key": "k", "wallet_address": null
        });
        let j = node_credentials_json(Some(&existing), &creds("E2E-user@local.test"), "abc");
        assert_eq!(j["machine_id"], "machine-579abb0b");
        assert_eq!(j["api_key"], "k");
        assert_eq!(j["node_id"], "abc");
        assert_eq!(j["auth_token"], "jwt");
    }

    #[test]
    fn another_accounts_leftover_is_replaced() {
        // 2026-10-22: this PC still held test@example.com's file from an old test
        let existing = serde_json::json!({
            "email": "test@example.com", "machine_id": "machine-5582a38d", "api_key": "old", "node_id": "persistent-node-12345"
        });
        let j = node_credentials_json(Some(&existing), &creds("e2e-user@local.test"), "abc");
        assert_eq!(j["email"], "e2e-user@local.test");
        assert!(j["machine_id"].is_null());
        assert!(j["api_key"].is_null());
        assert_eq!(j["node_id"], "abc");
    }

    #[test]
    fn the_route_to_localhost_is_loopback() {
        assert_eq!(
            local_address_towards("http://localhost:50060").as_deref(),
            Some("127.0.0.1")
        );
    }
}
