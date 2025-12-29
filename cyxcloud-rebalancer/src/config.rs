//! Rebalancer configuration
//!
//! Configuration loaded from environment variables and command line.

use std::time::Duration;
use thiserror::Error;

/// Configuration errors
#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("Missing required environment variable: {0}")]
    MissingEnvVar(String),

    #[error("Invalid value for {0}: {1}")]
    InvalidValue(String, String),
}

/// Rebalancer configuration
#[derive(Debug, Clone)]
pub struct RebalancerConfig {
    /// PostgreSQL database URL
    pub database_url: String,

    /// Interval between scans in seconds
    pub scan_interval_secs: u64,

    /// Target replication factor for chunks
    pub target_replication: usize,

    /// Maximum concurrent repair operations
    pub max_concurrent: usize,

    /// Maximum bytes to transfer per hour (rate limit)
    pub rate_limit_bytes_per_hour: u64,

    /// Timeout for health checks in seconds
    pub health_check_timeout_secs: u64,

    /// Maximum tasks per repair plan
    pub max_tasks_per_plan: usize,

    /// Dry run mode (don't actually repair)
    pub dry_run: bool,

    /// Metrics port for health/metrics endpoint
    pub metrics_port: u16,
}

impl Default for RebalancerConfig {
    fn default() -> Self {
        Self {
            database_url: String::new(),
            scan_interval_secs: 60,
            target_replication: 3,
            max_concurrent: 4,
            rate_limit_bytes_per_hour: 10 * 1024 * 1024 * 1024, // 10 GB/hour
            health_check_timeout_secs: 5,
            max_tasks_per_plan: 100,
            dry_run: false,
            metrics_port: 9090,
        }
    }
}

impl RebalancerConfig {
    /// Create configuration from environment variables
    pub fn from_env() -> Result<Self, ConfigError> {
        let database_url = std::env::var("DATABASE_URL")
            .map_err(|_| ConfigError::MissingEnvVar("DATABASE_URL".to_string()))?;

        let scan_interval_secs = std::env::var("REBALANCER_SCAN_INTERVAL")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(60);

        let target_replication = std::env::var("REBALANCER_TARGET_REPLICATION")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3);

        let max_concurrent = std::env::var("REBALANCER_MAX_CONCURRENT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(4);

        let rate_limit_gb = std::env::var("REBALANCER_RATE_LIMIT_GB")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(10);

        let health_check_timeout_secs = std::env::var("REBALANCER_HEALTH_TIMEOUT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(5);

        let max_tasks_per_plan = std::env::var("REBALANCER_MAX_TASKS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(100);

        let dry_run = std::env::var("REBALANCER_DRY_RUN")
            .ok()
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);

        let metrics_port = std::env::var("REBALANCER_METRICS_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(9090);

        Ok(Self {
            database_url,
            scan_interval_secs,
            target_replication,
            max_concurrent,
            rate_limit_bytes_per_hour: rate_limit_gb * 1024 * 1024 * 1024,
            health_check_timeout_secs,
            max_tasks_per_plan,
            dry_run,
            metrics_port,
        })
    }

    /// Get scan interval as Duration
    pub fn scan_interval(&self) -> Duration {
        Duration::from_secs(self.scan_interval_secs)
    }

    /// Get health check timeout as Duration
    pub fn health_check_timeout(&self) -> Duration {
        Duration::from_secs(self.health_check_timeout_secs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = RebalancerConfig::default();
        assert_eq!(config.scan_interval_secs, 60);
        assert_eq!(config.target_replication, 3);
        assert_eq!(config.max_concurrent, 4);
        assert!(!config.dry_run);
    }

    #[test]
    fn test_scan_interval_duration() {
        let config = RebalancerConfig {
            scan_interval_secs: 120,
            ..Default::default()
        };
        assert_eq!(config.scan_interval(), Duration::from_secs(120));
    }
}
