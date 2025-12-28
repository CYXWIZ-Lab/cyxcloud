//! Configuration and credential management
//!
//! Handles storing and loading CLI configuration and user credentials.
//! Config directory: ~/.cyxcloud/ (cross-platform)
//!
//! Config file format (~/.cyxcloud/config.toml):
//! ```toml
//! [gateway]
//! http_url = "http://localhost:8080"
//! grpc_url = "http://localhost:50052"
//!
//! [auth]
//! api_url = "http://localhost:3002"
//!
//! [cli]
//! default_bucket = "my-bucket"
//! ```

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// Shared CyxCloud configuration (used by both CLI and Node)
/// This is the structure of ~/.cyxcloud/config.toml
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CyxCloudConfig {
    /// Gateway settings
    #[serde(default)]
    pub gateway: GatewayConfig,

    /// Authentication server settings
    #[serde(default)]
    pub auth: AuthConfig,

    /// CLI-specific settings
    #[serde(default)]
    pub cli: CliSettings,

    /// Blockchain settings (optional)
    #[serde(default)]
    pub blockchain: BlockchainConfig,
}

/// Gateway connection settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayConfig {
    /// Gateway HTTP API URL (for S3-compatible operations)
    #[serde(default = "default_gateway_http_url")]
    pub http_url: String,

    /// Gateway gRPC URL (for node registration, etc.)
    #[serde(default = "default_gateway_grpc_url")]
    pub grpc_url: String,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            http_url: default_gateway_http_url(),
            grpc_url: default_gateway_grpc_url(),
        }
    }
}

fn default_gateway_http_url() -> String {
    std::env::var("CYXCLOUD_GATEWAY_HTTP_URL")
        .unwrap_or_else(|_| "http://localhost:8080".to_string())
}

fn default_gateway_grpc_url() -> String {
    std::env::var("CYXCLOUD_GATEWAY_GRPC_URL")
        .unwrap_or_else(|_| "http://localhost:50052".to_string())
}

/// Authentication server settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    /// CyxWiz API URL for authentication
    #[serde(default = "default_auth_api_url")]
    pub api_url: String,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            api_url: default_auth_api_url(),
        }
    }
}

fn default_auth_api_url() -> String {
    std::env::var("CYXCLOUD_AUTH_API_URL")
        .unwrap_or_else(|_| "http://localhost:3002".to_string())
}

/// CLI-specific settings
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CliSettings {
    /// Default bucket name for operations
    #[serde(default)]
    pub default_bucket: Option<String>,
}

/// Blockchain settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockchainConfig {
    /// Solana RPC URL
    #[serde(default = "default_solana_rpc_url")]
    pub solana_rpc_url: String,
}

impl Default for BlockchainConfig {
    fn default() -> Self {
        Self {
            solana_rpc_url: default_solana_rpc_url(),
        }
    }
}

fn default_solana_rpc_url() -> String {
    std::env::var("CYXCLOUD_SOLANA_RPC_URL")
        .unwrap_or_else(|_| "https://api.devnet.solana.com".to_string())
}

/// User credentials stored after login
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credentials {
    /// JWT access token
    pub access_token: String,
    /// Refresh token for obtaining new access tokens
    pub refresh_token: Option<String>,
    /// Token expiration time (Unix timestamp)
    pub expires_at: i64,
    /// User ID
    pub user_id: String,
    /// User email
    pub email: String,
    /// Username (optional)
    pub username: Option<String>,
}

impl Credentials {
    /// Check if the token is expired or about to expire (within 5 minutes)
    pub fn is_expired(&self) -> bool {
        let now = chrono::Utc::now().timestamp();
        self.expires_at - now < 300 // 5 minutes buffer
    }

    /// Check if credentials are still valid
    pub fn is_valid(&self) -> bool {
        !self.is_expired() && !self.access_token.is_empty()
    }
}

/// Get the config directory path (~/.cyxcloud/)
pub fn config_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("Could not determine home directory")?;
    let config_dir = home.join(".cyxcloud");

    // Create directory if it doesn't exist
    if !config_dir.exists() {
        fs::create_dir_all(&config_dir)
            .context("Failed to create config directory ~/.cyxcloud/")?;
    }

    Ok(config_dir)
}

