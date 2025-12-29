//! Health checking and heartbeat service for CyxCloud storage node
//!
//! Periodically checks local health and reports to central server via gRPC.

use crate::command_executor::{CommandBatchSummary, CommandExecutor};
use crate::config::NodeConfig;
use crate::metrics::{HealthState, NodeMetrics};
use cyxcloud_protocol::node::{
    node_service_client::NodeServiceClient, HeartbeatRequest, NodeCapacity, NodeCommand, NodeInfo,
    NodeLocation, NodeMetrics as ProtoNodeMetrics, NodeStatus, RegisterNodeRequest,
};
use cyxcloud_storage::backend::StorageBackendSync;
use cyxcloud_storage::RocksDbBackend;
use std::sync::Arc;
use std::time::Duration;
use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};
use tokio::sync::RwLock;
use tonic::transport::Channel;
use tracing::{debug, error, info, warn};

/// Health checker that monitors node status
pub struct HealthChecker {
    node_id: String,
    storage: Arc<RocksDbBackend>,
    metrics: NodeMetrics,
    state: Arc<RwLock<HealthState>>,
    check_interval: Duration,
}

impl HealthChecker {
    /// Create a new health checker
    pub fn new(
        node_id: String,
        storage: Arc<RocksDbBackend>,
        metrics: NodeMetrics,
        state: Arc<RwLock<HealthState>>,
    ) -> Self {
        Self {
            node_id,
            storage,
            metrics,
            state,
            check_interval: Duration::from_secs(10),
        }
    }

    /// Start the health check loop
    pub async fn run(&self) {
        info!(node_id = %self.node_id, "Starting health checker");

        let mut interval = tokio::time::interval(self.check_interval);

        loop {
            interval.tick().await;

            let storage_ok = self.check_storage().await;
            let network_ok = self.check_network().await;

            // Update health state
            {
                let mut state = self.state.write().await;
                state.update(storage_ok, network_ok);
            }

            // Update storage metrics
            if let Ok(stats) = self.storage.stats() {
                self.metrics.update_storage(
                    stats.bytes_used,
                    stats.bytes_capacity.saturating_sub(stats.bytes_used),
                    stats.chunk_count,
                );
            }

            debug!(
                node_id = %self.node_id,
                storage_ok = storage_ok,
                network_ok = network_ok,
                "Health check completed"
            );
        }
    }

    /// Check storage backend health
    async fn check_storage(&self) -> bool {
        // Try to get stats - if this works, storage is accessible
        match self.storage.stats() {
            Ok(_) => true,
            Err(e) => {
                error!(error = %e, "Storage health check failed");
                false
            }
        }
    }

    /// Check network connectivity
    async fn check_network(&self) -> bool {
        // Test DNS resolution and TCP connectivity to well-known endpoints
        // Run checks concurrently
        let dns_check = self.check_dns("api.devnet.solana.com");
        let tcp_check = self.check_tcp_connect("api.devnet.solana.com:443");

        let (dns_ok, tcp_ok) = tokio::join!(dns_check, tcp_check);

        let passed = [dns_ok, tcp_ok].iter().filter(|&&r| r).count();

        if passed == 0 {
            warn!("All network connectivity checks failed");
            return false;
        }

        debug!(
            passed = passed,
            total = 2,
            dns_ok = dns_ok,
            tcp_ok = tcp_ok,
            "Network connectivity check completed"
        );
        true
    }

    /// Check DNS resolution
    async fn check_dns(&self, host: &str) -> bool {
        use tokio::net::lookup_host;

        match tokio::time::timeout(
            Duration::from_secs(5),
            lookup_host(format!("{}:443", host)),
        )
        .await
        {
            Ok(Ok(mut addrs)) => {
                if addrs.next().is_some() {
                    debug!(host = %host, "DNS resolution successful");
                    true
                } else {
                    warn!(host = %host, "DNS resolution returned no addresses");
                    false
                }
            }
            Ok(Err(e)) => {
                warn!(host = %host, error = %e, "DNS resolution failed");
                false
            }
            Err(_) => {
                warn!(host = %host, "DNS resolution timed out");
                false
            }
        }
    }

