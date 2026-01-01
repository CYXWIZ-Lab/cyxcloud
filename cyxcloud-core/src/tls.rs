//! TLS utilities for CyxCloud mTLS (mutual TLS) authentication.
//!
//! This module provides shared TLS configuration for:
//! - Gateway: HTTPS server + gRPC server with optional client cert verification
//! - Storage Nodes: gRPC client (to gateway) + gRPC server (for inter-node)
//! - CLI: HTTPS client with optional client certificate

use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::sync::Arc;

use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::{ClientConfig, RootCertStore, ServerConfig};
use rustls_pemfile::{certs, private_key};

use crate::error::{CyxCloudError, Result};

/// Server TLS configuration for gateway and storage nodes.
#[derive(Debug, Clone)]
pub struct TlsServerConfig {
    /// Path to the server certificate file (PEM format)
    pub cert_path: std::path::PathBuf,
    /// Path to the server private key file (PEM format)
    pub key_path: std::path::PathBuf,
    /// Optional path to CA certificate for client verification (mTLS)
    pub ca_cert_path: Option<std::path::PathBuf>,
    /// Whether to require client certificates (mTLS)
    pub require_client_cert: bool,
}

/// Client TLS configuration for nodes and CLI.
#[derive(Debug, Clone)]
pub struct TlsClientConfig {
    /// Path to CA certificate for server verification
    pub ca_cert_path: std::path::PathBuf,
    /// Optional path to client certificate (mTLS)
    pub client_cert_path: Option<std::path::PathBuf>,
    /// Optional path to client private key (mTLS)
    pub client_key_path: Option<std::path::PathBuf>,
}

/// Load certificates from a PEM file.
fn load_certs(path: &Path) -> Result<Vec<CertificateDer<'static>>> {
    let file = File::open(path).map_err(|e| {
        CyxCloudError::Io(std::io::Error::new(
            e.kind(),
            format!("Failed to open certificate file {:?}: {}", path, e),
        ))
    })?;
    let mut reader = BufReader::new(file);
    let certs_result: std::result::Result<Vec<_>, _> = certs(&mut reader).collect();
    certs_result.map_err(|e| {
        CyxCloudError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Failed to parse certificates from {:?}: {}", path, e),
        ))
    })
}

/// Load a private key from a PEM file.
fn load_private_key(path: &Path) -> Result<PrivateKeyDer<'static>> {
    let file = File::open(path).map_err(|e| {
        CyxCloudError::Io(std::io::Error::new(
            e.kind(),
            format!("Failed to open private key file {:?}: {}", path, e),
        ))
    })?;
    let mut reader = BufReader::new(file);
    private_key(&mut reader)
        .map_err(|e| {
            CyxCloudError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Failed to parse private key from {:?}: {}", path, e),
            ))
        })?
        .ok_or_else(|| {
            CyxCloudError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("No private key found in {:?}", path),
            ))
        })
}

/// Build a root certificate store from a CA certificate file.
fn build_root_store(ca_cert_path: &Path) -> Result<RootCertStore> {
    let ca_certs = load_certs(ca_cert_path)?;
    let mut root_store = RootCertStore::empty();
    for cert in ca_certs {
        root_store.add(cert).map_err(|e| {
            CyxCloudError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Failed to add CA certificate to root store: {}", e),
            ))
        })?;
    }
    Ok(root_store)
}

/// Load server TLS configuration for rustls.
///
/// Returns a configured `ServerConfig` that can be used with:
/// - axum-server for HTTPS
/// - tonic for gRPC TLS
///
/// # Arguments
/// * `config` - Server TLS configuration with certificate paths
///
/// # Example
/// ```ignore
/// let config = TlsServerConfig {
///     cert_path: PathBuf::from("certs/gateway.crt"),
///     key_path: PathBuf::from("certs/gateway.key"),
///     ca_cert_path: Some(PathBuf::from("certs/ca.crt")),
///     require_client_cert: true,
/// };
/// let tls_config = load_server_config(&config)?;
/// ```
pub fn load_server_config(config: &TlsServerConfig) -> Result<Arc<ServerConfig>> {
    let certs = load_certs(&config.cert_path)?;
    let key = load_private_key(&config.key_path)?;

    let builder = if config.require_client_cert {
        let ca_path = config.ca_cert_path.as_ref().ok_or_else(|| {
            CyxCloudError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "CA certificate path required when client cert verification is enabled",
            ))
        })?;
        let root_store = build_root_store(ca_path)?;
        let client_verifier = rustls::server::WebPkiClientVerifier::builder(Arc::new(root_store))
            .build()
            .map_err(|e| {
                CyxCloudError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("Failed to build client verifier: {}", e),
                ))
            })?;
        ServerConfig::builder().with_client_cert_verifier(client_verifier)
    } else if let Some(ca_path) = &config.ca_cert_path {
        // Optional client cert verification (accept but don't require)
        let root_store = build_root_store(ca_path)?;
        let client_verifier = rustls::server::WebPkiClientVerifier::builder(Arc::new(root_store))
            .allow_unauthenticated()
            .build()
            .map_err(|e| {
                CyxCloudError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("Failed to build client verifier: {}", e),
                ))
            })?;
        ServerConfig::builder().with_client_cert_verifier(client_verifier)
    } else {
        // No client cert verification
        ServerConfig::builder().with_no_client_auth()
    };

    let server_config = builder.with_single_cert(certs, key).map_err(|e| {
        CyxCloudError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Failed to configure server TLS: {}", e),
        ))
    })?;

    Ok(Arc::new(server_config))
}

