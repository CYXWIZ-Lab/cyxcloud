//! Machine Service for CyxWiz API integration
//!
//! Handles machine registration and heartbeats with the CyxWiz REST API.
//! This replaces the gRPC-based registration with the central Gateway.

use crate::config::{self, NodeConfig};
use crate::cyxwiz_api_client::{CpuInfo, CyxWizApiClient, DetectedHardware, GpuInfo, SavedCredentials};
use crate::metrics::NodeMetrics;
use crate::symbols;
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::Duration;
use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

/// Machine service for CyxWiz API integration
pub struct MachineService {
    config: NodeConfig,
    client: RwLock<CyxWizApiClient>,
    metrics: NodeMetrics,
    system: RwLock<System>,
    credentials_path: PathBuf,
    email: RwLock<Option<String>>,
    username: RwLock<Option<String>>,
    /// JWT token for Gateway authentication (shared with HeartbeatService)
    jwt_token: RwLock<Option<String>>,
}

impl MachineService {
    /// Create a new machine service
    pub fn new(config: NodeConfig, metrics: NodeMetrics) -> Result<Self, crate::cyxwiz_api_client::ApiError> {
        let client = CyxWizApiClient::new(&config.cyxwiz_api)?;

        // Initialize system info for hardware detection
        let system = System::new_with_specifics(
            RefreshKind::new()
                .with_cpu(CpuRefreshKind::everything())
                .with_memory(MemoryRefreshKind::everything()),
        );

        // Credentials file in standard location (~/.cyxcloud/)
        let credentials_path = config::node_credentials_path();

        Ok(Self {
            config,
            client: RwLock::new(client),
            metrics,
            system: RwLock::new(system),
            credentials_path,
            email: RwLock::new(None),
            username: RwLock::new(None),
            jwt_token: RwLock::new(None),
        })
    }

    /// Get the JWT token for Gateway authentication
    pub async fn get_jwt_token(&self) -> Option<String> {
        self.jwt_token.read().await.clone()
    }

    /// Check if we have a valid JWT token
    pub async fn has_jwt_token(&self) -> bool {
        self.jwt_token.read().await.is_some()
    }

    /// Check if we have saved credentials
    pub fn has_saved_credentials(&self) -> bool {
        self.credentials_path.exists()
    }

    /// Load saved credentials from file
    pub async fn load_saved_credentials(&self) -> Option<SavedCredentials> {
        if !self.credentials_path.exists() {
            return None;
        }

        match std::fs::read_to_string(&self.credentials_path) {
            Ok(content) => match serde_json::from_str::<SavedCredentials>(&content) {
                Ok(creds) => {
                    info!(
                        email = %creds.email,
                        username = %creds.username,
                        node_id = ?creds.node_id,
                        "Loaded saved credentials"
                    );

                    // Update client with saved credentials
                    let mut client = self.client.write().await;
                    client.load_credentials(creds.clone());

                    // Store email/username for later
                    *self.email.write().await = Some(creds.email.clone());
                    *self.username.write().await = Some(creds.username.clone());

                    // Store JWT token for Gateway authentication
                    *self.jwt_token.write().await = Some(creds.auth_token.clone());

                    Some(creds)
                }
                Err(e) => {
                    warn!(error = %e, "Failed to parse saved credentials");
                    None
                }
            },
            Err(e) => {
                warn!(error = %e, "Failed to read credentials file");
                None
            }
        }
    }

    /// Save credentials to file (includes persistent node_id)
    pub async fn save_credentials(&self) -> Result<(), std::io::Error> {
        let client = self.client.read().await;
        let email = self.email.read().await;
        let username = self.username.read().await;

        if let (Some(email), Some(username)) = (email.as_ref(), username.as_ref()) {
            // Include node_id from config for persistence across restarts
            let node_id = Some(self.config.node.id.clone());
            if let Some(creds) = client.get_credentials(email, username, node_id) {
                let content = serde_json::to_string_pretty(&creds)?;

                // Ensure ~/.cyxcloud/ directory exists
                let config_dir = config::cyxcloud_config_dir();
                std::fs::create_dir_all(&config_dir)?;

                std::fs::write(&self.credentials_path, content)?;
                info!(
                    path = ?self.credentials_path,
                    node_id = %self.config.node.id,
                    "Credentials saved (node ID persisted)"
                );
            }
        }

        Ok(())
    }