    /// Check TCP connectivity
    async fn check_tcp_connect(&self, addr: &str) -> bool {
        use tokio::net::TcpStream;

        match tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(addr)).await {
            Ok(Ok(_)) => {
                debug!(addr = %addr, "TCP connection successful");
                true
            }
            Ok(Err(e)) => {
                warn!(addr = %addr, error = %e, "TCP connection failed");
                false
            }
            Err(_) => {
                warn!(addr = %addr, "TCP connection timed out");
                false
            }
        }
    }
}

/// Heartbeat service for central server registration
pub struct HeartbeatService {
    node_id: String,
    config: NodeConfig,
    metrics: NodeMetrics,
    storage: Arc<RocksDbBackend>,
    grpc_address: String,
    client: RwLock<Option<NodeServiceClient<Channel>>>,
    auth_token: RwLock<Option<String>>,
    /// JWT token from CyxWiz API for Gateway authentication
    jwt_token: RwLock<Option<String>>,
    /// Wallet address from CyxWiz API login (takes priority over config)
    credentials_wallet: RwLock<Option<String>>,
    system: RwLock<System>,
    command_executor: CommandExecutor,
}

impl HeartbeatService {
    /// Create a new heartbeat service
    pub fn new(
        config: NodeConfig,
        metrics: NodeMetrics,
        storage: Arc<RocksDbBackend>,
    ) -> Self {
        let node_id = config.node.id.clone();
        let grpc_address = format!(
            "{}:{}",
            config.network.public_address
                .clone()
                .unwrap_or_else(|| config.network.bind_address.clone()),
            config.network.grpc_port
        );

        // Initialize system info for metrics collection
        let system = System::new_with_specifics(
            RefreshKind::new()
                .with_cpu(CpuRefreshKind::everything())
                .with_memory(MemoryRefreshKind::everything()),
        );

        // Initialize command executor
        let command_executor = CommandExecutor::new(
            node_id.clone(),
            storage.clone(),
            metrics.clone(),
        );

        Self {
            node_id,
            config,
            metrics,
            storage,
            grpc_address,
            client: RwLock::new(None),
            auth_token: RwLock::new(None),
            jwt_token: RwLock::new(None),
            credentials_wallet: RwLock::new(None),
            system: RwLock::new(system),
            command_executor,
        }
    }

    /// Set the JWT token for Gateway authentication
    /// This token comes from CyxWiz API login
    pub async fn set_jwt_token(&self, token: String) {
        let mut jwt = self.jwt_token.write().await;
        *jwt = Some(token);
        info!("JWT token set for Gateway authentication");
    }

    /// Check if JWT token is set
    pub async fn has_jwt_token(&self) -> bool {
        self.jwt_token.read().await.is_some()
    }

    /// Set the wallet address from CyxWiz API login
    /// This takes priority over config.node.wallet_address
    pub async fn set_credentials_wallet(&self, wallet: String) {
        let mut w = self.credentials_wallet.write().await;
        *w = Some(wallet.clone());
        info!(wallet = %wallet, "Wallet address set from credentials");
    }

    /// Get the effective wallet address for registration
    /// Priority: credentials wallet > config wallet
    async fn get_wallet_address(&self) -> String {
        // First try credentials wallet (from login)
        if let Some(wallet) = self.credentials_wallet.read().await.clone() {
            return wallet;
        }
        // Fall back to config wallet
        self.config.node.wallet_address.clone().unwrap_or_default()
    }

