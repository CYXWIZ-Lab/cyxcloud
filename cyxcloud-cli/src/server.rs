//! One server address for the user (TOFIX177, TOFIX168): `cyxcloud login
//! --server HOST` and `cyxcloud config set server HOST` set it once; the CLI
//! and the node derive everything else from it.
//!
//! A front door (TOFIX168 `cyxwiz-front`) serves everything at one URL:
//! `https://HOST[:PORT]`, or `http://HOST:PORT` for one without TLS (local
//! or LAN). A plain host (no scheme, or `http://` without a port) means the
//! server side without a front door: the three services on their own ports
//! (gateway HTTP 8180, gateway gRPC 50060, CyxWiz API 3002).

/// Where the CLI and the node reach the server side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerUrls {
    /// Dataset REST and S3 (gateway HTTP)
    pub gateway_http: String,
    /// Node registration and datasets over gRPC (gateway gRPC)
    pub gateway_grpc: String,
    /// Sign-in (CyxWiz API)
    pub api: String,
}

pub const GATEWAY_HTTP_PORT: u16 = 8180;
pub const GATEWAY_GRPC_PORT: u16 = 50060;
pub const API_PORT: u16 = 3002;

/// Map what the user typed to the service URLs.
pub fn urls_for_server(server: &str) -> Result<ServerUrls, String> {
    let server = server.trim().trim_end_matches('/');
    if server.is_empty() {
        return Err("the server address is empty".into());
    }
    let front_door = |scheme: &str, rest: &str| -> Result<ServerUrls, String> {
        if rest.is_empty() || rest.contains('/') {
            return Err(format!(
                "'{}' is not a server address (expected {}://HOST:PORT)",
                server, scheme
            ));
        }
        let one = format!("{}://{}", scheme, rest);
        Ok(ServerUrls {
            gateway_http: one.clone(),
            gateway_grpc: one.clone(),
            api: one,
        })
    };
    if let Some(rest) = server.strip_prefix("https://") {
        return front_door("https", rest);
    }
    if let Some(rest) = server.strip_prefix("http://") {
        if rest.contains(':') {
            return front_door("http", rest);
        }
    }
    let host = server.strip_prefix("http://").unwrap_or(server);
    if host.contains('/') || host.contains(':') {
        return Err(format!(
            "'{}': give the server's host name or IP, or a front door as http://HOST:PORT or https://HOST",
            server
        ));
    }
    Ok(ServerUrls {
        gateway_http: format!("http://{}:{}", host, GATEWAY_HTTP_PORT),
        gateway_grpc: format!("http://{}:{}", host, GATEWAY_GRPC_PORT),
        api: format!("http://{}:{}", host, API_PORT),
    })
}

/// Host part of a URL ("http://192.168.1.116:50060" -> "192.168.1.116").
pub fn url_host(url: &str) -> Option<&str> {
    let rest = url.split("://").nth(1).unwrap_or(url);
    let host_port = rest.split('/').next()?;
    let host = match host_port.rsplit_once(':') {
        Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) => h,
        _ => host_port,
    };
    (!host.is_empty()).then_some(host)
}

/// Port of a URL, or the scheme's default.
pub fn url_port(url: &str) -> Option<u16> {
    let rest = url.split("://").nth(1).unwrap_or(url);
    let host_port = rest.split('/').next()?;
    match host_port.rsplit_once(':') {
        Some((_, p)) => p.parse().ok(),
        None if url.starts_with("https://") => Some(443),
        None if url.starts_with("http://") => Some(80),
        None => None,
    }
}

/// "6.5 MB", "700 GB", "1.2 TB" (powers of 1024, one decimal under 10).
pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} B", bytes)
    } else if value < 10.0 {
        format!("{:.1} {}", value, UNITS[unit])
    } else {
        format!("{:.0} {}", value, UNITS[unit])
    }
}

