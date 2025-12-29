//! Configuration management for CyxCloud storage node
//!
//! Supports loading from TOML files and environment variables.

use serde::{Deserialize, Serialize};
use serde_json;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Configuration errors
#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("Failed to read config file: {0}")]
    ReadError(#[from] std::io::Error),

    #[error("Failed to parse config file: {0}")]
    ParseError(#[from] toml::de::Error),

    #[error("Invalid configuration: {0}")]
    ValidationError(String),
}

/// Complete node configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeConfig {
    /// Node identity configuration
    #[serde(default)]
    pub node: NodeIdentity,

    /// Storage configuration
    #[serde(default)]
    pub storage: StorageSettings,

    /// Network configuration
    #[serde(default)]
    pub network: NetworkSettings,

    /// Metrics configuration
    #[serde(default)]
    pub metrics: MetricsSettings,

    /// Central server connection (Gateway for chunk storage)
    #[serde(default)]
    pub central: CentralServerSettings,

    /// CyxWiz API connection (for auth, machines, wallets)
    #[serde(default)]
    pub cyxwiz_api: CyxWizApiSettings,

    /// Blockchain (Solana) integration for staking and rewards
    #[serde(default)]
    pub blockchain: BlockchainSettings,
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            node: NodeIdentity::default(),
            storage: StorageSettings::default(),
            network: NetworkSettings::default(),
            metrics: MetricsSettings::default(),
            central: CentralServerSettings::default(),
            cyxwiz_api: CyxWizApiSettings::default(),
            blockchain: BlockchainSettings::default(),
        }
    }
}

impl NodeConfig {
    /// Load configuration from a TOML file
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let content = std::fs::read_to_string(path)?;
        let config: NodeConfig = toml::from_str(&content)?;
        config.validate()?;
        Ok(config)
    }

    /// Load configuration with fallback to defaults
    pub fn load_or_default(path: impl AsRef<Path>) -> Self {
        match Self::from_file(path) {
            Ok(config) => config,
            Err(e) => {
                tracing::warn!(error = %e, "Failed to load config, using defaults");
                Self::default()
            }
        }
    }

    /// Apply values from the shared config file (~/.cyxcloud/config.toml)
    /// This allows users to configure gateway/auth URLs in one place
    pub fn with_shared_config(mut self) -> Self {
        let shared = load_shared_config();

        // Only override if the node config still has default values
        if self.central.address == default_central_addr() {
            self.central.address = shared.gateway.grpc_url;
        }
        if self.cyxwiz_api.base_url == default_cyxwiz_api_url() {
            self.cyxwiz_api.base_url = shared.auth.api_url;
        }
        if self.blockchain.rpc_url == default_solana_rpc() {
            self.blockchain.rpc_url = shared.blockchain.solana_rpc_url;
        }

        self
    }

    /// Validate the configuration
    pub fn validate(&self) -> Result<(), ConfigError> {
        // Validate storage path is writable
        if !self.storage.data_dir.exists() {
            std::fs::create_dir_all(&self.storage.data_dir).map_err(|e| {
                ConfigError::ValidationError(format!(
                    "Cannot create data directory {:?}: {}",
                    self.storage.data_dir, e
                ))
            })?;
        }

        // Validate ports are sensible
        if self.network.grpc_port == 0 {
            return Err(ConfigError::ValidationError(
                "gRPC port cannot be 0".to_string(),
            ));
        }

        Ok(())
    }

    /// Override config with CLI arguments
    pub fn with_overrides(
        mut self,
        data_dir: Option<PathBuf>,
        port: Option<u16>,
    ) -> Self {
        if let Some(dir) = data_dir {
            self.storage.data_dir = dir;
        }
        if let Some(p) = port {
            self.network.grpc_port = p;
            self.network.p2p_port = p + 1;
        }
        self
    }

    /// Apply environment variable overrides to all settings
    pub fn with_env_overrides(mut self) -> Self {
        self.cyxwiz_api = self.cyxwiz_api.with_env_overrides();

        // Central server (Gateway) address
        if let Ok(addr) = std::env::var("CENTRAL_SERVER_ADDR") {
            self.central.address = addr;
        }

        // Storage capacity override (in GB)
        if let Ok(capacity) = std::env::var("STORAGE_CAPACITY_GB") {
            if let Ok(gb) = capacity.parse::<u64>() {
                self.storage.max_capacity_gb = gb;
            }
        }

        // Node ID override
        if let Ok(id) = std::env::var("NODE_ID") {
            self.node.id = id;
        }

        // Node name override
        if let Ok(name) = std::env::var("NODE_NAME") {
            self.node.name = name;
        }

        // Node region override
        if let Ok(region) = std::env::var("NODE_REGION") {
            self.node.region = Some(region);
        }

        // Public address override (for Docker/cloud networking)
        if let Ok(addr) = std::env::var("PUBLIC_ADDRESS") {
            self.network.public_address = Some(addr);
        }

        self
    }
}