    /// Start the heartbeat loop
    pub async fn run(&self) {
        if !self.config.central.register {
            info!("Central server registration disabled, skipping heartbeat");
            return;
        }

        info!(
            node_id = %self.node_id,
            central_addr = %self.config.central.address,
            interval_secs = self.config.central.heartbeat_interval_secs,
            "Starting heartbeat service"
        );

        let mut interval =
            tokio::time::interval(Duration::from_secs(self.config.central.heartbeat_interval_secs));

        // Initial registration
        match self.register().await {
            Ok(()) => info!(node_id = %self.node_id, "Initial registration successful"),
            Err(e) => warn!(error = %e, "Failed initial registration with central server"),
        }

        loop {
            interval.tick().await;

            match self.send_heartbeat().await {
                Ok(()) => {
                    self.metrics.record_heartbeat(true);
                    debug!(node_id = %self.node_id, "Heartbeat sent successfully");
                }
                Err(e) => {
                    self.metrics.record_heartbeat(false);
                    warn!(
                        node_id = %self.node_id,
                        error = %e,
                        "Failed to send heartbeat, attempting re-registration"
                    );
                    // Try to re-register on heartbeat failure
                    if let Err(reg_err) = self.register().await {
                        warn!(error = %reg_err, "Re-registration also failed");
                    }
                }
            }
        }
    }

    /// Connect to the central server
    async fn connect(&self) -> Result<NodeServiceClient<Channel>, Box<dyn std::error::Error + Send + Sync>> {
        // Check if we already have a connection
        {
            let client = self.client.read().await;
            if client.is_some() {
                return Ok(client.clone().unwrap());
            }
        }

        // Create new connection
        let endpoint = tonic::transport::Endpoint::from_shared(self.config.central.address.clone())?
            .connect_timeout(Duration::from_secs(self.config.central.connect_timeout_secs));

        let channel = endpoint.connect().await?;
        let client = NodeServiceClient::new(channel);

        // Store the connection
        {
            let mut stored_client = self.client.write().await;
            *stored_client = Some(client.clone());
        }

        info!(central_addr = %self.config.central.address, "Connected to central server");
        Ok(client)
    }

    /// Create a gRPC request with JWT authorization header
    fn create_auth_request<T>(&self, inner: T, jwt_token: Option<&str>) -> tonic::Request<T> {
        let mut request = tonic::Request::new(inner);
        if let Some(token) = jwt_token {
            if let Ok(value) = format!("Bearer {}", token).parse() {
                request.metadata_mut().insert("authorization", value);
            }
        }
        request
    }

    /// Register with central server
    async fn register(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Get JWT token for authentication
        let jwt_token = self.jwt_token.read().await.clone();
        if jwt_token.is_none() {
            return Err("JWT token not set - login to CyxWiz API first".into());
        }

        info!(
            node_id = %self.node_id,
            central_addr = %self.config.central.address,
            grpc_addr = %self.grpc_address,
            "Registering with Gateway (using JWT auth)"
        );

        let mut client = self.connect().await?;

        // Get storage stats for capacity info
        let stats = self.storage.stats().unwrap_or_default();

        // Get wallet address (credentials wallet takes priority over config)
        let wallet_address = self.get_wallet_address().await;

        // Build registration request
        let register_req = RegisterNodeRequest {
            node_id: self.node_id.clone(),
            info: Some(NodeInfo {
                node_id: self.node_id.clone(),
                public_key: String::new(), // TODO: Add key support
                wallet_address: wallet_address.clone(),
                listen_addrs: vec![self.grpc_address.clone()],
                location: Some(NodeLocation {
                    datacenter: String::new(),
                    rack: 0,
                    region: self.config.node.region.clone().unwrap_or_default(),
                    latitude: 0.0,
                    longitude: 0.0,
                }),
                capacity: Some(NodeCapacity {
                    storage_total: stats.bytes_capacity,
                    storage_used: stats.bytes_used,
                    bandwidth_mbps: 1000, // Default assumption
                    max_connections: 100,
                }),
                status: NodeStatus::Online.into(),
                registered_at: chrono::Utc::now().timestamp(),
            }),
        };

        if !wallet_address.is_empty() {
            info!(wallet = %wallet_address, "Registering with wallet address");
        }

        // Create request with JWT auth header
        let request = self.create_auth_request(register_req, jwt_token.as_deref());

        let response = client.register_node(request).await?;
        let result = response.into_inner();

        if result.success {
            // Calculate storage info for display
            let total_gb = stats.bytes_capacity as f64 / (1024.0 * 1024.0 * 1024.0);
            let reserved_gb = 2.0; // Gateway reserves 2GB
            let available_gb = (total_gb - reserved_gb).max(0.0);

            info!(
                node_id = %self.node_id,
                storage_total_gb = format!("{:.1}", total_gb),
                storage_reserved_gb = format!("{:.1}", reserved_gb),
                storage_available_gb = format!("{:.1}", available_gb),
                "Node registered successfully with Gateway"
            );

            // Print user-friendly storage summary
            if total_gb > 0.0 {
                info!("========================================");
                info!("  Storage Reservation Summary");
                info!("========================================");
                info!("  Total Capacity:  {:.1} GB", total_gb);
                info!("  System Reserved: {:.1} GB", reserved_gb);
                info!("  Available:       {:.1} GB", available_gb);
                info!("========================================");
            } else {
                warn!("No storage capacity configured - set max_capacity_gb in config.toml");
            }

            // Store the gateway auth token for future requests (if any)
            {
                let mut token = self.auth_token.write().await;
                *token = Some(result.auth_token);
            }
            Ok(())
        } else {
            Err(format!("Registration failed: {}", result.error).into())
        }
    }

