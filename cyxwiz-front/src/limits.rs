//! Per-client request limit (a token bucket per address) and the client
//! address itself.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::Instant;

/// Above this many tracked addresses, full buckets are forgotten.
const PRUNE_ABOVE: usize = 10_000;

pub struct RateLimiter {
    per_minute: f64,
    buckets: Mutex<HashMap<IpAddr, (f64, Instant)>>,
}

impl RateLimiter {
    pub fn new(per_minute: u32) -> Self {
        Self {
            per_minute: per_minute as f64,
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// Whether `ip` may make one more request at `now`. A client starts with
    /// a full minute's worth and refills continuously.
    pub fn allow(&self, ip: IpAddr, now: Instant) -> bool {
        let mut buckets = self.buckets.lock().unwrap_or_else(|e| e.into_inner());
        let refill_per_sec = self.per_minute / 60.0;
        if buckets.len() > PRUNE_ABOVE {
            let full = self.per_minute;
            buckets.retain(|_, (tokens, at)| {
                *tokens + now.duration_since(*at).as_secs_f64() * refill_per_sec < full
            });
        }
        let (tokens, at) = buckets.entry(ip).or_insert((self.per_minute, now));
        *tokens =
            (*tokens + now.duration_since(*at).as_secs_f64() * refill_per_sec).min(self.per_minute);
        *at = now;
        if *tokens >= 1.0 {
            *tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// The client's address: the connection's peer, or the address a trusted
/// proxy (e.g. Cloudflare's tunnel) put in `header`. Anyone else's header is
/// ignored, so a client cannot pick its own address to dodge the limit.
pub fn client_ip(
    headers: &hyper::HeaderMap,
    peer: IpAddr,
    trusted_proxies: &[IpAddr],
    header: &str,
) -> IpAddr {
    if !trusted_proxies.contains(&peer) {
        return peer;
    }
    headers
        .get(header)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(peer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn a_client_gets_its_minute_then_waits() {
        let limiter = RateLimiter::new(3);
        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        let t0 = Instant::now();
        assert!(limiter.allow(ip, t0));
        assert!(limiter.allow(ip, t0));
        assert!(limiter.allow(ip, t0));
        assert!(!limiter.allow(ip, t0), "fourth in the same instant");
        // 3 per minute refills one every 20 s
        assert!(limiter.allow(ip, t0 + Duration::from_secs(20)));
        assert!(!limiter.allow(ip, t0 + Duration::from_secs(21)));
        // another client has its own bucket
        assert!(limiter.allow("10.0.0.2".parse().unwrap(), t0));
    }

    #[test]
    fn only_a_trusted_proxy_names_the_client() {
        let mut h = hyper::HeaderMap::new();
        h.insert("cf-connecting-ip", "203.0.113.9".parse().unwrap());
        let tunnel: IpAddr = "127.0.0.1".parse().unwrap();
        let stranger: IpAddr = "198.51.100.4".parse().unwrap();
        assert_eq!(
            client_ip(&h, tunnel, &[tunnel], "cf-connecting-ip"),
            "203.0.113.9".parse::<IpAddr>().unwrap()
        );
        assert_eq!(
            client_ip(&h, stranger, &[tunnel], "cf-connecting-ip"),
            stranger
        );
        assert_eq!(
            client_ip(
                &hyper::HeaderMap::new(),
                tunnel,
                &[tunnel],
                "cf-connecting-ip"
            ),
            tunnel
        );
    }
}
