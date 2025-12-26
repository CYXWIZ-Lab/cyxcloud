//! Health checking for CyxCloud nodes
//!
//! Multi-layer health checks:
//! - Network ping
//! - gRPC health endpoint
//! - Storage backend status

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tokio::time::interval;
use tracing::{debug, info, warn};

/// Health status of a node
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthStatus {
    /// Node is healthy and responding
    Healthy,
    /// Node is degraded (some checks failing)
    Degraded,
    /// Node is unhealthy (not responding)
    Unhealthy,
    /// Node status is unknown (not yet checked)
    Unknown,
}

impl std::fmt::Display for HealthStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Healthy => write!(f, "healthy"),
            Self::Degraded => write!(f, "degraded"),
            Self::Unhealthy => write!(f, "unhealthy"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

/// Result of a health check
#[derive(Debug, Clone)]
pub struct HealthCheckResult {
    /// Overall status
    pub status: HealthStatus,
    /// Network ping result
    pub network_ok: bool,
    /// Network latency in milliseconds
    pub latency_ms: Option<u64>,
    /// gRPC health check result
    pub grpc_ok: bool,
    /// Storage backend check result
    pub storage_ok: bool,
    /// Time of this check
    pub checked_at: Instant,
    /// Number of consecutive failures
    pub consecutive_failures: u32,
    /// Error message if any
    pub error: Option<String>,
}

impl Default for HealthCheckResult {
    fn default() -> Self {
        Self {
            status: HealthStatus::Unknown,
            network_ok: false,
            latency_ms: None,
            grpc_ok: false,
            storage_ok: false,
            checked_at: Instant::now(),
            consecutive_failures: 0,
            error: None,
        }
    }
}

impl HealthCheckResult {
    /// Check if the node is considered healthy
    pub fn is_healthy(&self) -> bool {
        self.status == HealthStatus::Healthy
    }

    /// Check if the result is stale
    pub fn is_stale(&self, max_age: Duration) -> bool {
        self.checked_at.elapsed() > max_age
    }

    /// Determine status from check results
    fn determine_status(&mut self) {
        if self.network_ok && self.grpc_ok && self.storage_ok {
            self.status = HealthStatus::Healthy;
        } else if self.network_ok && (self.grpc_ok || self.storage_ok) {
            self.status = HealthStatus::Degraded;
        } else {
            self.status = HealthStatus::Unhealthy;
        }
    }
}

/// Health check configuration
#[derive(Debug, Clone)]
pub struct HealthConfig {
    /// Interval between health checks
    pub check_interval: Duration,
    /// Timeout for individual checks
    pub check_timeout: Duration,
    /// Number of failures before marking unhealthy
    pub failure_threshold: u32,
    /// Number of successes before marking healthy again
    pub recovery_threshold: u32,
    /// Max age before a result is considered stale
    pub stale_threshold: Duration,
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            check_interval: Duration::from_secs(30),
            check_timeout: Duration::from_secs(5),
            failure_threshold: 3,
            recovery_threshold: 2,
            stale_threshold: Duration::from_secs(120),
        }
    }
}

/// Trait for health check implementations
#[async_trait::async_trait]
pub trait HealthChecker: Send + Sync {
    /// Check network connectivity
    async fn check_network(&self, address: &str) -> (bool, Option<u64>);

    /// Check gRPC health endpoint
    async fn check_grpc(&self, address: &str) -> bool;

    /// Check storage backend
    async fn check_storage(&self, address: &str) -> bool;
}

/// Default health checker implementation
pub struct DefaultHealthChecker {
    timeout: Duration,
}

impl DefaultHealthChecker {
    pub fn new(timeout: Duration) -> Self {
        Self { timeout }
    }
}

#[async_trait::async_trait]
impl HealthChecker for DefaultHealthChecker {
    async fn check_network(&self, address: &str) -> (bool, Option<u64>) {
        // Parse address to get host and port
        let parts: Vec<&str> = address.split(':').collect();
        if parts.len() != 2 {
            return (false, None);
        }

        let start = Instant::now();

        // Try TCP connection
        match tokio::time::timeout(
            self.timeout,
            tokio::net::TcpStream::connect(address),
        )
        .await
        {
            Ok(Ok(_)) => {
                let latency = start.elapsed().as_millis() as u64;
                (true, Some(latency))
            }
            _ => (false, None),
        }
    }