    /// Send heartbeat to central server
    async fn send_heartbeat(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Get JWT token for authentication
        let jwt_token = self.jwt_token.read().await.clone();
        if jwt_token.is_none() {
            return Err("JWT token not set - login to CyxWiz API first".into());
        }

        let mut client = self.connect().await?;

        // Get current stats
        let stats = self.storage.stats()?;

        // Collect system metrics (CPU and memory)
        let (cpu_usage, memory_usage) = {
            let mut sys = self.system.write().await;
            sys.refresh_cpu_all();
            sys.refresh_memory();

            // Calculate average CPU usage across all cores
            let cpu_avg = sys
                .cpus()
                .iter()
                .map(|cpu| cpu.cpu_usage() as f64)
                .sum::<f64>()
                / sys.cpus().len().max(1) as f64;

            // Calculate memory usage percentage
            let used_mem = sys.used_memory();
            let total_mem = sys.total_memory();
            let mem_percent = if total_mem > 0 {
                (used_mem as f64 / total_mem as f64) * 100.0
            } else {
                0.0
            };

            (cpu_avg, mem_percent)
        };

        // Build heartbeat request with metrics
        let heartbeat_req = HeartbeatRequest {
            node_id: self.node_id.clone(),
            metrics: Some(ProtoNodeMetrics {
                storage_used: stats.bytes_used,
                storage_available: stats.bytes_capacity.saturating_sub(stats.bytes_used),
                chunks_stored: stats.chunk_count,
                bytes_uploaded: self.metrics.get_bytes_uploaded(),
                bytes_downloaded: self.metrics.get_bytes_downloaded(),
                cpu_usage,
                memory_usage,
                active_connections: 0,
                last_updated: chrono::Utc::now().timestamp(),
            }),
        };

        // Create request with JWT auth header
        let request = self.create_auth_request(heartbeat_req, jwt_token.as_deref());

        let response = client.heartbeat(request).await?;
        let result = response.into_inner();

        if result.acknowledged {
            debug!(
                node_id = %self.node_id,
                bytes_used = stats.bytes_used,
                chunk_count = stats.chunk_count,
                bytes_uploaded = self.metrics.get_bytes_uploaded(),
                bytes_downloaded = self.metrics.get_bytes_downloaded(),
                cpu_usage = format!("{:.1}%", cpu_usage),
                memory_usage = format!("{:.1}%", memory_usage),
                "Heartbeat acknowledged"
            );

            // Process any commands from the server
            if !result.commands.is_empty() {
                info!(
                    node_id = %self.node_id,
                    command_count = result.commands.len(),
                    "Received commands from central server"
                );

                // Execute commands asynchronously
                self.execute_server_commands(result.commands).await;
            }

            Ok(())
        } else {
            Err("Heartbeat not acknowledged".into())
        }
    }

