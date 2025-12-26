//! Machine Service for CyxWiz API integration
//!
//! Handles machine registration and heartbeats with the CyxWiz REST API.
//! This replaces the gRPC-based registration with the central Gateway.

use crate::config::NodeConfig;
use crate::cyxwiz_api_client::{CpuInfo, CyxWizApiClient, DetectedHardware, GpuInfo, SavedCredentials};
use crate::metrics::NodeMetrics;
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

        // Credentials file in data directory
        let credentials_path = config.storage.data_dir.join(".cyxwiz_credentials.json");

        Ok(Self {
            config,
            client: RwLock::new(client),
            metrics,
            system: RwLock::new(system),
            credentials_path,
            email: RwLock::new(None),
            username: RwLock::new(None),
        })
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
                        "Loaded saved credentials"
                    );

                    // Update client with saved credentials
                    let mut client = self.client.write().await;
                    client.load_credentials(creds.clone());

                    // Store email/username for later
                    *self.email.write().await = Some(creds.email.clone());
                    *self.username.write().await = Some(creds.username.clone());

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

    /// Save credentials to file
    pub async fn save_credentials(&self) -> Result<(), std::io::Error> {
        let client = self.client.read().await;
        let email = self.email.read().await;
        let username = self.username.read().await;

        if let (Some(email), Some(username)) = (email.as_ref(), username.as_ref()) {
            if let Some(creds) = client.get_credentials(email, username) {
                let content = serde_json::to_string_pretty(&creds)?;

                // Ensure parent directory exists
                if let Some(parent) = self.credentials_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }

                std::fs::write(&self.credentials_path, content)?;
                info!(path = ?self.credentials_path, "Credentials saved");
            }
        }

        Ok(())
    }

    /// Interactive login prompt
    pub async fn interactive_login(&self) -> Result<(), crate::cyxwiz_api_client::ApiError> {
        println!();
        println!("╔══════════════════════════════════════════════════════════╗");
        println!("║           CyxWiz Storage Node - Login Required           ║");
        println!("╚══════════════════════════════════════════════════════════╝");
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

        drop(client);

        println!();
        println!("✓ Login successful!");
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

    /// Run the machine service (login + registration + heartbeat loop)
    pub async fn run(&self) {
        if !self.config.cyxwiz_api.register {
            info!("CyxWiz API registration disabled, skipping machine service");
            return;
        }

        info!(
            api_url = %self.config.cyxwiz_api.base_url,
            "Starting machine service"
        );

        // Try to load saved credentials first
        let has_credentials = if let Some(creds) = self.load_saved_credentials().await {
            if creds.machine_id.is_some() && creds.api_key.is_some() {
                info!(
                    email = %creds.email,
                    machine_id = ?creds.machine_id,
                    "Loaded existing credentials - machine already registered"
                );
                true
            } else {
                info!(
                    email = %creds.email,
                    "Loaded saved login - machine needs registration"
                );
                false
            }
        } else {
            false
        };

        // If not logged in, prompt for login
        if !self.is_logged_in().await {
            loop {
                match self.interactive_login().await {
                    Ok(()) => break,
                    Err(e) => {
                        eprintln!();
                        eprintln!("✗ Login failed: {}", e);
                        eprintln!();
                        eprintln!("Please try again.");
                        eprintln!();
                    }
                }
            }
        }

        // Register machine if not already registered
        if !has_credentials {
            println!("Registering machine...");
            match self.register().await {
                Ok(()) => {
                    println!("✓ Machine registered successfully!");
                    println!();
                    info!("Machine registration complete");
                }
                Err(e) => {
                    error!(error = %e, "Failed to register machine");
                    eprintln!("✗ Failed to register machine: {}", e);
                    return;
                }
            }
        }

        println!("========================================");
        println!("  Machine service running");
        println!("  Sending heartbeats every {} seconds", self.config.central.heartbeat_interval_secs);
        println!("========================================");

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
}