    async fn check_grpc(&self, _address: &str) -> bool {
        // TODO: Implement actual gRPC health check
        // For now, assume if network is up, gRPC is up
        true
    }

    async fn check_storage(&self, _address: &str) -> bool {
        // TODO: Implement actual storage check
        // For now, assume if network is up, storage is up
        true
    }
}

/// Health monitor that tracks node health over time
pub struct HealthMonitor {
    /// Health check results by node address
    results: Arc<RwLock<HashMap<String, HealthCheckResult>>>,
    /// Health checker implementation
    checker: Arc<dyn HealthChecker>,
    /// Configuration
    config: HealthConfig,
}

impl HealthMonitor {
    /// Create a new health monitor
    pub fn new(checker: Arc<dyn HealthChecker>, config: HealthConfig) -> Self {
        Self {
            results: Arc::new(RwLock::new(HashMap::new())),
            checker,
            config,
        }
    }

    /// Create with default checker
    pub fn with_defaults() -> Self {
        let config = HealthConfig::default();
        let checker = Arc::new(DefaultHealthChecker::new(config.check_timeout));
        Self::new(checker, config)
    }

    /// Check health of a single node
    pub async fn check_node(&self, address: &str) -> HealthCheckResult {
        let (network_ok, latency_ms) = self.checker.check_network(address).await;
        let grpc_ok = if network_ok {
            self.checker.check_grpc(address).await
        } else {
            false
        };
        let storage_ok = if grpc_ok {
            self.checker.check_storage(address).await
        } else {
            false
        };

        // Get previous result for failure counting
        let previous = self.results.read().await.get(address).cloned();

        let mut result = HealthCheckResult {
            network_ok,
            latency_ms,
            grpc_ok,
            storage_ok,
            checked_at: Instant::now(),
            consecutive_failures: 0,
            error: None,
            status: HealthStatus::Unknown,
        };

        // Update failure count
        if !network_ok {
            result.consecutive_failures = previous
                .as_ref()
                .map(|p| p.consecutive_failures + 1)
                .unwrap_or(1);
            result.error = Some("Network check failed".to_string());
        } else if !grpc_ok {
            result.consecutive_failures = previous
                .as_ref()
                .map(|p| p.consecutive_failures + 1)
                .unwrap_or(1);
            result.error = Some("gRPC check failed".to_string());
        }

        result.determine_status();

        // Update stored result
        self.results
            .write()
            .await
            .insert(address.to_string(), result.clone());

        debug!(
            address = %address,
            status = %result.status,
            latency_ms = ?result.latency_ms,
            "Health check completed"
        );

        result
    }

    /// Check health of multiple nodes concurrently
    pub async fn check_nodes(&self, addresses: &[String]) -> HashMap<String, HealthCheckResult> {
        let futures: Vec<_> = addresses
            .iter()
            .map(|addr| {
                let addr = addr.clone();
                async move { (addr.clone(), self.check_node(&addr).await) }
            })
            .collect();

        futures::future::join_all(futures).await.into_iter().collect()
    }

    /// Get current health status for a node
    pub async fn get_status(&self, address: &str) -> Option<HealthCheckResult> {
        self.results.read().await.get(address).cloned()
    }

    /// Get all healthy nodes
    pub async fn get_healthy_nodes(&self) -> Vec<String> {
        self.results
            .read()
            .await
            .iter()
            .filter(|(_, result)| result.is_healthy())
            .map(|(addr, _)| addr.clone())
            .collect()
    }

    /// Get all unhealthy nodes
    pub async fn get_unhealthy_nodes(&self) -> Vec<String> {
        self.results
            .read()
            .await
            .iter()
            .filter(|(_, result)| result.status == HealthStatus::Unhealthy)
            .map(|(addr, _)| addr.clone())
            .collect()
    }

    /// Check if a node should be marked as failed
    pub async fn should_mark_failed(&self, address: &str) -> bool {
        if let Some(result) = self.results.read().await.get(address) {
            result.consecutive_failures >= self.config.failure_threshold
        } else {
            false
        }
    }

    /// Remove a node from monitoring
    pub async fn remove_node(&self, address: &str) {
        self.results.write().await.remove(address);
    }

