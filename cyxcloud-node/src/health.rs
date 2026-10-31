//! Health checking and heartbeat service for CyxCloud storage node
//!
//! Periodically checks local health and reports to central server via gRPC.

use crate::command_executor::{CommandBatchSummary, CommandExecutor};
use crate::config::NodeConfig;
use crate::metrics::{HealthState, NodeMetrics};
use cyxcloud_core::tls::{create_tonic_client_tls, TlsClientConfig};
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
    /// host:port of the gateway (the network check reaches it)
    gateway: String,
}

/// The host:port of a gateway address such as `http://gateway:50060`.
fn host_port(address: &str) -> String {
    let rest = address.split_once("://").map(|(_, r)| r).unwrap_or(address);
    let authority = rest.split('/').next().unwrap_or(rest);
    if authority
        .rsplit_once(':')
        .is_some_and(|(_, p)| p.parse::<u16>().is_ok())
    {
        authority.to_string()
    } else if address.starts_with("https://") {
        format!("{authority}:443")
    } else {
        format!("{authority}:80")
    }
}

impl HealthChecker {
    /// Create a new health checker; `gateway_address` is the gateway URL the
    /// node registers with (its network check reaches that, not a third
    /// party).
    pub fn new(
        node_id: String,
        storage: Arc<RocksDbBackend>,
        metrics: NodeMetrics,
        state: Arc<RwLock<HealthState>>,
        gateway_address: &str,
    ) -> Self {
        Self {
            gateway: host_port(gateway_address),
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

    /// Check network connectivity: the gateway this node serves can be
    /// reached (name resolution and a TCP connect, closed at once).
    async fn check_network(&self) -> bool {
        self.check_tcp_connect(&self.gateway).await
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
    /// The key that owns `node_id`; signs every registration
    key: cyxcloud_core::NodeKey,
    /// What the gateway must present on chunk RPCs to this node
    access_secret: String,
    config: NodeConfig,
    metrics: NodeMetrics,
    storage: Arc<RocksDbBackend>,
    grpc_address: String,
    client: RwLock<Option<NodeServiceClient<Channel>>>,
    auth_token: RwLock<Option<String>>,
    /// Wallet address from CyxWiz API login (takes priority over config)
    credentials_wallet: RwLock<Option<String>>,
    system: RwLock<System>,
    command_executor: CommandExecutor,
    /// Shared with the metrics server's /status
    health_state: Option<Arc<RwLock<HealthState>>>,
}

impl HeartbeatService {
    /// Create a new heartbeat service. Fails when the node key or access
    /// secret cannot be read or created, or when the configured node id is
    /// not the key's.
    pub fn new(
        config: NodeConfig,
        metrics: NodeMetrics,
        storage: Arc<RocksDbBackend>,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let key = cyxcloud_core::NodeKey::load_or_generate(&crate::config::node_key_path())?;
        let access_secret = cyxcloud_core::load_or_generate_access_secret(
            &crate::config::node_access_secret_path(),
        )?;
        let node_id = config.node.id.clone();
        if node_id != key.node_id() {
            return Err(format!(
                "node id {} in the configuration is not the id of the node key ({}); remove [node] id, the key decides",
                node_id,
                key.node_id()
            )
            .into());
        }
        let grpc_address = format!(
            "{}:{}",
            config
                .network
                .public_address
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
        let command_executor =
            CommandExecutor::new(node_id.clone(), storage.clone(), metrics.clone());

        Ok(Self {
            node_id,
            key,
            access_secret,
            config,
            metrics,
            storage,
            grpc_address,
            client: RwLock::new(None),
            auth_token: RwLock::new(None),
            credentials_wallet: RwLock::new(None),
            system: RwLock::new(system),
            command_executor,
            health_state: None,
        })
    }

    /// Keep this state's status report current (registration, heartbeats,
    /// storage, last error).
    pub fn with_health_state(mut self, state: Arc<RwLock<HealthState>>) -> Self {
        self.health_state = Some(state);
        self
    }

    async fn note(&self, update: impl FnOnce(&mut crate::status::NodeStatusReport)) {
        if let Some(state) = &self.health_state {
            update(&mut state.write().await.report);
        }
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

        let mut interval = tokio::time::interval(Duration::from_secs(
            self.config.central.heartbeat_interval_secs,
        ));

        // Initial registration
        match self.register().await {
            Ok(()) => {
                info!(node_id = %self.node_id, "Initial registration successful");
                self.note(|r| {
                    r.server.registered = true;
                    r.server.last_error.clear();
                })
                .await;
            }
            Err(e) => {
                warn!(error = %e, "Failed initial registration with central server");
                let text = format!("registration failed: {}", e);
                self.note(|r| {
                    r.server.registered = false;
                    r.server.last_error = text;
                })
                .await;
            }
        }

        loop {
            interval.tick().await;

            match self.send_heartbeat().await {
                Ok(()) => {
                    self.metrics.record_heartbeat(true);
                    debug!(node_id = %self.node_id, "Heartbeat sent successfully");
                    let now = chrono::Utc::now().timestamp();
                    self.note(|r| {
                        r.server.registered = true;
                        r.server.last_heartbeat_unix = now;
                        r.server.last_error.clear();
                    })
                    .await;
                }
                Err(e) => {
                    self.metrics.record_heartbeat(false);
                    warn!(
                        node_id = %self.node_id,
                        error = %e,
                        "Failed to send heartbeat, attempting re-registration"
                    );
                    // Try to re-register on heartbeat failure
                    match self.register().await {
                        Ok(()) => {
                            self.note(|r| {
                                r.server.registered = true;
                                r.server.last_error.clear();
                            })
                            .await
                        }
                        Err(reg_err) => {
                            warn!(error = %reg_err, "Re-registration also failed");
                            let text = format!(
                                "heartbeat failed: {}; re-registration failed: {}",
                                e, reg_err
                            );
                            self.note(|r| {
                                r.server.registered = false;
                                r.server.last_error = text;
                            })
                            .await;
                        }
                    }
                }
            }
        }
    }

    /// Connect to the central server
    async fn connect(
        &self,
    ) -> Result<NodeServiceClient<Channel>, Box<dyn std::error::Error + Send + Sync>> {
        // Check if we already have a connection
        {
            let client = self.client.read().await;
            if client.is_some() {
                return Ok(client.clone().unwrap());
            }
        }

        // Determine if TLS should be used (based on URL scheme or explicit config)
        let use_tls =
            self.config.central.address.starts_with("https://") || self.config.network.enable_tls;

        // Create new connection
        let mut endpoint =
            tonic::transport::Endpoint::from_shared(self.config.central.address.clone())?
                .connect_timeout(Duration::from_secs(
                    self.config.central.connect_timeout_secs,
                ));

        // Configure TLS if enabled
        if use_tls {
            if let Some(ref ca_cert_path) = self.config.network.tls_ca_cert {
                let tls_config = TlsClientConfig {
                    ca_cert_path: ca_cert_path.clone(),
                    client_cert_path: self.config.network.tls_client_cert.clone(),
                    client_key_path: self.config.network.tls_client_key.clone(),
                };

                match create_tonic_client_tls(&tls_config) {
                    Ok(tls) => {
                        endpoint = endpoint.tls_config(tls)?;
                        info!(
                            central_addr = %self.config.central.address,
                            mtls = self.config.network.tls_client_cert.is_some(),
                            "TLS configured for gateway connection"
                        );
                    }
                    Err(e) => {
                        error!(error = %e, "Failed to load TLS config for gateway connection");
                        return Err(format!("Failed to load TLS config: {}", e).into());
                    }
                }
            } else if self.config.central.address.starts_with("https://") {
                // An https:// server (a front door, TOFIX168) with a public
                // certificate: the system's trusted roots. This branch used
                // to log "using system roots" and then dial without TLS.
                endpoint = endpoint.tls_config(tonic::transport::ClientTlsConfig::new())?;
                info!(central_addr = %self.config.central.address, "TLS with the system's trusted roots");
            }
        }

        let channel = endpoint.connect().await?;
        let client = NodeServiceClient::new(channel);

        // Store the connection
        {
            let mut stored_client = self.client.write().await;
            *stored_client = Some(client.clone());
        }

        info!(
            central_addr = %self.config.central.address,
            tls = use_tls,
            "Connected to central server"
        );
        Ok(client)
    }

    /// A gRPC request carrying the node token from RegisterNode
    fn create_auth_request<T>(&self, inner: T, node_token: Option<&str>) -> tonic::Request<T> {
        let mut request = tonic::Request::new(inner);
        if let Some(token) = node_token {
            if let Ok(value) = format!("Bearer {}", token).parse() {
                request.metadata_mut().insert("authorization", value);
            }
        }
        request
    }

    /// Register with the gateway. The node key's signature is the credential
    /// (TOFIX176 P2): no account sign-in is needed, so a node keeps
    /// re-registering for as long as its key is on disk (TOFIX177).
    async fn register(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!(
            node_id = %self.node_id,
            central_addr = %self.config.central.address,
            grpc_addr = %self.grpc_address,
            "Registering with Gateway (signed by the node key)"
        );

        let mut client = self.connect().await?;

        // Get storage stats for capacity info
        let stats = self.storage.stats().unwrap_or_default();

        // Get wallet address (credentials wallet takes priority over config)
        let wallet_address = self.get_wallet_address().await;

        // Build registration request, signed by the node key
        let signed_at = chrono::Utc::now().timestamp();
        let register_req = RegisterNodeRequest {
            node_id: self.node_id.clone(),
            proof: Some(cyxcloud_protocol::node::NodeProof {
                timestamp: signed_at,
                signature: self.key.sign_registration(&self.grpc_address, signed_at),
            }),
            access_secret: self.access_secret.clone(),
            info: Some(NodeInfo {
                node_id: self.node_id.clone(),
                public_key: self.key.public_key_hex(),
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

        let request = tonic::Request::new(register_req);

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
        // Node RPCs carry the node token the gateway issued at registration;
        // without one the heartbeat fails and the caller re-registers.
        let node_token = self.auth_token.read().await.clone();
        if node_token.is_none() {
            return Err("not registered with the gateway yet (no node token)".into());
        }

        let mut client = self.connect().await?;

        // Get current stats
        let stats = self.storage.stats()?;
        let (disk_free, disk_total) = crate::status::disk_space_for(&self.config.storage.data_dir);
        self.note(|r| {
            r.storage.used_bytes = stats.bytes_used;
            r.storage.offered_bytes = stats.bytes_capacity;
            r.storage.chunks = stats.chunk_count;
            r.storage.disk_free_bytes = disk_free;
            r.storage.disk_total_bytes = disk_total;
        })
        .await;

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
        let request = self.create_auth_request(heartbeat_req, node_token.as_deref());

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

    #[test]
    fn the_network_check_targets_the_gateway_host_and_port() {
        assert_eq!(host_port("http://gateway:50060"), "gateway:50060");
        assert_eq!(host_port("https://cloud.cyxwiz.ai"), "cloud.cyxwiz.ai:443");
        assert_eq!(
            host_port("http://192.168.1.180:8000/"),
            "192.168.1.180:8000"
        );
        assert_eq!(host_port("gateway:50060"), "gateway:50060");
    }
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