    /// Non-interactive login using email/password from config (environment variables)
    pub async fn non_interactive_login(&self) -> Result<(), crate::cyxwiz_api_client::ApiError> {
        let email = self.config.cyxwiz_api.email.as_ref().ok_or_else(|| {
            crate::cyxwiz_api_client::ApiError::ApiError(
                "CYXWIZ_EMAIL environment variable not set".to_string(),
            )
        })?;

        let password = self.config.cyxwiz_api.password.as_ref().ok_or_else(|| {
            crate::cyxwiz_api_client::ApiError::ApiError(
                "CYXWIZ_PASSWORD environment variable not set".to_string(),
            )
        })?;

        info!(email = %email, "Performing non-interactive login");

        // Perform login
        let mut client = self.client.write().await;
        let result = client.login(email, password).await?;

        // Store email/username
        *self.email.write().await = Some(result.user.email.clone());
        *self.username.write().await = Some(result.user.username.clone());

        // Store JWT token for Gateway authentication
        *self.jwt_token.write().await = Some(result.token.clone());

        drop(client);

        info!(
            email = %result.user.email,
            username = %result.user.username,
            "Non-interactive login successful"
        );

        // Save credentials
        if let Err(e) = self.save_credentials().await {
            warn!(error = %e, "Failed to save credentials");
        }

        Ok(())
    }

    /// Interactive login prompt
    pub async fn interactive_login(&self) -> Result<(), crate::cyxwiz_api_client::ApiError> {
        println!();
        println!("{}", symbols::BOX_TOP);
        println!("{}           CyxWiz Storage Node - Login Required           {}", symbols::BOX_SIDE, symbols::BOX_SIDE);
        println!("{}", symbols::BOX_BOTTOM);
        println!();
        println!("Please login with your CyxWiz account to register this node.");
        println!();

        // Get email
        print!("Email: ");
        io::stdout().flush().unwrap();
        let mut email = String::new();
        io::stdin().read_line(&mut email).unwrap();
        let email = email.trim().to_string();

        // Get password (hidden input)
        let password = rpassword::prompt_password("Password: ")
            .map_err(|e| crate::cyxwiz_api_client::ApiError::ApiError(format!("Failed to read password: {}", e)))?;

        println!();
        println!("Logging in...");

        // Perform login
        let mut client = self.client.write().await;
        let result = client.login(&email, &password).await?;

        // Store email/username
        *self.email.write().await = Some(result.user.email.clone());
        *self.username.write().await = Some(result.user.username.clone());

        // Store JWT token for Gateway authentication
        *self.jwt_token.write().await = Some(result.token.clone());

        drop(client);

        println!();
        println!("{} Login successful!", symbols::CHECK);
        println!("  Welcome, {}!", result.user.name.as_deref().unwrap_or(&result.user.username));
        println!();

        // Save credentials
        if let Err(e) = self.save_credentials().await {
            warn!(error = %e, "Failed to save credentials");
        }

        Ok(())
    }

    /// Check if client is logged in
    pub async fn is_logged_in(&self) -> bool {
        let client = self.client.read().await;
        client.is_logged_in()
    }

    /// Logout and clear credentials
    pub async fn logout(&self) -> Result<(), std::io::Error> {
        // Clear in-memory state
        *self.jwt_token.write().await = None;
        *self.email.write().await = None;
        *self.username.write().await = None;

        // Clear client state
        let mut client = self.client.write().await;
        client.set_credentials(String::new(), String::new());
        drop(client);

        // Remove credentials file
        if self.credentials_path.exists() {
            std::fs::remove_file(&self.credentials_path)?;
            info!(path = ?self.credentials_path, "Credentials file removed");
        }

        info!("Logged out successfully");
        Ok(())
    }

    /// Get user ID for Gateway registration
    pub async fn get_user_id(&self) -> Option<String> {
        let client = self.client.read().await;
        client.user_id().map(|s| s.to_string())
    }

    /// Get email for display
    pub async fn get_email(&self) -> Option<String> {
        self.email.read().await.clone()
    }

    /// Get username for display
    pub async fn get_username(&self) -> Option<String> {
        self.username.read().await.clone()
    }

    /// Get node ID from config
    pub fn get_node_id(&self) -> &str {
        &self.config.node.id
    }

    /// Get wallet address from config (if set)
    pub fn get_wallet_address(&self) -> Option<&str> {
        self.config.node.wallet_address.as_deref()
    }

    /// Get region from config (if set)
    pub fn get_region(&self) -> Option<&str> {
        self.config.node.region.as_deref()
    }

    /// Check if running in a TTY (interactive terminal)
    fn is_interactive(&self) -> bool {
        atty::is(atty::Stream::Stdin) && atty::is(atty::Stream::Stdout)
    }

