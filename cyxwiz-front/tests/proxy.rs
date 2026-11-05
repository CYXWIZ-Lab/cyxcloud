//! The front door in front of a stand-in service, over real sockets.

use bytes::Bytes;
use cyxwiz_front::config::{FrontConfig, Limits, Route};
use cyxwiz_front::{serve, tls_acceptor, Front};
use http_body_util::Full;
use hyper::{Request, Response};
use hyper_util::rt::{TokioExecutor, TokioIo};
use std::convert::Infallible;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::net::TcpListener;

const SECRET: &str = "test-secret";

/// A service that answers with what it was sent.
async fn stand_in(hits: Arc<AtomicUsize>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let hits = hits.clone();
            tokio::spawn(async move {
                let svc = hyper::service::service_fn(move |req: Request<hyper::body::Incoming>| {
                    hits.fetch_add(1, Ordering::SeqCst);
                    let echo = format!(
                        "path={} xff={} proto={}",
                        req.uri().path_and_query().map(|p| p.as_str()).unwrap_or(""),
                        req.headers()
                            .get("x-forwarded-for")
                            .and_then(|v| v.to_str().ok())
                            .unwrap_or("-"),
                        req.headers()
                            .get("x-forwarded-proto")
                            .and_then(|v| v.to_str().ok())
                            .unwrap_or("-"),
                    );
                    async move { Ok::<_, Infallible>(Response::new(Full::new(Bytes::from(echo)))) }
                });
                let _ = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new())
                    .serve_connection(TokioIo::new(stream), svc)
                    .await;
            });
        }
    });
    port
}

async fn front_door(upstream_port: u16) -> String {
    format!(
        "http://{}",
        front_door_at(upstream_port, "127.0.0.1:0").await
    )
}

async fn front_door_at(upstream_port: u16, listen: &str) -> std::net::SocketAddr {
    let config = FrontConfig {
        listen: listen.parse().unwrap(),
        tls_cert: None,
        tls_key: None,
        internal_listen: None,
        trusted_proxies: vec![],
        client_ip_header: "cf-connecting-ip".into(),
        limits: Limits {
            requests_per_minute: 1000,
            http_body_bytes: 1024,
        },
        routes: vec![
            Route {
                name: "api".into(),
                prefix: "/api/".into(),
                upstream: format!("http://127.0.0.1:{}", upstream_port),
                grpc: false,
                body_bytes: None,
            },
            Route {
                name: "big".into(),
                prefix: "/big/".into(),
                upstream: format!("http://127.0.0.1:{}", upstream_port),
                grpc: false,
                body_bytes: Some(8192),
            },
            Route {
                name: "gateway".into(),
                prefix: "/down/".into(),
                upstream: "http://127.0.0.1:9".into(),
                grpc: false,
                body_bytes: None,
            },
        ],
    };
    let listener = cyxwiz_front::listen(config.listen).unwrap();
    let addr = listener.local_addr().unwrap();
    let front = Arc::new(Front::new(config, &[SECRET.into()]).unwrap());
    tokio::spawn(serve(listener, front, None));
    addr
}

/// reqwest builds on the process's rustls provider; the door's main
/// installs ring, a test does it here.
fn http_client() -> reqwest::Client {
    let _ = rustls::crypto::ring::default_provider().install_default();
    reqwest::Client::new()
}

fn token(secret: &str) -> String {
    let exp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 300;
    jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &serde_json::json!({ "sub": "u1", "exp": exp }),
        &jsonwebtoken::EncodingKey::from_secret(secret.as_bytes()),
    )
    .unwrap()
}

