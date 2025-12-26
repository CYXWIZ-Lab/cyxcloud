//! Prometheus metrics for CyxCloud storage node
//!
//! Exposes node health, performance, and storage metrics.

use metrics::{counter, gauge, histogram, describe_counter, describe_gauge, describe_histogram};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;

/// Metric names as constants
pub mod names {
    // Storage metrics
    pub const STORAGE_BYTES_USED: &str = "cyxcloud_storage_bytes_used";
    pub const STORAGE_BYTES_AVAILABLE: &str = "cyxcloud_storage_bytes_available";
    pub const STORAGE_CHUNKS_TOTAL: &str = "cyxcloud_storage_chunks_total";

    // Request metrics
    pub const REQUESTS_TOTAL: &str = "cyxcloud_requests_total";
    pub const REQUESTS_DURATION: &str = "cyxcloud_request_duration_seconds";
    pub const REQUESTS_BYTES: &str = "cyxcloud_request_bytes";

    // Network metrics
    pub const CONNECTIONS_ACTIVE: &str = "cyxcloud_connections_active";
    pub const BANDWIDTH_IN: &str = "cyxcloud_bandwidth_in_bytes";
    pub const BANDWIDTH_OUT: &str = "cyxcloud_bandwidth_out_bytes";

    // Health metrics
    pub const NODE_UP: &str = "cyxcloud_node_up";
    pub const NODE_START_TIME: &str = "cyxcloud_node_start_time_seconds";
    pub const HEARTBEAT_SUCCESS: &str = "cyxcloud_heartbeat_success_total";
    pub const HEARTBEAT_FAILURE: &str = "cyxcloud_heartbeat_failure_total";
}

/// Initialize metric descriptions
pub fn init_metrics() {
    // Storage metrics
    describe_gauge!(
        names::STORAGE_BYTES_USED,
        "Total bytes currently stored on this node"
    );
    describe_gauge!(
        names::STORAGE_BYTES_AVAILABLE,
        "Available storage capacity in bytes"
    );
    describe_gauge!(
        names::STORAGE_CHUNKS_TOTAL,
        "Total number of chunks stored"
    );

    // Request metrics
    describe_counter!(
        names::REQUESTS_TOTAL,
        "Total number of requests processed"
    );
    describe_histogram!(
        names::REQUESTS_DURATION,
        "Request processing duration in seconds"
    );
    describe_counter!(
        names::REQUESTS_BYTES,
        "Total bytes transferred in requests"
    );

    // Network metrics
    describe_gauge!(
        names::CONNECTIONS_ACTIVE,
        "Number of active gRPC connections"
    );
    describe_counter!(
        names::BANDWIDTH_IN,
        "Total incoming bandwidth in bytes"
    );
    describe_counter!(
        names::BANDWIDTH_OUT,
        "Total outgoing bandwidth in bytes"
    );

    // Health metrics
    describe_gauge!(names::NODE_UP, "Whether the node is up (1) or down (0)");
    describe_gauge!(
        names::NODE_START_TIME,
        "Unix timestamp when the node started"
    );
    describe_counter!(
        names::HEARTBEAT_SUCCESS,
        "Number of successful heartbeats"
    );
    describe_counter!(
        names::HEARTBEAT_FAILURE,
        "Number of failed heartbeats"
    );
}

/// Metrics recorder for tracking node statistics
#[derive(Clone)]
pub struct NodeMetrics {
    node_id: String,
    start_time: std::time::Instant,
    // Atomic counters for bandwidth tracking (can be read for heartbeat)
    bytes_uploaded: Arc<std::sync::atomic::AtomicU64>,
    bytes_downloaded: Arc<std::sync::atomic::AtomicU64>,
}

impl NodeMetrics {
    /// Create a new metrics recorder
    pub fn new(node_id: impl Into<String>) -> Self {
        let metrics = Self {
            node_id: node_id.into(),
            start_time: std::time::Instant::now(),
            bytes_uploaded: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            bytes_downloaded: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };

        // Set initial metrics
        gauge!(names::NODE_UP, "node_id" => metrics.node_id.clone()).set(1.0);
        gauge!(names::NODE_START_TIME, "node_id" => metrics.node_id.clone())
            .set(std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs_f64());

        metrics
    }

