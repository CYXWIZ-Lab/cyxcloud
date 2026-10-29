//! `cyxcloud doctor`: check everything between this machine and the server,
//! say what is wrong and what to do (TOFIX177).
//!
//! Exit codes: 0 all good (warnings allowed), 1 something for the user to do
//! (sign in, set up), 2 the network or the server cannot be reached.

use crate::commands::node;
use crate::config;
use crate::cyxwiz_client::CyxWizClient;
use crate::server::{human_bytes, human_duration};
use crate::symbols;
use anyhow::Result;
use console::style;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Outcome {
    Ok,
    Warn,
    /// The user has to do something
    Fix,
    /// Network or server
    Down,
}

#[derive(Debug, Clone)]
pub struct Check {
    pub outcome: Outcome,
    pub text: String,
}

impl Check {
    fn ok(text: impl Into<String>) -> Self {
        Self {
            outcome: Outcome::Ok,
            text: text.into(),
        }
    }
    fn warn(text: impl Into<String>) -> Self {
        Self {
            outcome: Outcome::Warn,
            text: text.into(),
        }
    }
    fn fix(text: impl Into<String>) -> Self {
        Self {
            outcome: Outcome::Fix,
            text: text.into(),
        }
    }
    fn down(text: impl Into<String>) -> Self {
        Self {
            outcome: Outcome::Down,
            text: text.into(),
        }
    }
}

/// The exit code for a list of checks.
pub fn exit_code(checks: &[Check]) -> i32 {
    match checks.iter().map(|c| c.outcome).max() {
        Some(Outcome::Down) => crate::exit::Kind::Network.code(),
        Some(Outcome::Fix) => crate::exit::Kind::User.code(),
        _ => 0,
    }
}

/// Seconds between our clock and an HTTP `Date` header (positive = we are ahead).
pub fn clock_skew_secs(http_date: &str, now: chrono::DateTime<chrono::Utc>) -> Option<i64> {
    let server = chrono::DateTime::parse_from_rfc2822(http_date).ok()?;
    Some((now - server.with_timezone(&chrono::Utc)).num_seconds())
}

/// Offered space against what the disk can hold; warn under 10 % headroom.
pub fn disk_check(offered: u64, used: u64, free: u64, total: u64, data_dir: &str) -> Check {
    if total == 0 {
        return Check::warn(format!(
            "{}: could not read the disk's free space",
            data_dir
        ));
    }
    let room = free + used;
    if offered > room {
        Check::fix(format!(
            "{}: you offer {} but only {} can ever be stored there; lower the offer (cyxcloud node init --force --offer ...)",
            data_dir,
            human_bytes(offered),
            human_bytes(room)
        ))
    } else if offered > room * 9 / 10 {
        Check::warn(format!(
            "{}: {} free of {}, you offer {} (leave 10% headroom)",
            data_dir,
            human_bytes(free),
            human_bytes(total),
            human_bytes(offered)
        ))
    } else {
        Check::ok(format!(
            "{}: {} free of {}, you offer {}",
            data_dir,
            human_bytes(free),
            human_bytes(total),
            human_bytes(offered)
        ))
    }
}