    /// Ensure user is logged in (load saved credentials or prompt for login)
    /// Returns true if already had saved credentials with machine registered
    ///
    /// Login priority:
    /// 1. Saved credentials (from previous login)
    /// 2. Environment variables (CYXWIZ_EMAIL, CYXWIZ_PASSWORD) - non-interactive
    /// 3. Interactive prompt (only if TTY available)
    pub async fn ensure_logged_in(&self) -> Result<bool, crate::cyxwiz_api_client::ApiError> {
        // 1. Try to load saved credentials first
        if let Some(creds) = self.load_saved_credentials().await {
            if creds.machine_id.is_some() && creds.api_key.is_some() {
                info!(
                    email = %creds.email,
                    machine_id = ?creds.machine_id,
                    "Loaded existing credentials - machine already registered"
                );
                return Ok(true); // Already registered
            }
            info!(
                email = %creds.email,
                "Loaded saved login - machine needs registration"
            );
            return Ok(false); // Logged in but not registered
        }

        // 2. Try environment variable credentials (non-interactive login)
        if self.config.cyxwiz_api.has_credentials() {
            info!("Attempting non-interactive login using environment variables");
            match self.non_interactive_login().await {
                Ok(()) => return Ok(false), // Logged in, not registered yet
                Err(e) => {
                    error!(error = %e, "Non-interactive login failed");
                    // Fall through to interactive if available
                }
            }
        }

        // 3. Fall back to interactive login (if TTY available)
        if self.is_interactive() {
            loop {
                match self.interactive_login().await {
                    Ok(()) => return Ok(false), // Logged in, not registered yet
                    Err(e) => {
                        eprintln!();
                        eprintln!("{} Login failed: {}", symbols::CROSS, e);
                        eprintln!();
                        eprintln!("Please try again.");
                        eprintln!();
                    }
                }
            }
        }

        // No TTY and no valid credentials - cannot proceed
        Err(crate::cyxwiz_api_client::ApiError::ApiError(
            "No valid credentials available. Set CYXWIZ_EMAIL and CYXWIZ_PASSWORD environment variables for non-interactive login.".to_string(),
        ))
    }

    /// Detect hardware information
    pub async fn detect_hardware(&self) -> DetectedHardware {
        let mut sys = self.system.write().await;
        sys.refresh_cpu_all();
        sys.refresh_memory();

        // Collect CPU info
        let cpus: Vec<CpuInfo> = sys
            .cpus()
            .iter()
            .enumerate()
            .filter(|(i, _)| *i == 0) // Just get the first CPU info (they're all the same model)
            .map(|(_, cpu)| CpuInfo {
                name: cpu.brand().to_string(),
                cores: sys.cpus().len() as u32,
                threads: sys.cpus().len() as u32, // sysinfo doesn't distinguish cores/threads
                frequency_mhz: Some(cpu.frequency() as u32),
            })
            .collect();

        // For GPUs, we'd need a separate library (nvml for NVIDIA, etc.)
        // For now, return empty - the API will handle machines without GPUs
        let gpus: Vec<GpuInfo> = Vec::new();

        // Get total RAM
        let ram_total_mb = sys.total_memory() / (1024 * 1024);

        DetectedHardware {
            cpus,
            gpus,
            ram_total_mb,
        }
    }

    /// Get OS information
    pub fn get_os_info(&self) -> String {
        System::name()
            .map(|name| {
                let version = System::os_version().unwrap_or_default();
                format!("{} {}", name, version)
            })
            .unwrap_or_else(|| "Unknown".to_string())
    }

    /// Get hostname
    pub fn get_hostname(&self) -> String {
        System::host_name().unwrap_or_else(|| self.config.node.name.clone())
    }

    /// Register machine with CyxWiz API
    /// Requires login first
    pub async fn register(&self) -> Result<(), crate::cyxwiz_api_client::ApiError> {
        let hostname = self.get_hostname();
        let os = self.get_os_info();
        let hardware = self.detect_hardware().await;

        info!(
            hostname = hostname,
            os = os,
            cpus = hardware.cpus.len(),
            gpus = hardware.gpus.len(),
            ram_mb = hardware.ram_total_mb,
            "Registering machine with CyxWiz API"
        );

        let mut client = self.client.write().await;
        let result = client
            .register_machine(&hostname, &os, hardware)
            .await?;

        drop(client);

        info!(
            machine_id = %result.machine.machine_id,
            compute_units = result.compute_units.len(),
            "Machine registered successfully with CyxWiz API"
        );

        // Save updated credentials (with machine_id and api_key)
        if let Err(e) = self.save_credentials().await {
            warn!(error = %e, "Failed to save credentials after registration");
        }

        Ok(())
    }

