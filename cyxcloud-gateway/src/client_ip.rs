//! The client's address, for per-client limits.
//!
//! Behind the front door (or any proxy) the connection's peer is the proxy
//! and the client is in X-Forwarded-For. Each proxy appends the address it
//! saw, so only the last `TRUSTED_PROXY_HOPS` entries were written by
//! proxies we run; anything to their left came from the client, who could
//! otherwise name a new address on every request to dodge a limit.

use std::net::IpAddr;
use std::sync::OnceLock;

/// How many proxies of ours stand in front of the gateway (0: none, the
/// connection's peer is the client).
pub const TRUSTED_PROXY_HOPS_ENV: &str = "TRUSTED_PROXY_HOPS";

pub fn trusted_proxy_hops() -> usize {
    static HOPS: OnceLock<usize> = OnceLock::new();
    *HOPS.get_or_init(|| {
        std::env::var(TRUSTED_PROXY_HOPS_ENV)
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(0)
    })
}

/// The client's address: the entry the outermost trusted proxy appended, or
/// the connection's peer when there is no proxy (or its entry is missing).
pub fn client_ip(forwarded_for: Option<&str>, peer: Option<IpAddr>, hops: usize) -> Option<IpAddr> {
    if hops == 0 {
        return peer;
    }
    let entries: Vec<&str> = forwarded_for
        .map(|v| v.split(',').map(str::trim).collect())
        .unwrap_or_default();
    entries
        .len()
        .checked_sub(hops)
        .and_then(|i| entries[i].parse().ok())
        .or(peer)
}

/// `client_ip` as a rate-limit key ("unknown" when there is no address).
pub fn client_key(forwarded_for: Option<&str>, peer: Option<IpAddr>) -> String {
    client_ip(forwarded_for, peer, trusted_proxy_hops())
        .map(|ip| ip.to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn without_a_proxy_the_peer_is_the_client_and_the_header_is_ignored() {
        let peer = Some(ip("198.51.100.4"));
        assert_eq!(client_ip(Some("203.0.113.9"), peer, 0), peer);
    }

    #[test]
    fn behind_one_proxy_its_entry_counts_not_the_clients() {
        let proxy = Some(ip("172.18.0.5"));
        // the client sent "1.2.3.4"; the front door appended what it saw
        assert_eq!(
            client_ip(Some("1.2.3.4, 203.0.113.9"), proxy, 1),
            Some(ip("203.0.113.9"))
        );
        assert_eq!(
            client_ip(Some("203.0.113.9"), proxy, 1),
            Some(ip("203.0.113.9"))
        );
    }

    #[test]
    fn behind_two_proxies_the_outer_ones_entry_counts() {
        let proxy = Some(ip("172.18.0.5"));
        assert_eq!(
            client_ip(Some("1.2.3.4, 203.0.113.9, 10.0.0.2"), proxy, 2),
            Some(ip("203.0.113.9"))
        );
    }

    #[test]
    fn a_missing_or_short_header_falls_back_to_the_peer() {
        let proxy = Some(ip("172.18.0.5"));
        assert_eq!(client_ip(None, proxy, 1), proxy);
        assert_eq!(client_ip(Some("203.0.113.9"), proxy, 2), proxy);
        assert_eq!(client_ip(Some("not-an-address"), proxy, 1), proxy);
    }
}