/// "12 s", "4 min", "3 h 05 min", "3 d 02 h".
pub fn human_duration(secs: i64) -> String {
    let secs = secs.max(0);
    if secs < 60 {
        format!("{} s", secs)
    } else if secs < 3600 {
        format!("{} min", secs / 60)
    } else if secs < 86_400 {
        format!("{} h {:02} min", secs / 3600, (secs % 3600) / 60)
    } else {
        format!("{} d {:02} h", secs / 86_400, (secs % 86_400) / 3600)
    }
}

/// Parse "700GB", "700 GB", "1TB", "500" (GB) into whole GB.
pub fn parse_offer_gb(text: &str) -> Result<u64, String> {
    let t = text.trim().to_ascii_lowercase().replace(' ', "");
    let (number, factor) = if let Some(n) = t.strip_suffix("tb") {
        (n, 1024)
    } else if let Some(n) = t.strip_suffix("gb") {
        (n, 1)
    } else {
        (t.as_str(), 1)
    };
    let gb: u64 = number
        .parse()
        .map_err(|_| format!("'{}' is not a size (try 700GB or 1TB)", text))?;
    let gb = gb * factor;
    if gb == 0 {
        return Err("offer at least 1 GB".into());
    }
    Ok(gb)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_host_maps_to_the_three_service_ports() {
        let u = urls_for_server("192.168.1.116").unwrap();
        assert_eq!(u.gateway_http, "http://192.168.1.116:8180");
        assert_eq!(u.gateway_grpc, "http://192.168.1.116:50060");
        assert_eq!(u.api, "http://192.168.1.116:3002");
        assert_eq!(
            urls_for_server("http://localhost/").unwrap().api,
            "http://localhost:3002"
        );
    }

    #[test]
    fn an_https_address_is_a_front_door_for_everything() {
        let u = urls_for_server("https://cloud.cyxwiz.ai").unwrap();
        assert_eq!(u.gateway_http, "https://cloud.cyxwiz.ai");
        assert_eq!(u.gateway_grpc, "https://cloud.cyxwiz.ai");
        assert_eq!(u.api, "https://cloud.cyxwiz.ai");
    }

    #[test]
    fn an_http_address_with_a_port_is_a_front_door_without_tls() {
        let u = urls_for_server("http://192.168.1.216:8000").unwrap();
        assert_eq!(u.gateway_http, "http://192.168.1.216:8000");
        assert_eq!(u.gateway_grpc, "http://192.168.1.216:8000");
        assert_eq!(u.api, "http://192.168.1.216:8000");
    }

    #[test]
    fn unclear_addresses_are_refused_with_the_reason() {
        assert!(urls_for_server("").is_err());
        assert!(urls_for_server("host:9999")
            .unwrap_err()
            .contains("http://HOST:PORT"));
        assert!(urls_for_server("https://x/y").is_err());
        assert!(urls_for_server("http://x:1/y").is_err());
    }

    #[test]
    fn hosts_and_ports_come_out_of_urls() {
        assert_eq!(
            url_host("http://192.168.1.116:50060"),
            Some("192.168.1.116")
        );
        assert_eq!(url_host("https://cloud.cyxwiz.ai"), Some("cloud.cyxwiz.ai"));
        assert_eq!(url_port("http://localhost:3002/api"), Some(3002));
        assert_eq!(url_port("https://cloud.cyxwiz.ai"), Some(443));
    }

    #[test]
    fn sizes_and_durations_read_like_the_mockup() {
        assert_eq!(human_bytes(6_815_744), "6.5 MB");
        assert_eq!(human_bytes(700 << 30), "700 GB");
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_duration(12), "12 s");
        assert_eq!(human_duration(3 * 86_400 + 2 * 3600 + 7), "3 d 02 h");
        assert_eq!(human_duration(3 * 3600 + 5 * 60), "3 h 05 min");
    }

    #[test]
    fn offers_are_read_in_gb() {
        assert_eq!(parse_offer_gb("700GB").unwrap(), 700);
        assert_eq!(parse_offer_gb("700 gb").unwrap(), 700);
        assert_eq!(parse_offer_gb("1TB").unwrap(), 1024);
        assert_eq!(parse_offer_gb("50").unwrap(), 50);
        assert!(parse_offer_gb("lots").is_err());
        assert!(parse_offer_gb("0").is_err());
    }
}
