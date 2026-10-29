//! `cyxwiz-front`: the CyxWiz server side's one front door (TOFIX168 step 2).
//!
//! One port for every client (the Engine, the cyxcloud CLI, storage and
//! server nodes, the website): TLS when a certificate is configured, the
//! sign-in check once ([`auth`]), a per-client limit and an HTTP body cap
//! ([`limits`]), one audit line per request, and routing by path
//! ([`routing`]) to central, the CyxCloud gateway and the CyxWiz API on a
//! private network. It is a transparent proxy: gRPC streams and trailers
//! pass through unchanged, so no service protocol is compiled in.

pub mod auth;
pub mod config;
pub mod limits;
pub mod routing;

use bytes::Bytes;
use config::{FrontConfig, Route};
use http_body_util::{combinators::BoxBody, BodyExt, Full, Limited};
use hyper::body::Incoming;
use hyper::header::{HeaderName, HeaderValue};
use hyper::{HeaderMap, Request, Response, StatusCode, Version};
use hyper_util::client::legacy::{connect::HttpConnector, Client};
use hyper_util::rt::{TokioExecutor, TokioIo};
use std::convert::Infallible;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Instant;
use tokio::net::TcpListener;
use tracing::{info, warn};

pub type BoxError = Box<dyn std::error::Error + Send + Sync>;
pub type Body = BoxBody<Bytes, BoxError>;

/// gRPC status codes the door answers with itself
mod grpc_code {
    pub const RESOURCE_EXHAUSTED: u16 = 8;
    pub const UNIMPLEMENTED: u16 = 12;
    pub const UNAVAILABLE: u16 = 14;
    pub const UNAUTHENTICATED: u16 = 16;
}

/// Headers that belong to one connection and are not forwarded (RFC 9110
/// 7.6.1). `te: trailers` is kept for gRPC, which needs it.
const HOP_BY_HOP: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-connection",
    "proxy-authenticate",
    "proxy-authorization",
    "transfer-encoding",
    "upgrade",
    "host",
];

pub struct Front {
    config: FrontConfig,
    tokens: auth::TokenCheck,
    limiter: limits::RateLimiter,
    grpc_client: Client<HttpConnector, Body>,
    http_client: Client<HttpConnector, Body>,
    tls: bool,
}

/// What the door decided, for the audit line.
struct Outcome {
    response: Response<Body>,
    route: &'static str,
    account: Option<String>,
    /// gRPC status the door answered with itself (proxied gRPC carries its
    /// status in trailers the door does not read)
    grpc_status: Option<u16>,
}

fn full(text: impl Into<Bytes>) -> Body {
    Full::new(text.into())
        .map_err(|never: Infallible| match never {})
        .boxed()
}

