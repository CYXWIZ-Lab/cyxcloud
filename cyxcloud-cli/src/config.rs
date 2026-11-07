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
    /// The one server address the user gave (`cyxcloud login --server`)
    #[serde(default)]
    pub server: ServerConfig,

    /// Gateway settings
    #[serde(default)]
    pub gateway: GatewayConfig,

    /// Authentication server settings
    #[serde(default)]
    pub auth: AuthConfig,

    /// CLI-specific settings
    #[serde(default)]
    pub cli: CliSettings,
}

/// The server the user set once; the service URLs below are derived from it
/// (crate::server::urls_for_server) and can still be overridden one by one.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ServerConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// The server's private CA (a test server's ca.crt, docs/usage/tls.md),
    /// for a server whose certificate no public authority signed. Cleared
    /// when the server changes: another server's CA is no use to it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca_cert: Option<PathBuf>,
}

impl CyxCloudConfig {
    /// Point every service at `server` (a host, or a front door: https://HOST, http://HOST:PORT).
    pub fn set_server(&mut self, server: &str) -> std::result::Result<(), String> {
        let urls = crate::server::urls_for_server(server)?;
        cyxcloud_core::plain_http::check_url(&urls.api)?;
        let host = server.trim().trim_end_matches('/').to_string();
        // A CA belongs to its server: a different host or port drops it
        // ("https://h" and "https://h:443" are the same server)
        let same_server = self.server.host.as_deref().is_some_and(|old| {
            let key = |s: &str| (crate::server::url_host(s), crate::server::url_port(s));
            key(old) == key(&host)
        });
        if !same_server {
            self.server.ca_cert = None;
        }
        self.server.host = Some(host);
        self.gateway.http_url = urls.gateway_http;
        self.gateway.grpc_url = urls.gateway_grpc;
        self.auth.api_url = urls.api;
        Ok(())
    }

    /// Trust `path` (a PEM CA certificate) for the saved server. It must
    /// read as at least one certificate; the absolute path is kept, so the
    /// setting works from any folder.
    pub fn set_ca_cert(&mut self, path: &std::path::Path) -> std::result::Result<(), String> {
        use rustls::pki_types::{pem::PemObject, CertificateDer};
        let certs = CertificateDer::pem_file_iter(path)
            .and_then(|c| c.collect::<std::result::Result<Vec<_>, _>>())
            .map_err(|e| format!("{}: not a PEM certificate ({})", path.display(), e))?;
        if certs.is_empty() {
            return Err(format!("{}: holds no certificate", path.display()));
        }
        // absolute, not canonicalize: no \\?\ prefix on Windows
        let absolute =
            std::path::absolute(path).map_err(|e| format!("{}: {}", path.display(), e))?;
        self.server.ca_cert = Some(absolute);
        Ok(())
    }
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
    std::env::var("CYXCLOUD_AUTH_API_URL").unwrap_or_else(|_| "http://localhost:3002".to_string())
}

/// CLI-specific settings
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CliSettings {
    /// Default bucket name for operations
    #[serde(default)]
    pub default_bucket: Option<String>,
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
        Ok(path) if path.exists() => match fs::read_to_string(&path) {
            Ok(content) => match toml::from_str(&content) {
                Ok(config) => config,
                Err(e) => {
                    eprintln!("Warning: Failed to parse config file: {}", e);
                    CyxCloudConfig::default()
                }
            },
            Err(e) => {
                eprintln!("Warning: Failed to read config file: {}", e);
                CyxCloudConfig::default()
            }
        },
        _ => CyxCloudConfig::default(),
    }
}

/// Save configuration to file
pub fn save_config(config: &CyxCloudConfig) -> Result<()> {
    let path = config_file_path()?;
    let content = toml::to_string_pretty(config).context("Failed to serialize config")?;
    fs::write(&path, content).context("Failed to write config file")?;
    Ok(())
}

/// Load user credentials from file
pub fn load_credentials() -> Result<Option<Credentials>> {
    let path = credentials_file_path()?;

    if path.exists() {
        let content = fs::read_to_string(&path).context("Failed to read credentials file")?;
        let creds: Credentials =
            serde_json::from_str(&content).context("Failed to parse credentials file")?;
        Ok(Some(creds))
    } else {
        Ok(None)
    }
}

/// Save user credentials to file
pub fn save_credentials(credentials: &Credentials) -> Result<()> {
    let path = credentials_file_path()?;
    let content =
        serde_json::to_string_pretty(credentials).context("Failed to serialize credentials")?;

    // Readable by the owner only, on every OS (Windows ACL too)
    cyxcloud_core::owner_only::write_owner_only(&path, content)
        .context("Failed to save credentials owner-only")?;

    Ok(())
}

/// Delete stored credentials (logout)
pub fn delete_credentials() -> Result<()> {
    let path = credentials_file_path()?;

    if path.exists() {
        fs::remove_file(&path).context("Failed to delete credentials file")?;
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
    fn a_public_server_must_be_https() {
        let mut cfg = CyxCloudConfig::default();
        assert!(cfg.set_server("http://192.168.1.180:8000").is_ok());
        assert!(cfg.set_server("https://cloud.cyxwiz.ai").is_ok());
        let err = cfg.set_server("http://cloud.cyxwiz.ai:8000").unwrap_err();
        assert!(err.contains("https://"), "{err}");
        assert_eq!(cfg.server.host.as_deref(), Some("https://cloud.cyxwiz.ai"));
    }

    #[test]
    fn the_ca_belongs_to_its_server() {
        let dir = tempfile::tempdir().unwrap();
        let ca = dir.path().join("ca.crt");
        let cert = rcgen::generate_simple_self_signed(vec!["cyxwiz-hp".into()]).unwrap();
        std::fs::write(&ca, cert.cert.pem()).unwrap();

        let mut cfg = CyxCloudConfig::default();
        cfg.set_server("https://192.168.1.180").unwrap();
        cfg.set_ca_cert(&ca).unwrap();
        assert!(cfg.server.ca_cert.as_ref().unwrap().is_absolute());

        cfg.set_server("https://192.168.1.180/").unwrap();
        assert!(cfg.server.ca_cert.is_some(), "the same server keeps its CA");
        cfg.set_server("https://192.168.1.180:443").unwrap();
        assert!(
            cfg.server.ca_cert.is_some(),
            "its default port is the same server"
        );
        cfg.set_server("https://192.168.1.180:8443").unwrap();
        assert!(
            cfg.server.ca_cert.is_none(),
            "another port is another server"
        );
        cfg.set_server("https://192.168.1.180").unwrap();
        cfg.set_ca_cert(&ca).unwrap();
        cfg.set_server("https://cloud.cyxwiz.ai").unwrap();
        assert!(
            cfg.server.ca_cert.is_none(),
            "another server's CA is dropped"
        );

        let not_pem = dir.path().join("notes.txt");
        std::fs::write(&not_pem, "not a certificate").unwrap();
        assert!(cfg.set_ca_cert(&not_pem).is_err());
        assert!(cfg.set_ca_cert(&dir.path().join("missing.crt")).is_err());
    }

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