    /// Get summary statistics
    pub async fn get_summary(&self) -> HealthSummary {
        let results = self.results.read().await;
        let mut summary = HealthSummary::default();

        for (_, result) in results.iter() {
            summary.total += 1;
            match result.status {
                HealthStatus::Healthy => summary.healthy += 1,
                HealthStatus::Degraded => summary.degraded += 1,
                HealthStatus::Unhealthy => summary.unhealthy += 1,
                HealthStatus::Unknown => summary.unknown += 1,
            }

            if let Some(latency) = result.latency_ms {
                summary.total_latency_ms += latency;
                summary.latency_samples += 1;
            }
        }

        if summary.latency_samples > 0 {
            summary.avg_latency_ms = Some(summary.total_latency_ms / summary.latency_samples);
        }

        summary
    }

    /// Start background health checking loop
    pub fn start_background_checks(
        self: Arc<Self>,
        nodes: Arc<RwLock<Vec<String>>>,
    ) -> tokio::task::JoinHandle<()> {
        let monitor = self;
        let interval_duration = monitor.config.check_interval;

        tokio::spawn(async move {
            let mut check_interval = interval(interval_duration);

            loop {
                check_interval.tick().await;

                let addresses = nodes.read().await.clone();
                if addresses.is_empty() {
                    continue;
                }

                let results = monitor.check_nodes(&addresses).await;

                // Log summary
                let healthy = results.values().filter(|r| r.is_healthy()).count();
                let unhealthy = results
                    .values()
                    .filter(|r| r.status == HealthStatus::Unhealthy)
                    .count();

                info!(
                    total = addresses.len(),
                    healthy = healthy,
                    unhealthy = unhealthy,
                    "Health check cycle completed"
                );

                // Warn about unhealthy nodes
                for (addr, result) in &results {
                    if result.consecutive_failures >= 3 {
                        warn!(
                            address = %addr,
                            failures = result.consecutive_failures,
                            "Node has multiple consecutive failures"
                        );
                    }
                }
            }
        })
    }
}

/// Health summary statistics
#[derive(Debug, Clone, Default)]
pub struct HealthSummary {
    pub total: usize,
    pub healthy: usize,
    pub degraded: usize,
    pub unhealthy: usize,
    pub unknown: usize,
    pub avg_latency_ms: Option<u64>,
    total_latency_ms: u64,
    latency_samples: u64,
}

impl HealthSummary {
    /// Get health percentage
    pub fn health_percentage(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            (self.healthy as f64 / self.total as f64) * 100.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_health_status_display() {
        assert_eq!(HealthStatus::Healthy.to_string(), "healthy");
        assert_eq!(HealthStatus::Unhealthy.to_string(), "unhealthy");
    }

    #[test]
    fn test_health_check_result_default() {
        let result = HealthCheckResult::default();
        assert_eq!(result.status, HealthStatus::Unknown);
        assert!(!result.is_healthy());
    }

    #[test]
    fn test_health_check_result_status() {
        let mut result = HealthCheckResult {
            network_ok: true,
            grpc_ok: true,
            storage_ok: true,
            ..Default::default()
        };
        result.determine_status();
        assert_eq!(result.status, HealthStatus::Healthy);

        let mut degraded = HealthCheckResult {
            network_ok: true,
            grpc_ok: true,
            storage_ok: false,
            ..Default::default()
        };
        degraded.determine_status();
        assert_eq!(degraded.status, HealthStatus::Degraded);

        let mut unhealthy = HealthCheckResult {
            network_ok: false,
            grpc_ok: false,
            storage_ok: false,
            ..Default::default()
        };
        unhealthy.determine_status();
        assert_eq!(unhealthy.status, HealthStatus::Unhealthy);
    }

    #[test]
    fn test_health_config_default() {
        let config = HealthConfig::default();
        assert_eq!(config.check_interval, Duration::from_secs(30));
        assert_eq!(config.failure_threshold, 3);
    }

    #[test]
    fn test_health_summary() {
        let summary = HealthSummary {
            total: 10,
            healthy: 8,
            degraded: 1,
            unhealthy: 1,
            unknown: 0,
            avg_latency_ms: Some(50),
            total_latency_ms: 500,
            latency_samples: 10,
        };
        assert!((summary.health_percentage() - 80.0).abs() < 0.01);
    }

    #[tokio::test]
    async fn test_health_monitor_creation() {
        let monitor = HealthMonitor::with_defaults();
        let summary = monitor.get_summary().await;
        assert_eq!(summary.total, 0);
    }
}
