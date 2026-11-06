//! Where plain (unencrypted) http:// may go. A sign-in token, an API key or
//! a node's access secret sent in clear can be read by anyone on the path,
//! so plain text is for this machine and private networks only; anything
//! else must use https:// (TOFIX192 D3). The Engine applies the same rule
//! (cyxwiz-protocol server_address).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Whether plain http:// may be used to reach `host` (a name or an IP, an
/// IPv6 address with or without brackets): this machine, a private or
/// link-local address, or a LAN name (one label, as `hp-linux` or a compose
/// service like `gateway`, or under .local, .lan, .internal, .home.arpa).
pub fn allowed(host: &str) -> bool {
    let host = host.trim().trim_start_matches('[').trim_end_matches(']');
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() {
        return false;
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        return match ip {
            IpAddr::V4(v4) => private_v4(v4),
            IpAddr::V6(v6) => private_v6(v6),
        };
    }
    if host == "localhost" || host.ends_with(".localhost") {
        return true;
    }
    if !host.contains('.') {
        return true;
    }
    [".local", ".lan", ".internal", ".home.arpa"]
        .iter()
        .any(|suffix| host.ends_with(suffix))
}

/// Whether `host` is this machine (loopback): plain text there never leaves it.
pub fn is_loopback(host: &str) -> bool {
    let host = host.trim().trim_start_matches('[').trim_end_matches(']');
    let host = host.to_ascii_lowercase();
    host == "localhost"
        || host.ends_with(".localhost")
        || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

fn private_v4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        // 100.64.0.0/10, carrier-grade NAT and overlay VPNs (Tailscale)
        || (o[0] == 100 && (o[1] & 0xc0) == 64)
}

fn private_v6(ip: Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return private_v4(v4);
    }
    let first = ip.segments()[0];
    ip.is_loopback()
        // fc00::/7 unique local
        || (first & 0xfe00) == 0xfc00
        // fe80::/10 link local
        || (first & 0xffc0) == 0xfe80
}

/// The host of an `http://` or `https://` URL ("http://[::1]:8000/x" -> "::1").
fn url_host(url: &str) -> Option<&str> {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority
        .rsplit_once('@')
        .map(|(_, a)| a)
        .unwrap_or(authority);
    if let Some(v6) = authority.strip_prefix('[') {
        return v6.split(']').next();
    }
    Some(
        authority
            .rsplit_once(':')
            .map(|(h, _)| h)
            .unwrap_or(authority),
    )
}

/// Refuse a plain `http://` URL (or a bare host, which means plain) to a
/// host that is not this machine or a private network, with what to do
/// instead. An `https://` URL is always fine.
pub fn check_url(url: &str) -> Result<(), String> {
    let url = url.trim();
    if url.starts_with("https://") {
        return Ok(());
    }
    let host = url_host(url).unwrap_or("");
    if allowed(host) {
        Ok(())
    } else {
        Err(format!(
            "{url} is plain http:// to {host}, which is not on this machine or a private network: \
             sign-in tokens would cross the internet unencrypted. Use the server's https:// address."
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_machine_and_private_networks_may_be_plain() {
        for host in [
            "localhost",
            "127.0.0.1",
            "::1",
            "[::1]",
            "10.1.2.3",
            "172.16.0.9",
            "172.31.255.1",
            "192.168.1.180",
            "169.254.10.1",
            "100.100.1.2",
            "fd12:3456::1",
            "fe80::1",
            "::ffff:192.168.1.5",
            "hp-linux",
            "gateway",
            "nas.local",
            "box.lan",
            "svc.internal",
            "printer.home.arpa",
        ] {
            assert!(allowed(host), "{host} should be allowed plain");
        }
    }

    #[test]
    fn the_internet_must_be_https() {
        for host in [
            "8.8.8.8",
            "203.0.113.9",
            "172.32.0.1",
            "100.128.0.1",
            "2001:db8::1",
            "::ffff:8.8.8.8",
            "cloud.cyxwiz.ai",
            "example.com",
            "",
        ] {
            assert!(!allowed(host), "{host} should need https");
        }
    }

    #[test]
    fn urls_are_judged_by_their_host() {
        assert!(check_url("https://8.8.8.8").is_ok());
        assert!(check_url("http://192.168.1.180:8000").is_ok());
        assert!(check_url("http://[::1]:8000/api").is_ok());
        assert!(check_url("http://gateway:50060").is_ok());
        assert!(check_url("192.168.1.180").is_ok());
        let err = check_url("http://cloud.cyxwiz.ai:8000").unwrap_err();
        assert!(err.contains("https://"), "{err}");
        assert!(check_url("http://user@203.0.113.9/x").is_err());
    }

    #[test]
    fn loopback_is_only_this_machine() {
        assert!(is_loopback("localhost"));
        assert!(is_loopback("127.0.0.1"));
        assert!(is_loopback("[::1]"));
        assert!(!is_loopback("192.168.1.180"));
        assert!(!is_loopback("hp-linux"));
    }
}
