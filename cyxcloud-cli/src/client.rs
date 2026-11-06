//! Gateway REST client for the dataset calls that are not on gRPC yet:
//! `cyxcloud dataset verify` and `cyxcloud dataset share` (share takes an
//! email address; the gRPC ShareDataset needs an internal user id).
//! Everything else goes over gRPC (crate::cloud).

use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;
use thiserror::Error;

/// Client errors
#[derive(Error, Debug)]
pub enum ClientError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("API error: {status} - {message}")]
    Api { status: u16, message: String },

    #[error("Not found: {0}")]
    NotFound(String),
}

pub type Result<T> = std::result::Result<T, ClientError>;

/// Dataset verification result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationResult {
    pub manifest_valid: bool,
    pub all_files_valid: bool,
    pub files_verified: i32,
    pub files_failed: i32,
    pub trust_level: i32,
    pub public_match: Option<PublicDatasetMatch>,
}

/// Public dataset match
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicDatasetMatch {
    pub name: String,
    pub version: String,
    pub verified_by: Vec<String>,
    pub license: Option<String>,
}

/// Dataset share result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShareResult {
    pub success: bool,
    pub share_id: String,
}

/// How the CLI's clients verify the server (docs/usage/tls.md). Always
/// verified: with `ca_cert`, against that CA only; without, against the
/// system's trusted roots.
#[derive(Debug, Clone, Default)]
pub struct TlsConfig {
    /// The server's private CA (PEM)
    pub ca_cert: Option<PathBuf>,
    /// Path to client certificate (PEM format) for mTLS
    pub client_cert: Option<PathBuf>,
    /// Path to client private key (PEM format) for mTLS
    pub client_key: Option<PathBuf>,
}

impl TlsConfig {
    /// `builder` with this TLS setup. Certificate files that cannot be read
    /// or parsed are an error, not silently skipped.
    pub fn apply(
        &self,
        mut builder: reqwest::ClientBuilder,
    ) -> anyhow::Result<reqwest::ClientBuilder> {
        use anyhow::Context;
        if let Some(ref ca_path) = self.ca_cert {
            let pem = std::fs::read(ca_path)
                .with_context(|| format!("cannot read the CA certificate {}", ca_path.display()))?;
            let cert = reqwest::Certificate::from_pem(&pem)
                .with_context(|| format!("{} is not a PEM certificate", ca_path.display()))?;
            // The server's own CA, and only it: a public authority cannot
            // vouch for a server the user said is signed by this CA.
            builder = builder.tls_certs_only([cert]);
        }
        match (&self.client_cert, &self.client_key) {
            (Some(cert_path), Some(key_path)) => {
                let mut identity_pem = std::fs::read(cert_path).with_context(|| {
                    format!("cannot read the client certificate {}", cert_path.display())
                })?;
                identity_pem.extend_from_slice(&std::fs::read(key_path).with_context(|| {
                    format!("cannot read the client key {}", key_path.display())
                })?);
                let identity = reqwest::Identity::from_pem(&identity_pem)
                    .context("the client certificate and key are not a PEM identity")?;
                builder = builder.identity(identity);
            }
            (None, None) => {}
            _ => anyhow::bail!("mTLS needs both --client-cert and --client-key"),
        }
        Ok(builder)
    }
}

/// Gateway client
pub struct GatewayClient {
    client: Client,
    base_url: String,
    auth_token: Option<String>,
}

impl GatewayClient {
    /// A gateway client verifying the server as `tls` says.
    pub fn with_tls(
        base_url: &str,
        auth_token: Option<String>,
        tls: &TlsConfig,
    ) -> anyhow::Result<Self> {
        use anyhow::Context;
        let builder = tls.apply(Client::builder().timeout(Duration::from_secs(300)))?;

        let client = builder.build().context("cannot set up the HTTP client")?;

        Ok(Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            auth_token,
        })
    }

    /// Add authorization header to a request if token is available
    fn auth_headers(&self) -> Option<String> {
        self.auth_token.as_ref().map(|t| format!("Bearer {}", t))
    }

    /// Verify a dataset's integrity
    pub async fn verify_dataset(
        &self,
        dataset_id: &str,
        check_public: bool,
        full_verification: bool,
    ) -> Result<VerificationResult> {
        let url = format!(
            "{}/api/datasets/{}/verify?check_public={}&full={}",
            self.base_url, dataset_id, check_public, full_verification
        );

        let mut req = self.client.post(&url);
        if let Some(auth) = self.auth_headers() {
            req = req.header("Authorization", auth);
        }

        let response = req.send().await?;

        if response.status().is_success() {
            let result: VerificationResult = response.json().await?;
            Ok(result)
        } else if response.status() == StatusCode::NOT_FOUND {
            Err(ClientError::NotFound(dataset_id.to_string()))
        } else {
            Err(ClientError::Api {
                status: response.status().as_u16(),
                message: response.text().await.unwrap_or_default(),
            })
        }
    }

    /// Share a dataset with another user
    pub async fn share_dataset(
        &self,
        dataset_id: &str,
        share_with: &str,
        permissions: &[String],
    ) -> Result<ShareResult> {
        let url = format!("{}/api/datasets/{}/share", self.base_url, dataset_id);

        #[derive(Serialize)]
        struct ShareRequest<'a> {
            share_with: &'a str,
            permissions: &'a [String],
        }

        let body = ShareRequest {
            share_with,
            permissions,
        };

        let mut req = self.client.post(&url).json(&body);
        if let Some(auth) = self.auth_headers() {
            req = req.header("Authorization", auth);
        }

        let response = req.send().await?;

        if response.status().is_success() {
            let result: ShareResult = response.json().await?;
            Ok(result)
        } else if response.status() == StatusCode::NOT_FOUND {
            Err(ClientError::NotFound(dataset_id.to_string()))
        } else {
            Err(ClientError::Api {
                status: response.status().as_u16(),
                message: response.text().await.unwrap_or_default(),
            })
        }
    }
}
