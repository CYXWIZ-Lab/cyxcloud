//! Prometheus Metrics for CyxCloud Gateway
//!
//! Exposes metrics at GET /metrics in Prometheus text format.
//! Uses the `metrics` crate with prometheus exporter.

use axum::{routing::get, Router};
use metrics::{counter, gauge, histogram};
use std::sync::Arc;

/// Initialize the Prometheus metrics exporter and install it as the global recorder.
/// Returns the handle for rendering metrics on the /metrics endpoint.
pub fn init_metrics() -> metrics_exporter_prometheus::PrometheusHandle {
    let builder = metrics_exporter_prometheus::PrometheusBuilder::new();
    builder
        .install_recorder()
        .expect("Failed to install Prometheus metrics recorder")
}

/// Create metrics route that can be merged into any Router
pub fn routes<S: Clone + Send + Sync + 'static>(
    handle: metrics_exporter_prometheus::PrometheusHandle,
) -> Router<S> {
    let handle = Arc::new(handle);
    Router::new().route(
        "/metrics",
        get(move || {
            let h = handle.clone();
            async move { h.render() }
        }),
    )
}

// ============================================================================
// Metric Recording Helpers
// ============================================================================

/// Record an S3 API request
pub fn record_s3_request(method: &str, status: u16) {
    counter!("s3_requests_total", "method" => method.to_string(), "status" => status.to_string())
        .increment(1);
}

/// Record S3 request latency
pub fn record_s3_latency(method: &str, duration_secs: f64) {
    histogram!("s3_request_duration_seconds", "method" => method.to_string()).record(duration_secs);
}

/// Record bytes uploaded
pub fn record_bytes_uploaded(bytes: u64) {
    counter!("s3_bytes_uploaded_total").increment(bytes);
}

/// Record bytes downloaded
pub fn record_bytes_downloaded(bytes: u64) {
    counter!("s3_bytes_downloaded_total").increment(bytes);
}

/// Record active gRPC connections
pub fn set_grpc_connections(count: u64) {
    gauge!("grpc_active_connections").set(count as f64);
}

/// Record registered node count
pub fn set_registered_nodes(count: u64) {
    gauge!("registered_nodes").set(count as f64);
}

/// Record auth events
pub fn record_auth_event(event_type: &str) {
    counter!("auth_events_total", "type" => event_type.to_string()).increment(1);
}

/// Record token revocation
pub fn record_token_revocation() {
    counter!("token_revocations_total").increment(1);
}

/// Record circuit breaker state change
pub fn record_circuit_breaker_state(name: &str, state: &str) {
    gauge!("circuit_breaker_state", "name" => name.to_string(), "state" => state.to_string())
        .set(1.0);
}