/// Run every check and print it; returns the exit code.
pub async fn run(cyxwiz: &CyxWizClient) -> Result<i32> {
    let cfg = config::load_config();
    let mut checks = Vec::new();

    checks.push(Check::ok(format!(
        "cyxcloud {} ({}-{})",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    )));
    match &cfg.server.host {
        Some(host) => checks.push(Check::ok(format!("server {}", host))),
        None => checks.push(Check::warn(
            "no server set: run `cyxcloud login --server HOST` (using the separate gateway/API addresses from the config)",
        )),
    }

    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()?;

    // Gateway
    let started = Instant::now();
    match http.get(format!("{}/health", cfg.gateway.http_url)).send().await {
        Ok(resp) if resp.status().is_success() => {
            let ms = started.elapsed().as_millis();
            let tls = if cfg.gateway.http_url.starts_with("https://") {
                "TLS, certificate verified"
            } else {
                "no TLS"
            };
            checks.push(Check::ok(format!(
                "storage gateway {} reachable in {} ms ({})",
                cfg.gateway.http_url, ms, tls
            )));
            if let Some(date) = resp.headers().get("date").and_then(|d| d.to_str().ok()) {
                if let Some(skew) = clock_skew_secs(date, chrono::Utc::now()) {
                    if skew.abs() > 60 {
                        checks.push(Check::fix(format!(
                            "this clock is {} s off the server's: fix the system time (node registrations allow 300 s)",
                            skew
                        )));
                    } else {
                        checks.push(Check::ok(format!("clock within {} s of the server", skew.abs())));
                    }
                }
            }
        }
        Ok(resp) => checks.push(Check::down(format!(
            "storage gateway {} answered {}",
            cfg.gateway.http_url,
            resp.status()
        ))),
        Err(e) => checks.push(Check::down(format!(
            "cannot reach the storage gateway {}: {} (check the address with `cyxcloud config show` or your network)",
            cfg.gateway.http_url,
            short_error(&e)
        ))),
    }

    // Sign-in server
    match http
        .get(format!("{}/health", cfg.auth.api_url))
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => checks.push(Check::ok(format!(
            "sign-in server {} reachable",
            cfg.auth.api_url
        ))),
        Ok(resp) => checks.push(Check::down(format!(
            "sign-in server {} answered {}",
            cfg.auth.api_url,
            resp.status()
        ))),
        Err(e) => checks.push(Check::down(format!(
            "cannot reach the sign-in server {}: {}",
            cfg.auth.api_url,
            short_error(&e)
        ))),
    }

    // Account
    match config::get_valid_credentials() {
        Some(creds) => {
            let left = creds.expires_at - chrono::Utc::now().timestamp();
            match cyxwiz.get_profile(&creds.access_token).await {
                Ok(profile) => checks.push(Check::ok(format!(
                    "signed in as {} · sign-in valid for {}",
                    profile.username.unwrap_or(profile.email),
                    human_duration(left)
                ))),
                Err(crate::cyxwiz_client::CyxWizError::InvalidCredentials) => checks.push(
                    Check::fix("the saved sign-in was refused: run `cyxcloud login`"),
                ),
                Err(e) => checks.push(Check::warn(format!(
                    "signed in as {} (could not confirm with the server: {})",
                    creds.email, e
                ))),
            }
        }
        None => checks.push(Check::fix(
            "not signed in: run `cyxcloud login --server HOST`",
        )),
    }

    // Node on this machine
    if let Ok(path) = node::node_config_path() {
        if path.exists() {
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            let port = node::status_port_of(&text);
            let report = reqwest::Client::builder()
                .timeout(Duration::from_secs(3))
                .build()?
                .get(format!("http://127.0.0.1:{}/status", port))
                .send()
                .await
                .ok();
            let report = match report {
                Some(r) => r
                    .json::<cyxcloud_core::node_status::NodeStatusReport>()
                    .await
                    .ok(),
                None => None,
            };
            match report {
                Some(r) => {
                    if r.server.registered {
                        checks.push(Check::ok(format!(
                            "node {} is running and registered",
                            r.name
                        )));
                    } else {
                        checks.push(Check::fix(format!(
                            "node {} is running but not registered: {}",
                            r.name, r.server.last_error
                        )));
                    }
                    checks.push(port_check(&r.listen_addr));
                    checks.push(disk_check(
                        r.storage.offered_bytes,
                        r.storage.used_bytes,
                        r.storage.disk_free_bytes,
                        r.storage.disk_total_bytes,
                        &r.storage.data_dir,
                    ));
                }
                None => checks.push(Check::warn(
                    "a node is set up here but not running: `cyxcloud node start`",
                )),
            }
        }
    }

    for c in &checks {
        let mark = match c.outcome {
            Outcome::Ok => style(symbols::CHECK).green(),
            Outcome::Warn => style(symbols::WARN).yellow(),
            Outcome::Fix | Outcome::Down => style(symbols::CROSS).red(),
        };
        println!("{} {}", mark, c.text);
    }
    Ok(exit_code(&checks))
}

/// The node's listen address must accept connections (the server dials it).
fn port_check(listen_addr: &str) -> Check {
    let addr = listen_addr.replace("0.0.0.0", "127.0.0.1");
    match std::net::ToSocketAddrs::to_socket_addrs(&addr.as_str())
        .ok()
        .and_then(|mut a| a.next())
    {
        Some(sock) => match std::net::TcpStream::connect_timeout(&sock, Duration::from_secs(3)) {
            Ok(_) => Check::ok(format!("node port {} accepts connections", listen_addr)),
            Err(e) => Check::fix(format!(
                "node port {} does not accept connections ({}): open it in the firewall",
                listen_addr, e
            )),
        },
        None => Check::warn(format!(
            "node listen address '{}' is not an address",
            listen_addr
        )),
    }
}

fn short_error(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "timed out".to_string()
    } else if e.is_connect() {
        "connection refused or no route".to_string()
    } else {
        e.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_worst_outcome_decides_the_exit_code() {
        assert_eq!(exit_code(&[Check::ok("a"), Check::warn("b")]), 0);
        assert_eq!(exit_code(&[Check::ok("a"), Check::fix("b")]), 1);
        assert_eq!(exit_code(&[Check::fix("a"), Check::down("b")]), 2);
        assert_eq!(exit_code(&[]), 0);
    }

    #[test]
    fn clock_skew_is_read_from_the_date_header() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-10-26T12:00:30Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        assert_eq!(
            clock_skew_secs("Mon, 26 Oct 2026 12:00:00 GMT", now),
            Some(30)
        );
        assert_eq!(clock_skew_secs("not a date", now), None);
    }

    #[test]
    fn disk_headroom_warns_and_impossible_offers_are_for_the_user() {
        let gb = 1u64 << 30;
        assert_eq!(
            disk_check(700 * gb, 0, 830 * gb, 915 * gb, "/d").outcome,
            Outcome::Ok
        );
        assert_eq!(
            disk_check(800 * gb, 0, 830 * gb, 915 * gb, "/d").outcome,
            Outcome::Warn
        );
        assert_eq!(
            disk_check(900 * gb, 0, 830 * gb, 915 * gb, "/d").outcome,
            Outcome::Fix
        );
        assert_eq!(disk_check(1, 0, 0, 0, "/d").outcome, Outcome::Warn);
    }

    #[test]
    fn a_closed_port_is_reported_with_the_fix() {
        let c = port_check("127.0.0.1:1");
        assert_eq!(c.outcome, Outcome::Fix);
        assert!(c.text.contains("firewall"));
    }
}