/// Node identity configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeIdentity {
    /// Unique node identifier (auto-generated if not provided)
    #[serde(default = "generate_node_id")]
    pub id: String,

    /// Human-readable node name
    #[serde(default = "default_node_name")]
    pub name: String,

    /// Node region for topology-aware placement
    #[serde(default)]
    pub region: Option<String>,

    /// Node zone within region
    #[serde(default)]
    pub zone: Option<String>,

    /// Operator wallet address (Solana) for payments
    #[serde(default)]
    pub wallet_address: Option<String>,
}

impl Default for NodeIdentity {
    fn default() -> Self {
        Self {
            id: generate_node_id(),
            name: default_node_name(),
            region: None,
            zone: None,
            wallet_address: None,
        }
    }
}

/// Generate or load persistent node ID.
/// Checks ~/.cyxcloud/node_credentials.json first, generates new UUID if not found.
fn generate_node_id() -> String {
    // Try to load saved node ID from credentials file
    let creds_path = node_credentials_path();
    if creds_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&creds_path) {
            // Parse JSON and extract node_id field
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
                if let Some(node_id) = json.get("node_id").and_then(|v| v.as_str()) {
                    if !node_id.is_empty() {
                        tracing::debug!(node_id = %node_id, "Loaded persistent node ID from credentials");
                        return node_id.to_string();
                    }
                }
            }
        }
    }

    // Generate new node ID
    let new_id = uuid::Uuid::new_v4().to_string();
    tracing::info!(node_id = %new_id, "Generated new node ID (will be persisted on login)");
    new_id
}

fn default_node_name() -> String {
    hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_else(|_| "cyxcloud-node".to_string())
}

/// Storage configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageSettings {
    /// Directory for chunk data storage
    #[serde(default = "default_data_dir")]
    pub data_dir: PathBuf,

    /// Maximum storage capacity in GB (0 = unlimited)
    #[serde(default)]
    pub max_capacity_gb: u64,

    /// Enable LZ4 compression for stored chunks
    #[serde(default = "default_true")]
    pub compression: bool,

    /// RocksDB block cache size in MB
    #[serde(default = "default_cache_size")]
    pub cache_size_mb: usize,

    /// Number of background compaction threads
    #[serde(default = "default_compaction_threads")]
    pub compaction_threads: usize,
}

impl Default for StorageSettings {
    fn default() -> Self {
        Self {
            data_dir: default_data_dir(),
            max_capacity_gb: 0,
            compression: true,
            cache_size_mb: 512,
            compaction_threads: 4,
        }
    }
}

impl StorageSettings {
    /// Convert to cyxcloud_storage::StorageConfig
    pub fn to_storage_config(&self) -> cyxcloud_storage::StorageConfig {
        cyxcloud_storage::StorageConfig {
            path: self.data_dir.clone(),
            max_capacity: self.max_capacity_gb * 1024 * 1024 * 1024,
            compression: self.compression,
            cache_size: self.cache_size_mb * 1024 * 1024,
            compaction_threads: self.compaction_threads,
        }
    }
}