/// TLS outside, plain inside: the same door on two listeners. The service
/// learns which one a request came through (X-Forwarded-Proto), and the TLS
/// port refuses plain HTTP.
#[tokio::test]
async fn tls_outside_plain_inside() {
    let hits = Arc::new(AtomicUsize::new(0));
    let up = stand_in(hits.clone()).await;

    let dir = tempfile::tempdir().unwrap();
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let cert_path = dir.path().join("cert.pem");
    let key_path = dir.path().join("key.pem");
    std::fs::write(&cert_path, cert.cert.pem()).unwrap();
    std::fs::write(&key_path, cert.signing_key.serialize_pem()).unwrap();

    let public = cyxwiz_front::listen("127.0.0.1:0".parse().unwrap()).unwrap();
    let internal = cyxwiz_front::listen("127.0.0.1:0".parse().unwrap()).unwrap();
    let (public_addr, internal_addr) =
        (public.local_addr().unwrap(), internal.local_addr().unwrap());
    let config = FrontConfig {
        listen: public_addr,
        tls_cert: Some(cert_path),
        tls_key: Some(key_path),
        internal_listen: Some(internal_addr),
        trusted_proxies: vec![],
        client_ip_header: "cf-connecting-ip".into(),
        limits: Limits {
            requests_per_minute: 1000,
            http_body_bytes: 1024,
        },
        routes: vec![Route {
            name: "api".into(),
            prefix: "/api/".into(),
            upstream: format!("http://127.0.0.1:{}", up),
            grpc: false,
            body_bytes: None,
        }],
    };
    let _ = rustls::crypto::ring::default_provider().install_default();
    let tls = tls_acceptor(&config).unwrap();
    assert!(tls.is_some());
    let front = Arc::new(Front::new(config, &[SECRET.into()]).unwrap());
    tokio::spawn(serve(public, front.clone(), tls));
    tokio::spawn(serve(internal, front, None));

    let trusting = reqwest::Client::builder()
        .add_root_certificate(reqwest::Certificate::from_pem(cert.cert.pem().as_bytes()).unwrap())
        .build()
        .unwrap();
    let r = trusting
        .get(format!("https://localhost:{}/api/x", public_addr.port()))
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.text().await.unwrap(),
        "path=/api/x xff=127.0.0.1 proto=https"
    );

    let r = http_client()
        .get(format!("http://{}/api/x", internal_addr))
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.text().await.unwrap(),
        "path=/api/x xff=127.0.0.1 proto=http"
    );

    // plain HTTP on the TLS port is not served
    assert!(http_client()
        .get(format!("http://{}/api/x", public_addr))
        .send()
        .await
        .is_err());
    // a client that does not trust the certificate gets no answer
    assert!(http_client()
        .get(format!("https://localhost:{}/api/x", public_addr.port()))
        .send()
        .await
        .is_err());
    assert_eq!(hits.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn the_door_routes_checks_and_refuses() {
    let hits = Arc::new(AtomicUsize::new(0));
    let up = stand_in(hits.clone()).await;
    let door = front_door(up).await;
    let http = http_client();

    // routed, with the path, query and the client's address forwarded
    let r = http
        .get(format!("{}/api/users/me?x=1", door))
        .bearer_auth(token(SECRET))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(
        r.text().await.unwrap(),
        "path=/api/users/me?x=1 xff=127.0.0.1 proto=http"
    );

    // no token: the service decides
    let r = http
        .get(format!("{}/api/models", door))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(hits.load(Ordering::SeqCst), 2);

    // a bad token stops at the door and never reaches the service
    let r = http
        .get(format!("{}/api/users/me", door))
        .bearer_auth(token("not-our-secret"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    assert_eq!(hits.load(Ordering::SeqCst), 2);

    // what is not routed is not served
    let r = http.get(format!("{}/metrics", door)).send().await.unwrap();
    assert_eq!(r.status(), 404);

    // a body above the cap
    let r = http
        .post(format!("{}/api/models", door))
        .body(vec![0u8; 4096])
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 413);
    assert_eq!(hits.load(Ordering::SeqCst), 2);

    // a route with its own, larger cap takes the same body
    let r = http
        .post(format!("{}/big/x", door))
        .body(vec![0u8; 4096])
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(hits.load(Ordering::SeqCst), 3);

    // the door's health names the service that is down
    let r = http.get(format!("{}/health", door)).send().await.unwrap();
    assert_eq!(r.status(), 503);
    let health: serde_json::Value = serde_json::from_str(&r.text().await.unwrap()).unwrap();
    assert_eq!(health["services"]["api"], "up");
    assert_eq!(health["services"]["gateway"], "down");
    let r = http.get(format!("{}/down/x", door)).send().await.unwrap();
    assert_eq!(r.status(), 502);
}

// "localhost" is ::1 first on Windows: the door's default address answers on
// IPv4 and IPv6, and an IPv4 client is forwarded as its plain address.
#[tokio::test]
async fn the_door_answers_on_ipv4_and_ipv6() {
    let up = stand_in(Arc::new(AtomicUsize::new(0))).await;
    let port = front_door_at(up, "[::]:0").await.port();
    let http = http_client();
    let r = http
        .get(format!("http://127.0.0.1:{}/api/x", port))
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.text().await.unwrap(),
        "path=/api/x xff=127.0.0.1 proto=http"
    );
    let r = http
        .get(format!("http://[::1]:{}/api/x", port))
        .send()
        .await
        .unwrap();
    assert_eq!(r.text().await.unwrap(), "path=/api/x xff=::1 proto=http");
}
