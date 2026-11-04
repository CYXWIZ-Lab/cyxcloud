//! Per-client request limits, counted in Redis (MetadataService) so they
//! hold across gateway restarts. The client is `client_ip::client_key`.

use crate::AppState;

/// A fixed window: at most `max_requests` per `window_secs` per client.
pub struct RateLimit {
    /// Key prefix for the rate limit
    pub key: &'static str,
    /// Maximum requests allowed in the window
    pub max_requests: u64,
    /// Window duration in seconds
    pub window_secs: u64,
}

/// Whether `client` may make one more `limit` request: Some(allowed), or
/// None when the count cannot be read (the caller decides whether that
/// fails open or closed). With no metadata service there is nothing to
/// count against and the request is allowed.
pub async fn allowed(state: &AppState, limit: &RateLimit, client: &str) -> Option<bool> {
    let Some(meta) = state.metadata_service() else {
        return Some(true);
    };
    let key = format!("{}:{}", limit.key, client);
    match meta
        .check_rate_limit(&key, limit.max_requests, limit.window_secs)
        .await
    {
        Ok(allowed) => Some(allowed),
        Err(e) => {
            tracing::debug!(error = %e, key = %limit.key, "Rate limit check failed");
            None
        }
    }
}