    /// Execute commands received from the central server
    async fn execute_server_commands(&self, commands: Vec<NodeCommand>) {
        let command_count = commands.len();

        info!(
            node_id = %self.node_id,
            command_count = command_count,
            "Executing server commands"
        );

        // Execute all commands
        let results = self.command_executor.execute_commands(commands).await;

        // Log summary
        let summary = CommandBatchSummary::from_results(&results);

        if summary.failed > 0 {
            warn!(
                node_id = %self.node_id,
                total = summary.total,
                successful = summary.successful,
                failed = summary.failed,
                repairs = summary.repairs,
                deletes = summary.deletes,
                transfers = summary.transfers,
                duration_ms = summary.total_duration.as_millis(),
                "Command batch completed with failures"
            );
        } else {
            info!(
                node_id = %self.node_id,
                total = summary.total,
                successful = summary.successful,
                repairs = summary.repairs,
                deletes = summary.deletes,
                transfers = summary.transfers,
                duration_ms = summary.total_duration.as_millis(),
                "Command batch completed successfully"
            );
        }

        // Log individual failures for debugging
        for result in &results {
            if !result.success {
                if let Some(ref error) = result.error {
                    error!(
                        node_id = %self.node_id,
                        command_type = ?result.command_type,
                        error = %error,
                        "Command failed"
                    );
                }
            }
        }
    }
}

/// Node announcer for P2P network
pub struct NodeAnnouncer {
    node_id: String,
    grpc_addr: String,
    storage: Arc<RocksDbBackend>,
}

impl NodeAnnouncer {
    /// Create a new node announcer
    pub fn new(node_id: String, grpc_addr: String, storage: Arc<RocksDbBackend>) -> Self {
        Self {
            node_id,
            grpc_addr,
            storage,
        }
    }

    /// Get node capacity info for announcements
    pub fn get_capacity(&self) -> NodeCapacity2 {
        let stats = self.storage.stats().unwrap_or_default();

        NodeCapacity2 {
            total_bytes: stats.bytes_capacity,
            used_bytes: stats.bytes_used,
            available_bytes: stats.bytes_capacity.saturating_sub(stats.bytes_used),
            chunk_count: stats.chunk_count,
        }
    }

    /// Get node announcement message
    pub fn get_announcement(&self) -> NodeAnnouncement {
        let capacity = self.get_capacity();

        NodeAnnouncement {
            node_id: self.node_id.clone(),
            grpc_addr: self.grpc_addr.clone(),
            capacity,
            status: NodeStatus2::Online,
        }
    }
}

/// Node capacity information (local struct, not proto)
#[derive(Debug, Clone, Default)]
pub struct NodeCapacity2 {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub available_bytes: u64,
    pub chunk_count: u64,
}

/// Node status (local enum)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeStatus2 {
    Online,
    Maintenance,
    Draining,
    Offline,
}

/// Node announcement message
#[derive(Debug, Clone)]
pub struct NodeAnnouncement {
    pub node_id: String,
    pub grpc_addr: String,
    pub capacity: NodeCapacity2,
    pub status: NodeStatus2,
}

#[cfg(test)]
mod tests {
    use super::*;
    use cyxcloud_storage::StorageConfig;
    use tempfile::TempDir;

    fn create_test_storage() -> (Arc<RocksDbBackend>, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let config = StorageConfig::new(temp_dir.path());
        let storage = Arc::new(RocksDbBackend::open(config).unwrap());
        (storage, temp_dir)
    }

    #[test]
    fn test_node_announcer() {
        let (storage, _dir) = create_test_storage();
        let announcer = NodeAnnouncer::new(
            "test-node".to_string(),
            "localhost:50051".to_string(),
            storage,
        );

        let announcement = announcer.get_announcement();
        assert_eq!(announcement.node_id, "test-node");
        assert_eq!(announcement.grpc_addr, "localhost:50051");
        assert_eq!(announcement.status, NodeStatus2::Online);
    }

    #[test]
    fn test_node_capacity() {
        let capacity = NodeCapacity2 {
            total_bytes: 1000,
            used_bytes: 300,
            available_bytes: 700,
            chunk_count: 10,
        };

        assert_eq!(capacity.total_bytes, 1000);
        assert_eq!(capacity.available_bytes, 700);
    }
}