    /// Get total bytes uploaded (for heartbeat reporting)
    pub fn get_bytes_uploaded(&self) -> u64 {
        self.bytes_uploaded.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Get total bytes downloaded (for heartbeat reporting)
    pub fn get_bytes_downloaded(&self) -> u64 {
        self.bytes_downloaded.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Record a chunk store operation
    pub fn record_store(&self, size: usize, duration: std::time::Duration) {
        let labels = [("node_id", self.node_id.clone()), ("operation", "store".to_string())];
        counter!(names::REQUESTS_TOTAL, &labels).increment(1);
        counter!(names::REQUESTS_BYTES, &labels).increment(size as u64);
        histogram!(names::REQUESTS_DURATION, &labels).record(duration.as_secs_f64());
        counter!(names::BANDWIDTH_IN, "node_id" => self.node_id.clone()).increment(size as u64);
        // Update atomic counter for heartbeat reporting
        self.bytes_uploaded.fetch_add(size as u64, std::sync::atomic::Ordering::Relaxed);
    }

    /// Record a chunk get operation
    pub fn record_get(&self, size: usize, duration: std::time::Duration) {
        let labels = [("node_id", self.node_id.clone()), ("operation", "get".to_string())];
        counter!(names::REQUESTS_TOTAL, &labels).increment(1);
        counter!(names::REQUESTS_BYTES, &labels).increment(size as u64);
        histogram!(names::REQUESTS_DURATION, &labels).record(duration.as_secs_f64());
        counter!(names::BANDWIDTH_OUT, "node_id" => self.node_id.clone()).increment(size as u64);
        // Update atomic counter for heartbeat reporting
        self.bytes_downloaded.fetch_add(size as u64, std::sync::atomic::Ordering::Relaxed);
    }

    /// Record a chunk delete operation
    pub fn record_delete(&self, duration: std::time::Duration) {
        let labels = [("node_id", self.node_id.clone()), ("operation", "delete".to_string())];
        counter!(names::REQUESTS_TOTAL, &labels).increment(1);
        histogram!(names::REQUESTS_DURATION, &labels).record(duration.as_secs_f64());
    }

    /// Update storage statistics
    pub fn update_storage(&self, used_bytes: u64, available_bytes: u64, chunk_count: u64) {
        gauge!(names::STORAGE_BYTES_USED, "node_id" => self.node_id.clone()).set(used_bytes as f64);
        gauge!(names::STORAGE_BYTES_AVAILABLE, "node_id" => self.node_id.clone()).set(available_bytes as f64);
        gauge!(names::STORAGE_CHUNKS_TOTAL, "node_id" => self.node_id.clone()).set(chunk_count as f64);
    }

    /// Update connection count
    pub fn update_connections(&self, count: usize) {
        gauge!(names::CONNECTIONS_ACTIVE, "node_id" => self.node_id.clone()).set(count as f64);
    }

    /// Record heartbeat result
    pub fn record_heartbeat(&self, success: bool) {
        if success {
            counter!(names::HEARTBEAT_SUCCESS, "node_id" => self.node_id.clone()).increment(1);
        } else {
            counter!(names::HEARTBEAT_FAILURE, "node_id" => self.node_id.clone()).increment(1);
        }
    }

    /// Mark node as down
    pub fn mark_down(&self) {
        gauge!(names::NODE_UP, "node_id" => self.node_id.clone()).set(0.0);
    }

    /// Get uptime in seconds
    pub fn uptime_secs(&self) -> u64 {
        self.start_time.elapsed().as_secs()
    }
}

/// HTTP server for metrics endpoint
pub struct MetricsServer {
    handle: PrometheusHandle,
    addr: SocketAddr,
}

impl MetricsServer {
    /// Create a new metrics server
    pub fn new(port: u16) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let addr: SocketAddr = format!("0.0.0.0:{}", port).parse()?;

        let builder = PrometheusBuilder::new();
        let handle = builder.install_recorder()?;

        Ok(Self { handle, addr })
    }

    /// Start the metrics HTTP server
    pub async fn start(
        self,
        health_path: String,
        metrics_path: String,
        health_state: Arc<RwLock<HealthState>>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        use axum::{routing::get, Router, response::IntoResponse, http::StatusCode};

        let handle = self.handle;

        // Health check handler
        let health_handler = {
            let state = health_state.clone();
            move || {
                let state = state.clone();
                async move {
                    let health = state.read().await;
                    if health.is_healthy {
                        (StatusCode::OK, "OK").into_response()
                    } else {
                        (StatusCode::SERVICE_UNAVAILABLE, "UNHEALTHY").into_response()
                    }
                }
            }
        };

        // Metrics handler
        let metrics_handler = move || {
            let handle = handle.clone();
            async move { handle.render() }
        };

        let app = Router::new()
            .route(&health_path, get(health_handler))
            .route(&metrics_path, get(metrics_handler));

        info!(addr = %self.addr, "Starting metrics server");

        let listener = tokio::net::TcpListener::bind(self.addr).await?;
        axum::serve(listener, app).await?;

        Ok(())
    }
}

/// Health state for the node
#[derive(Debug, Clone)]
pub struct HealthState {
    pub is_healthy: bool,
    pub storage_ok: bool,
    pub network_ok: bool,
    pub last_check: std::time::Instant,
}

impl Default for HealthState {
    fn default() -> Self {
        Self {
            is_healthy: true,
            storage_ok: true,
            network_ok: true,
            last_check: std::time::Instant::now(),
        }
    }
}

impl HealthState {
    /// Update health state
    pub fn update(&mut self, storage_ok: bool, network_ok: bool) {
        self.storage_ok = storage_ok;
        self.network_ok = network_ok;
        self.is_healthy = storage_ok && network_ok;
        self.last_check = std::time::Instant::now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_metrics_creation() {
        let metrics = NodeMetrics::new("test-node");
        assert!(metrics.uptime_secs() < 1);
    }

    #[test]
    fn test_health_state() {
        let mut state = HealthState::default();
        assert!(state.is_healthy);

        state.update(false, true);
        assert!(!state.is_healthy);

        state.update(true, true);
        assert!(state.is_healthy);
    }
}