/// Get the config file path
pub fn config_file_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("config.toml"))
}

/// Get the credentials file path
pub fn credentials_file_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("credentials.json"))
}

/// Load shared CyxCloud configuration from file
/// Falls back to defaults if file doesn't exist
pub fn load_config() -> CyxCloudConfig {
    match config_file_path() {
        Ok(path) if path.exists() => {
            match fs::read_to_string(&path) {
                Ok(content) => {
                    match toml::from_str(&content) {
                        Ok(config) => config,
                        Err(e) => {
                            eprintln!("Warning: Failed to parse config file: {}", e);
                            CyxCloudConfig::default()
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Warning: Failed to read config file: {}", e);
                    CyxCloudConfig::default()
                }
            }
        }
        _ => CyxCloudConfig::default(),
    }
}

/// Save configuration to file
pub fn save_config(config: &CyxCloudConfig) -> Result<()> {
    let path = config_file_path()?;
    let content = toml::to_string_pretty(config)
        .context("Failed to serialize config")?;
    fs::write(&path, content)
        .context("Failed to write config file")?;
    Ok(())
}

/// Create a default config file if it doesn't exist
pub fn ensure_config_exists() -> Result<()> {
    let path = config_file_path()?;
    if !path.exists() {
        save_config(&CyxCloudConfig::default())?;
    }
    Ok(())
}

/// Load user credentials from file
pub fn load_credentials() -> Result<Option<Credentials>> {
    let path = credentials_file_path()?;

    if path.exists() {
        let content = fs::read_to_string(&path)
            .context("Failed to read credentials file")?;
        let creds: Credentials = serde_json::from_str(&content)
            .context("Failed to parse credentials file")?;
        Ok(Some(creds))
    } else {
        Ok(None)
    }
}

/// Save user credentials to file
pub fn save_credentials(credentials: &Credentials) -> Result<()> {
    let path = credentials_file_path()?;
    let content = serde_json::to_string_pretty(credentials)
        .context("Failed to serialize credentials")?;

    // Set restrictive permissions on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::write(&path, &content)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    }

    #[cfg(not(unix))]
    {
        fs::write(&path, content)?;
    }

    Ok(())
}

/// Delete stored credentials (logout)
pub fn delete_credentials() -> Result<()> {
    let path = credentials_file_path()?;

    if path.exists() {
        fs::remove_file(&path)
            .context("Failed to delete credentials file")?;
    }

    Ok(())
}

/// Get valid credentials, or None if not logged in or expired
pub fn get_valid_credentials() -> Option<Credentials> {
    match load_credentials() {
        Ok(Some(creds)) if creds.is_valid() => Some(creds),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_credentials_expiry() {
        let now = chrono::Utc::now().timestamp();

        // Expired token
        let expired = Credentials {
            access_token: "test".to_string(),
            refresh_token: None,
            expires_at: now - 100,
            user_id: "user1".to_string(),
            email: "test@example.com".to_string(),
            username: None,
        };
        assert!(expired.is_expired());
        assert!(!expired.is_valid());

        // Valid token
        let valid = Credentials {
            access_token: "test".to_string(),
            refresh_token: None,
            expires_at: now + 3600,
            user_id: "user1".to_string(),
            email: "test@example.com".to_string(),
            username: None,
        };
        assert!(!valid.is_expired());
        assert!(valid.is_valid());
    }

    #[test]
    fn test_default_config() {
        let config = CyxCloudConfig::default();
        assert!(config.gateway.http_url.contains("8080"));
        assert!(config.gateway.grpc_url.contains("50052"));
        assert!(config.auth.api_url.contains("3002"));
    }

    #[test]
    fn test_config_serialization() {
        let config = CyxCloudConfig::default();
        let toml_str = toml::to_string_pretty(&config).unwrap();

        // Should contain all sections
        assert!(toml_str.contains("[gateway]"));
        assert!(toml_str.contains("[auth]"));
        assert!(toml_str.contains("http_url"));
        assert!(toml_str.contains("api_url"));
    }
}