fn default_data_dir() -> PathBuf {
    PathBuf::from("./data")
}

fn default_cache_size() -> usize {
    512
}

fn default_compaction_threads() -> usize {
    4
}

fn default_true() -> bool {
    true
}

/// Network configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkSettings {
    /// Address to bind for gRPC server
    #[serde(default = "default_bind_addr")]
    pub bind_address: String,

    /// gRPC server port
    #[serde(default = "default_grpc_port")]
    pub grpc_port: u16,

    /// libp2p P2P port
    #[serde(default = "default_p2p_port")]
    pub p2p_port: u16,

    /// Public address for other nodes to connect (optional, auto-detected)
    #[serde(default)]
    pub public_address: Option<String>,

    /// Maximum message size in MB
    #[serde(default = "default_max_message_size")]
    pub max_message_size_mb: usize,

    /// Enable TLS for gRPC
    #[serde(default)]
    pub enable_tls: bool,

    /// TLS certificate path
    #[serde(default)]
    pub tls_cert: Option<PathBuf>,

    /// TLS key path
    #[serde(default)]
    pub tls_key: Option<PathBuf>,

    /// Bootstrap peers for P2P discovery
    #[serde(default)]
    pub bootstrap_peers: Vec<String>,
}

impl Default for NetworkSettings {
    fn default() -> Self {
        Self {
            bind_address: default_bind_addr(),
            grpc_port: default_grpc_port(),
            p2p_port: default_p2p_port(),
            public_address: None,
            max_message_size_mb: 64,
            enable_tls: false,
            tls_cert: None,
            tls_key: None,
            bootstrap_peers: Vec::new(),
        }
    }
}

impl NetworkSettings {
    /// Get the gRPC listen address
    pub fn grpc_addr(&self) -> SocketAddr {
        format!("{}:{}", self.bind_address, self.grpc_port)
            .parse()
            .unwrap_or_else(|_| "0.0.0.0:50051".parse().unwrap())
    }

    /// Get the P2P listen address
    pub fn p2p_addr(&self) -> SocketAddr {
        format!("{}:{}", self.bind_address, self.p2p_port)
            .parse()
            .unwrap_or_else(|_| "0.0.0.0:4001".parse().unwrap())
    }
}

fn default_bind_addr() -> String {
    "0.0.0.0".to_string()
}

fn default_grpc_port() -> u16 {
    50051
}

fn default_p2p_port() -> u16 {
    4001
}

fn default_max_message_size() -> usize {
    64
}

/// Metrics and monitoring configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsSettings {
    /// Enable Prometheus metrics endpoint
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Metrics HTTP server port
    #[serde(default = "default_metrics_port")]
    pub port: u16,

    /// Health check endpoint path
    #[serde(default = "default_health_path")]
    pub health_path: String,

    /// Metrics endpoint path
    #[serde(default = "default_metrics_path")]
    pub metrics_path: String,
}

impl Default for MetricsSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            port: 9090,
            health_path: "/health".to_string(),
            metrics_path: "/metrics".to_string(),
        }
    }
}

fn default_metrics_port() -> u16 {
    9090
}

fn default_health_path() -> String {
    "/health".to_string()
}

fn default_metrics_path() -> String {
    "/metrics".to_string()
}

/// Central server connection configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CentralServerSettings {
    /// Central server gRPC address
    #[serde(default = "default_central_addr")]
    pub address: String,

    /// Enable registration with central server
    #[serde(default = "default_true")]
    pub register: bool,

    /// Heartbeat interval in seconds
    #[serde(default = "default_heartbeat_interval")]
    pub heartbeat_interval_secs: u64,

    /// Connection timeout in seconds
    #[serde(default = "default_connect_timeout")]
    pub connect_timeout_secs: u64,

    /// Authentication token (if required)
    #[serde(default)]
    pub auth_token: Option<String>,
}