    /// Send heartbeat to CyxWiz API
    pub async fn heartbeat(&self, is_online: bool) -> Result<(), crate::cyxwiz_api_client::ApiError> {
        let client = self.client.read().await;

        if !client.is_registered() {
            warn!("Cannot send heartbeat - machine not registered");
            return Err(crate::cyxwiz_api_client::ApiError::NotConfigured(
                "Machine not registered".to_string(),
            ));
        }

        // Get current CPU load
        let current_load = {
            let mut sys = self.system.write().await;
            sys.refresh_cpu_all();

            let avg_load: f32 = sys.cpus().iter().map(|c| c.cpu_usage()).sum::<f32>()
                / sys.cpus().len().max(1) as f32;

            avg_load as u8
        };

        // Get local IP address (simplified - in production would be more robust)
        let ip_address = local_ip_address::local_ip()
            .map(|ip| ip.to_string())
            .ok();

        let version = Some(env!("CARGO_PKG_VERSION").to_string());

        client
            .heartbeat(ip_address.as_deref(), version.as_deref(), is_online, Some(current_load))
            .await?;

        self.metrics.record_heartbeat(true);
        debug!(load = current_load, "Heartbeat sent to CyxWiz API");

        Ok(())
    }

    /// Check if machine is registered
    pub async fn is_registered(&self) -> bool {
        let client = self.client.read().await;
        client.is_registered()
    }

    /// Get machine ID
    pub async fn machine_id(&self) -> Option<String> {
        let client = self.client.read().await;
        client.machine_id().map(|s| s.to_string())
    }

    /// Run the machine service heartbeat loop
    /// NOTE: Call ensure_logged_in() before starting this service
    /// The already_registered param indicates if machine is already registered with CyxWiz API
    pub async fn run(&self, already_registered: bool) {
        if !self.config.cyxwiz_api.register {
            info!("CyxWiz API registration disabled, skipping machine service");
            return;
        }

        info!(
            api_url = %self.config.cyxwiz_api.base_url,
            "Starting machine service"
        );

        // Register machine if not already registered
        if !already_registered {
            if self.is_interactive() {
                println!("Registering machine with CyxWiz API...");
            }
            match self.register().await {
                Ok(()) => {
                    if self.is_interactive() {
                        println!("{} Machine registered successfully!", symbols::CHECK);
                        println!();
                    }
                    info!("Machine registration complete");
                }
                Err(e) => {
                    error!(error = %e, "Failed to register machine");
                    if self.is_interactive() {
                        eprintln!("{} Failed to register machine: {}", symbols::CROSS, e);
                    }
                    return;
                }
            }
        }

        if self.is_interactive() {
            println!("========================================");
            println!("  Machine service running");
            println!("  Sending heartbeats every {} seconds", self.config.central.heartbeat_interval_secs);
            println!("========================================");
        }

        // Heartbeat loop
        let interval_secs = self.config.central.heartbeat_interval_secs;
        let mut interval = tokio::time::interval(Duration::from_secs(interval_secs));

        loop {
            interval.tick().await;

            match self.heartbeat(true).await {
                Ok(()) => {
                    debug!("Heartbeat to CyxWiz API successful");
                }
                Err(crate::cyxwiz_api_client::ApiError::AuthFailed(_)) => {
                    warn!("Heartbeat auth failed, attempting re-registration");
                    if let Err(e) = self.register().await {
                        error!(error = %e, "Re-registration failed");
                    }
                }
                Err(crate::cyxwiz_api_client::ApiError::NotConfigured(_)) => {
                    // Try to register first
                    if let Err(e) = self.register().await {
                        warn!(error = %e, "Registration attempt failed");
                    }
                }
                Err(e) => {
                    warn!(error = %e, "Heartbeat to CyxWiz API failed");
                    self.metrics.record_heartbeat(false);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_os_info() {
        // Just verify it doesn't panic
        let name = System::name();
        let version = System::os_version();
        println!("OS: {:?} {:?}", name, version);
    }

    #[test]
    fn test_hostname() {
        let hostname = System::host_name();
        println!("Hostname: {:?}", hostname);
    }

    #[test]
    fn test_tty_detection() {
        // Just verify it doesn't panic
        let is_stdin_tty = atty::is(atty::Stream::Stdin);
        let is_stdout_tty = atty::is(atty::Stream::Stdout);
        println!("Stdin TTY: {}, Stdout TTY: {}", is_stdin_tty, is_stdout_tty);
    }
}