fn http_answer(status: StatusCode, message: &str) -> Response<Body> {
    let mut r = Response::new(full(format!(
        "{{\"error\":{}}}\n",
        serde_json::json!(message)
    )));
    *r.status_mut() = status;
    r.headers_mut().insert(
        hyper::header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    r
}

/// A gRPC "trailers-only" answer: HTTP 200 with the status in the headers.
fn grpc_answer(code: u16, message: &str) -> Response<Body> {
    let mut r = Response::new(full(Bytes::new()));
    let h = r.headers_mut();
    h.insert(
        hyper::header::CONTENT_TYPE,
        HeaderValue::from_static("application/grpc"),
    );
    h.insert("grpc-status", HeaderValue::from(code));
    if let Ok(v) = HeaderValue::from_str(&percent_encode(message)) {
        h.insert("grpc-message", v);
    }
    r
}

/// grpc-message is percent-encoded outside printable ASCII (and `%`).
fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for b in text.bytes() {
        if (0x20..0x7f).contains(&b) && b != b'%' {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

fn strip_hop_by_hop(headers: &mut HeaderMap, grpc: bool) {
    for name in HOP_BY_HOP {
        headers.remove(*name);
    }
    if !grpc {
        headers.remove("te");
    }
}

impl Front {
    pub fn new(config: FrontConfig, jwt_secrets: &[String]) -> anyhow::Result<Self> {
        config.validate()?;
        let tokens = auth::TokenCheck::new(jwt_secrets)?;
        let limiter = limits::RateLimiter::new(config.limits.requests_per_minute);
        let grpc_client = Client::builder(TokioExecutor::new())
            .http2_only(true)
            .build_http();
        let http_client = Client::builder(TokioExecutor::new()).build_http();
        let tls = config.tls_cert.is_some();
        Ok(Self {
            config,
            tokens,
            limiter,
            grpc_client,
            http_client,
            tls,
        })
    }

    pub fn config(&self) -> &FrontConfig {
        &self.config
    }

    /// Answer one request and write its audit line.
    pub async fn handle(&self, req: Request<Incoming>, peer: SocketAddr) -> Response<Body> {
        let started = Instant::now();
        let ip = limits::client_ip(
            req.headers(),
            peer.ip(),
            &self.config.trusted_proxies,
            &self.config.client_ip_header,
        );
        let method = req.method().clone();
        let path = req.uri().path().to_string();
        let grpc = routing::is_grpc(req.headers());
        let outcome = self.decide(req, ip, grpc).await;
        info!(
            target: "audit",
            ip = %ip,
            method = %method,
            path = %path,
            route = outcome.route,
            status = outcome.response.status().as_u16(),
            grpc_status = outcome.grpc_status,
            account = outcome.account.as_deref().unwrap_or("-"),
            ms = started.elapsed().as_millis() as u64,
        );
        outcome.response
    }

    async fn decide(&self, req: Request<Incoming>, ip: IpAddr, grpc: bool) -> Outcome {
        let refuse = |status: StatusCode, code: u16, message: &str| -> Outcome {
            Outcome {
                response: if grpc {
                    grpc_answer(code, message)
                } else {
                    http_answer(status, message)
                },
                route: "-",
                account: None,
                grpc_status: grpc.then_some(code),
            }
        };

        if !self.limiter.allow(ip, Instant::now()) {
            return refuse(
                StatusCode::TOO_MANY_REQUESTS,
                grpc_code::RESOURCE_EXHAUSTED,
                "too many requests from this address: slow down",
            );
        }
        if !grpc && req.uri().path() == "/health" {
            return Outcome {
                response: self.health().await,
                route: "front",
                account: None,
                grpc_status: None,
            };
        }
        let Some(route) = routing::route_for(&self.config.routes, req.uri().path(), grpc) else {
            return refuse(
                StatusCode::NOT_FOUND,
                grpc_code::UNIMPLEMENTED,
                "not served here",
            );
        };
        let route_name: &'static str = match route.name.as_str() {
            "central" => "central",
            "gateway" => "gateway",
            "api" => "api",
            _ => "other",
        };
        let authorization = req
            .headers()
            .get(hyper::header::AUTHORIZATION)
            .map(|v| v.to_str().unwrap_or("\u{0}"));
        let account = match self.tokens.check(authorization) {
            Ok(account) => account,
            Err(refusal) => {
                let mut o = refuse(
                    StatusCode::UNAUTHORIZED,
                    grpc_code::UNAUTHENTICATED,
                    refusal.message(),
                );
                o.route = route_name;
                return o;
            }
        };
        if !grpc {
            let too_big = req
                .headers()
                .get(hyper::header::CONTENT_LENGTH)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .is_some_and(|n| n > self.config.limits.http_body_bytes);
            if too_big {
                let mut o = refuse(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    0,
                    "the request body is larger than this server accepts",
                );
                o.route = route_name;
                o.account = account;
                return o;
            }
        }
        let response = self.forward(req, route, ip, grpc).await;
        Outcome {
            response,
            route: route_name,
            account,
            grpc_status: None,
        }
    }

    /// `GET /health`: the door, and whether it reaches each service behind
    /// it (a TCP connect, 2 s each). 503 when one is down, so `cyxcloud
    /// doctor` through the door sees a dead service. Names only, no
    /// addresses.
    async fn health(&self) -> Response<Body> {
        let mut services = serde_json::Map::new();
        let mut all_up = true;
        for route in &self.config.routes {
            if services.contains_key(&route.name) {
                continue;
            }
            let up = match route
                .upstream
                .parse::<hyper::Uri>()
                .ok()
                .and_then(|u| Some((u.host()?.to_string(), u.port_u16().unwrap_or(80))))
            {
                Some((host, port)) => matches!(
                    tokio::time::timeout(
                        std::time::Duration::from_secs(2),
                        tokio::net::TcpStream::connect((host.as_str(), port)),
                    )
                    .await,
                    Ok(Ok(_))
                ),
                None => false,
            };
            all_up &= up;
            services.insert(route.name.clone(), (if up { "up" } else { "down" }).into());
        }
        let body = serde_json::json!({ "front": "ok", "services": services });
        let mut r = Response::new(full(format!("{}\n", body)));
        if !all_up {
            *r.status_mut() = StatusCode::SERVICE_UNAVAILABLE;
        }
        r.headers_mut().insert(
            hyper::header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        r
    }

    async fn forward(
        &self,
        req: Request<Incoming>,
        route: &Route,
        ip: IpAddr,
        grpc: bool,
    ) -> Response<Body> {
        let (mut parts, body) = req.into_parts();
        let path_and_query = parts
            .uri
            .path_and_query()
            .map(|p| p.as_str())
            .unwrap_or("/");
        let target = format!("{}{}", route.upstream.trim_end_matches('/'), path_and_query);
        let Ok(uri) = target.parse::<hyper::Uri>() else {
            return http_answer(StatusCode::BAD_REQUEST, "this path cannot be forwarded");
        };
        let original_host = parts.headers.get(hyper::header::HOST).cloned().or_else(|| {
            parts
                .uri
                .authority()
                .and_then(|a| HeaderValue::from_str(a.as_str()).ok())
        });
        parts.uri = uri;
        parts.version = if grpc {
            Version::HTTP_2
        } else {
            Version::HTTP_11
        };
        strip_hop_by_hop(&mut parts.headers, grpc);
        let forwarded_for = match parts
            .headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
        {
            Some(chain) => format!("{}, {}", chain, ip),
            None => ip.to_string(),
        };
        if let Ok(v) = HeaderValue::from_str(&forwarded_for) {
            parts.headers.insert("x-forwarded-for", v);
        }
        parts.headers.insert(
            HeaderName::from_static("x-forwarded-proto"),
            HeaderValue::from_static(if self.tls { "https" } else { "http" }),
        );
        if let Some(host) = original_host {
            parts.headers.insert("x-forwarded-host", host);
        }

        let body: Body = if grpc {
            body.map_err(|e| Box::new(e) as BoxError).boxed()
        } else {
            // a body without a declared length is cut at the cap
            Limited::new(body, self.config.limits.http_body_bytes as usize).boxed()
        };
        let client = if grpc {
            &self.grpc_client
        } else {
            &self.http_client
        };
        match client.request(Request::from_parts(parts, body)).await {
            Ok(response) => {
                let (mut parts, body) = response.into_parts();
                strip_hop_by_hop(&mut parts.headers, grpc);
                Response::from_parts(parts, body.map_err(|e| Box::new(e) as BoxError).boxed())
            }
            Err(e) => {
                warn!(route = %route.name, upstream = %route.upstream, error = %e, "service not reachable");
                let message = format!("the {} service is not reachable", route.name);
                if grpc {
                    grpc_answer(grpc_code::UNAVAILABLE, &message)
                } else {
                    http_answer(StatusCode::BAD_GATEWAY, &message)
                }
            }
        }
    }
}

/// TLS for the door: the certificate chain and key from the config, with
/// ALPN for HTTP/2 (gRPC) and HTTP/1.1.
pub fn tls_acceptor(config: &FrontConfig) -> anyhow::Result<Option<tokio_rustls::TlsAcceptor>> {
    let (Some(cert), Some(key)) = (&config.tls_cert, &config.tls_key) else {
        return Ok(None);
    };
    let certs = rustls_pemfile::certs(&mut std::io::BufReader::new(
        std::fs::File::open(cert).map_err(|e| anyhow::anyhow!("{}: {}", cert.display(), e))?,
    ))
    .collect::<Result<Vec<_>, _>>()?;
    let key = rustls_pemfile::private_key(&mut std::io::BufReader::new(
        std::fs::File::open(key).map_err(|e| anyhow::anyhow!("{}: {}", key.display(), e))?,
    ))?
    .ok_or_else(|| anyhow::anyhow!("{}: no private key", key.display()))?;
    let mut server = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)?;
    server.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok(Some(tokio_rustls::TlsAcceptor::from(Arc::new(server))))
}

/// Accept connections until the listener fails.
pub async fn serve(
    listener: TcpListener,
    front: Arc<Front>,
    tls: Option<tokio_rustls::TlsAcceptor>,
) -> anyhow::Result<()> {
    loop {
        let (stream, peer) = listener.accept().await?;
        let front = front.clone();
        let tls = tls.clone();
        tokio::spawn(async move {
            let service = hyper::service::service_fn(move |req| {
                let front = front.clone();
                async move { Ok::<_, Infallible>(front.handle(req, peer).await) }
            });
            let builder = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new());
            let result = match tls {
                Some(acceptor) => match acceptor.accept(stream).await {
                    Ok(s) => builder.serve_connection(TokioIo::new(s), service).await,
                    Err(e) => {
                        tracing::debug!(peer = %peer, error = %e, "TLS handshake failed");
                        return;
                    }
                },
                None => {
                    builder
                        .serve_connection(TokioIo::new(stream), service)
                        .await
                }
            };
            if let Err(e) = result {
                tracing::debug!(peer = %peer, error = %e, "connection ended with an error");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grpc_messages_are_percent_encoded() {
        assert_eq!(percent_encode("not served here"), "not served here");
        assert_eq!(percent_encode("100% é"), "100%25 %C3%A9");
    }

    #[test]
    fn a_grpc_refusal_is_trailers_only() {
        let r = grpc_answer(grpc_code::UNAUTHENTICATED, "sign in again");
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(r.headers()["grpc-status"], "16");
        assert_eq!(r.headers()["content-type"], "application/grpc");
    }
}