impl Default for CentralServerSettings {
    fn default() -> Self {
        Self {
            address: default_central_addr(),
            register: true,
            heartbeat_interval_secs: 30,
            connect_timeout_secs: 10,
            auth_token: None,
        }
    }
}

fn default_central_addr() -> String {
    "http://localhost:50052".to_string()
}

fn default_heartbeat_interval() -> u64 {
    30
}

fn default_connect_timeout() -> u64 {
    10
}

/// CyxWiz API connection configuration (for auth, machines, wallets)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CyxWizApiSettings {
    /// CyxWiz API base URL
    #[serde(default = "default_cyxwiz_api_url")]
    pub base_url: String,

    /// Owner ID (user who owns this machine)
    #[serde(default)]
    pub owner_id: Option<String>,

    /// Machine API key (received after registration)
    #[serde(default)]
    pub api_key: Option<String>,

    /// Machine ID (received after registration)
    #[serde(default)]
    pub machine_id: Option<String>,

    /// Enable machine registration with CyxWiz API
    #[serde(default = "default_true")]
    pub register: bool,

    /// Request timeout in seconds
    #[serde(default = "default_connect_timeout")]
    pub timeout_secs: u64,

    /// Email for non-interactive login (from CYXWIZ_EMAIL env var)
    #[serde(default)]
    pub email: Option<String>,

    /// Password for non-interactive login (from CYXWIZ_PASSWORD env var)
    #[serde(default, skip_serializing)]
    pub password: Option<String>,
}

impl Default for CyxWizApiSettings {
    fn default() -> Self {
        Self {
            base_url: default_cyxwiz_api_url(),
            owner_id: None,
            api_key: None,
            machine_id: None,
            register: true,
            timeout_secs: 10,
            email: None,
            password: None,
        }
    }
}

impl CyxWizApiSettings {
    /// Apply environment variable overrides
    pub fn with_env_overrides(mut self) -> Self {
        if let Ok(url) = std::env::var("CYXWIZ_API_URL") {
            self.base_url = url;
        }
        if let Ok(email) = std::env::var("CYXWIZ_EMAIL") {
            self.email = Some(email);
        }
        if let Ok(password) = std::env::var("CYXWIZ_PASSWORD") {
            self.password = Some(password);
        }
        self
    }

    /// Check if credentials are available for non-interactive login
    pub fn has_credentials(&self) -> bool {
        self.email.is_some() && self.password.is_some()
    }
}

fn default_cyxwiz_api_url() -> String {
    "http://localhost:3002".to_string()
}

/// Get the standard CyxCloud config directory (~/.cyxcloud/)
pub fn cyxcloud_config_dir() -> std::path::PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".cyxcloud")
}

/// Get the standard credentials file path for node (~/.cyxcloud/node_credentials.json)
pub fn node_credentials_path() -> std::path::PathBuf {
    cyxcloud_config_dir().join("node_credentials.json")
}

/// Get the shared config file path (~/.cyxcloud/config.toml)
pub fn shared_config_path() -> std::path::PathBuf {
    cyxcloud_config_dir().join("config.toml")
}