/// Load client TLS configuration for rustls.
///
/// Returns a configured `ClientConfig` that can be used with:
/// - reqwest for HTTPS client
/// - tonic for gRPC TLS client
///
/// # Arguments
/// * `config` - Client TLS configuration with certificate paths
///
/// # Example
/// ```ignore
/// let config = TlsClientConfig {
///     ca_cert_path: PathBuf::from("certs/ca.crt"),
///     client_cert_path: Some(PathBuf::from("certs/node1.crt")),
///     client_key_path: Some(PathBuf::from("certs/node1.key")),
/// };
/// let tls_config = load_client_config(&config)?;
/// ```
pub fn load_client_config(config: &TlsClientConfig) -> Result<Arc<ClientConfig>> {
    let root_store = build_root_store(&config.ca_cert_path)?;

    let client_config = if let (Some(cert_path), Some(key_path)) =
        (&config.client_cert_path, &config.client_key_path)
    {
        // mTLS: client certificate authentication
        let client_certs = load_certs(cert_path)?;
        let client_key = load_private_key(key_path)?;

        ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_client_auth_cert(client_certs, client_key)
            .map_err(|e| {
                CyxCloudError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("Failed to configure client TLS with cert: {}", e),
                ))
            })?
    } else {
        // Server verification only (no client cert)
        ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth()
    };

    Ok(Arc::new(client_config))
}

/// Load client TLS configuration with system root certificates.
///
/// Uses the system's trusted root certificates instead of a custom CA.
/// Useful for connecting to public TLS servers.
pub fn load_client_config_with_system_roots() -> Result<Arc<ClientConfig>> {
    let mut root_store = RootCertStore::empty();
    root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

    let client_config = ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();

    Ok(Arc::new(client_config))
}

/// Create a tonic client TLS config from TlsClientConfig.
///
/// This returns a `tonic::transport::ClientTlsConfig` suitable for gRPC clients.
pub fn create_tonic_client_tls(
    config: &TlsClientConfig,
) -> Result<tonic::transport::ClientTlsConfig> {
    use tonic::transport::Certificate;

    let ca_cert = std::fs::read(&config.ca_cert_path).map_err(|e| {
        CyxCloudError::Io(std::io::Error::new(
            e.kind(),
            format!("Failed to read CA certificate: {}", e),
        ))
    })?;

    let mut tls_config =
        tonic::transport::ClientTlsConfig::new().ca_certificate(Certificate::from_pem(ca_cert));

    if let (Some(cert_path), Some(key_path)) = (&config.client_cert_path, &config.client_key_path) {
        let client_cert = std::fs::read(cert_path).map_err(|e| {
            CyxCloudError::Io(std::io::Error::new(
                e.kind(),
                format!("Failed to read client certificate: {}", e),
            ))
        })?;
        let client_key = std::fs::read(key_path).map_err(|e| {
            CyxCloudError::Io(std::io::Error::new(
                e.kind(),
                format!("Failed to read client key: {}", e),
            ))
        })?;

        tls_config = tls_config.identity(tonic::transport::Identity::from_pem(
            client_cert,
            client_key,
        ));
    }

    Ok(tls_config)
}

/// Create a tonic server TLS config from TlsServerConfig.
///
/// This returns a `tonic::transport::ServerTlsConfig` suitable for gRPC servers.
pub fn create_tonic_server_tls(
    config: &TlsServerConfig,
) -> Result<tonic::transport::ServerTlsConfig> {
    use tonic::transport::{Certificate, Identity};

    let cert = std::fs::read(&config.cert_path).map_err(|e| {
        CyxCloudError::Io(std::io::Error::new(
            e.kind(),
            format!("Failed to read server certificate: {}", e),
        ))
    })?;
    let key = std::fs::read(&config.key_path).map_err(|e| {
        CyxCloudError::Io(std::io::Error::new(
            e.kind(),
            format!("Failed to read server key: {}", e),
        ))
    })?;

    let mut tls_config =
        tonic::transport::ServerTlsConfig::new().identity(Identity::from_pem(cert, key));

    if let Some(ca_path) = &config.ca_cert_path {
        let ca_cert = std::fs::read(ca_path).map_err(|e| {
            CyxCloudError::Io(std::io::Error::new(
                e.kind(),
                format!("Failed to read CA certificate: {}", e),
            ))
        })?;
        tls_config = tls_config.client_ca_root(Certificate::from_pem(ca_cert));
    }

    Ok(tls_config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_tls_server_config_creation() {
        let config = TlsServerConfig {
            cert_path: PathBuf::from("test.crt"),
            key_path: PathBuf::from("test.key"),
            ca_cert_path: None,
            require_client_cert: false,
        };
        assert!(!config.require_client_cert);
    }

    #[test]
    fn test_tls_client_config_creation() {
        let config = TlsClientConfig {
            ca_cert_path: PathBuf::from("ca.crt"),
            client_cert_path: Some(PathBuf::from("client.crt")),
            client_key_path: Some(PathBuf::from("client.key")),
        };
        assert!(config.client_cert_path.is_some());
    }
}
