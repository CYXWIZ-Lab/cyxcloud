//! `front.toml`: where the front door listens and where each path goes.
//! The JWT secrets come from the environment, never from this file.

use serde::Deserialize;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrontConfig {
    /// One port for gRPC and HTTP
    #[serde(default = "default_listen")]
    pub listen: SocketAddr,
    /// TLS certificate chain and key (PEM). Without them the door speaks
    /// plain HTTP/1.1 and HTTP/2 (local and LAN use).
    pub tls_cert: Option<PathBuf>,
    pub tls_key: Option<PathBuf>,
    /// Peers allowed to say who the client is (`client_ip_header`), e.g. the
    /// local Cloudflare tunnel. Anyone else's header is ignored.
    #[serde(default)]
    pub trusted_proxies: Vec<IpAddr>,
    #[serde(default = "default_client_ip_header")]
    pub client_ip_header: String,
    #[serde(default)]
    pub limits: Limits,
    /// Where each path goes; the longest matching prefix wins. Empty means
    /// the local defaults (`default_routes`).
    #[serde(default, rename = "route")]
    pub routes: Vec<Route>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    /// Requests per minute from one client address
    #[serde(default = "default_requests_per_minute")]
    pub requests_per_minute: u32,
    /// Largest body an HTTP route accepts; gRPC streams (dataset uploads)
    /// are capped by the services
    #[serde(default = "default_http_body_bytes")]
    pub http_body_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            requests_per_minute: default_requests_per_minute(),
            http_body_bytes: default_http_body_bytes(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Route {
    /// Name in the audit log ("central", "gateway", "api")
    pub name: String,
    /// Path prefix: a gRPC package ("/cyxcloud.") or an HTTP path ("/api/")
    pub prefix: String,
    /// The service behind it, e.g. "http://127.0.0.1:50060"
    pub upstream: String,
    /// gRPC (HTTP/2 to the service) or plain HTTP
    #[serde(default)]
    pub grpc: bool,
}

// Every address, IPv4 and IPv6 (see `listen`): "localhost" resolves to ::1
// first on Windows, and a client that times out there never tries 127.0.0.1.
fn default_listen() -> SocketAddr {
    "[::]:8000".parse().unwrap()
}
fn default_client_ip_header() -> String {
    "cf-connecting-ip".into()
}
fn default_requests_per_minute() -> u32 {
    1200
}
fn default_http_body_bytes() -> u64 {
    16 * 1024 * 1024
}

/// The local server side (deploy/local): central, gateway and API on their
/// usual ports. Not routed on purpose: the gateway's `/s3` (no sign-in,
/// TOFIX171), every service's `/metrics` and `/health`, the gateway's `/ws`
/// and central's REST (admin, internal).
pub fn default_routes() -> Vec<Route> {
    let r = |name: &str, prefix: &str, upstream: &str, grpc: bool| Route {
        name: name.into(),
        prefix: prefix.into(),
        upstream: upstream.into(),
        grpc,
    };
    vec![
        r("central", "/cyxwiz.", "http://127.0.0.1:50051", true),
        r("gateway", "/cyxcloud.", "http://127.0.0.1:50060", true),
        r("gateway", "/api/datasets/", "http://127.0.0.1:8180", false),
        r("gateway", "/api/v1/auth/", "http://127.0.0.1:8180", false),
        r("api", "/api/", "http://127.0.0.1:3002", false),
    ]
}

impl FrontConfig {
    pub fn from_toml(text: &str) -> anyhow::Result<Self> {
        let mut cfg: FrontConfig = toml::from_str(text)?;
        if cfg.routes.is_empty() {
            cfg.routes = default_routes();
        }
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        if self.tls_cert.is_some() != self.tls_key.is_some() {
            anyhow::bail!("tls_cert and tls_key go together");
        }
        for r in &self.routes {
            if !r.prefix.starts_with('/') {
                anyhow::bail!("route {}: prefix '{}' must start with /", r.name, r.prefix);
            }
            if !(r.upstream.starts_with("http://") || r.upstream.starts_with("https://")) {
                anyhow::bail!(
                    "route {}: upstream '{}' must be an http:// URL",
                    r.name,
                    r.upstream
                );
            }
        }
        if self.limits.requests_per_minute == 0 {
            anyhow::bail!("limits.requests_per_minute must be above 0");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_file_is_the_local_server_side() {
        let cfg = FrontConfig::from_toml("").unwrap();
        assert_eq!(cfg.listen.port(), 8000);
        assert_eq!(cfg.routes, default_routes());
        assert!(cfg.tls_cert.is_none());
    }

    #[test]
    fn mistakes_are_refused() {
        assert!(FrontConfig::from_toml("tls_cert = \"a.pem\"").is_err());
        assert!(FrontConfig::from_toml("listn = \"0.0.0.0:1\"").is_err());
        let bad = "[[route]]\nname = \"x\"\nprefix = \"api\"\nupstream = \"http://a\"\n";
        assert!(FrontConfig::from_toml(bad).is_err());
    }
}