/// Shared CyxCloud configuration (from ~/.cyxcloud/config.toml)
/// Used to get gateway and auth server URLs
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SharedConfig {
    #[serde(default)]
    pub gateway: SharedGatewayConfig,
    #[serde(default)]
    pub auth: SharedAuthConfig,
    #[serde(default)]
    pub blockchain: SharedBlockchainConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharedGatewayConfig {
    #[serde(default = "default_gateway_http_url")]
    pub http_url: String,
    #[serde(default = "default_central_addr")]
    pub grpc_url: String,
}

impl Default for SharedGatewayConfig {
    fn default() -> Self {
        Self {
            http_url: default_gateway_http_url(),
            grpc_url: default_central_addr(),
        }
    }
}

fn default_gateway_http_url() -> String {
    std::env::var("CYXCLOUD_GATEWAY_HTTP_URL")
        .unwrap_or_else(|_| "http://localhost:8080".to_string())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharedAuthConfig {
    #[serde(default = "default_cyxwiz_api_url")]
    pub api_url: String,
}

impl Default for SharedAuthConfig {
    fn default() -> Self {
        Self {
            api_url: default_cyxwiz_api_url(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharedBlockchainConfig {
    #[serde(default = "default_solana_rpc")]
    pub solana_rpc_url: String,
}

impl Default for SharedBlockchainConfig {
    fn default() -> Self {
        Self {
            solana_rpc_url: default_solana_rpc(),
        }
    }
}

/// Load shared configuration from ~/.cyxcloud/config.toml
pub fn load_shared_config() -> SharedConfig {
    let path = shared_config_path();
    if path.exists() {
        match std::fs::read_to_string(&path) {
            Ok(content) => match toml::from_str(&content) {
                Ok(config) => return config,
                Err(e) => {
                    tracing::warn!(error = %e, "Failed to parse shared config file");
                }
            },
            Err(e) => {
                tracing::warn!(error = %e, "Failed to read shared config file");
            }
        }
    }
    SharedConfig::default()
}

/// Blockchain (Solana) configuration for staking and rewards
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockchainSettings {
    /// Enable blockchain integration
    #[serde(default)]
    pub enabled: bool,

    /// Solana RPC URL
    #[serde(default = "default_solana_rpc")]
    pub rpc_url: String,

    /// Path to node owner keypair (Solana JSON format)
    #[serde(default)]
    pub keypair_path: Option<String>,

    /// Enable automatic heartbeat to blockchain
    #[serde(default)]
    pub heartbeat_enabled: bool,

    /// Heartbeat interval in seconds
    #[serde(default = "default_blockchain_heartbeat_interval")]
    pub heartbeat_interval_secs: u64,

    /// Enable automatic reward claiming
    #[serde(default)]
    pub auto_claim_rewards: bool,
}

impl Default for BlockchainSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            rpc_url: default_solana_rpc(),
            keypair_path: None,
            heartbeat_enabled: false,
            heartbeat_interval_secs: 60,
            auto_claim_rewards: false,
        }
    }
}

fn default_solana_rpc() -> String {
    "https://api.devnet.solana.com".to_string()
}

fn default_blockchain_heartbeat_interval() -> u64 {
    60
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_default_config() {
        let config = NodeConfig::default();
        assert!(!config.node.id.is_empty());
        assert_eq!(config.network.grpc_port, 50051);
        assert_eq!(config.storage.cache_size_mb, 512);
    }

    #[test]
    fn test_config_from_toml() {
        let toml = r#"
            [node]
            name = "test-node"
            region = "us-west"

            [storage]
            max_capacity_gb = 100
            compression = true

            [network]
            grpc_port = 9000
            enable_tls = false

            [metrics]
            enabled = true
            port = 9090
        "#;

        let config: NodeConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.node.name, "test-node");
        assert_eq!(config.node.region, Some("us-west".to_string()));
        assert_eq!(config.storage.max_capacity_gb, 100);
        assert_eq!(config.network.grpc_port, 9000);
    }

    #[test]
    fn test_config_validation() {
        let temp_dir = TempDir::new().unwrap();
        let mut config = NodeConfig::default();
        config.storage.data_dir = temp_dir.path().to_path_buf();

        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_overrides() {
        let config = NodeConfig::default()
            .with_overrides(Some(PathBuf::from("/custom/path")), Some(8000));

        assert_eq!(config.storage.data_dir, PathBuf::from("/custom/path"));
        assert_eq!(config.network.grpc_port, 8000);
        assert_eq!(config.network.p2p_port, 8001);
    }

    #[test]
    fn test_cyxwiz_api_credentials() {
        let settings = CyxWizApiSettings {
            email: Some("test@example.com".to_string()),
            password: Some("password123".to_string()),
            ..Default::default()
        };
        assert!(settings.has_credentials());

        let no_creds = CyxWizApiSettings::default();
        assert!(!no_creds.has_credentials());
    }
}
