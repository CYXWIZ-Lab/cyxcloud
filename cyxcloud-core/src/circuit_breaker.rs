//! Circuit Breaker for external service calls (DB, Redis, gRPC)
//!
//! States:
//! - Closed: Normal operation, requests pass through
//! - Open: Failing, requests immediately rejected
//! - HalfOpen: Testing recovery, limited requests allowed
//!
//! Transitions:
//! - Closed → Open: When failure_count >= failure_threshold
//! - Open → HalfOpen: After recovery_timeout elapses
//! - HalfOpen → Closed: On success
//! - HalfOpen → Open: On failure

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;
use std::time::{Duration, Instant};

/// Circuit breaker state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    Closed,
    Open,
    HalfOpen,
}

/// Circuit breaker configuration
#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    /// Number of consecutive failures before opening the circuit
    pub failure_threshold: u64,
    /// Duration to wait before transitioning from Open to HalfOpen
    pub recovery_timeout: Duration,
    /// Name for logging/metrics
    pub name: String,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 5,
            recovery_timeout: Duration::from_secs(30),
            name: "default".to_string(),
        }
    }
}

/// Circuit breaker implementation
pub struct CircuitBreaker {
    config: CircuitBreakerConfig,
    state: RwLock<CircuitState>,
    failure_count: AtomicU64,
    success_count: AtomicU64,
    last_failure_time: RwLock<Option<Instant>>,
}

impl CircuitBreaker {
    pub fn new(config: CircuitBreakerConfig) -> Self {
        Self {
            config,
            state: RwLock::new(CircuitState::Closed),
            failure_count: AtomicU64::new(0),
            success_count: AtomicU64::new(0),
            last_failure_time: RwLock::new(None),
        }
    }

    /// Check if a request is allowed through the circuit breaker
    pub fn allow_request(&self) -> bool {
        let state = *self.state.read().unwrap();
        match state {
            CircuitState::Closed => true,
            CircuitState::Open => {
                // Check if recovery timeout has elapsed
                if let Some(last_failure) = *self.last_failure_time.read().unwrap() {
                    if last_failure.elapsed() >= self.config.recovery_timeout {
                        // Transition to HalfOpen
                        if let Ok(mut s) = self.state.write() {
                            if *s == CircuitState::Open {
                                *s = CircuitState::HalfOpen;
                                return true;
                            }
                        }
                    }
                }
                false
            }
            CircuitState::HalfOpen => true,
        }
    }

    /// Record a successful operation
    pub fn record_success(&self) {
        self.success_count.fetch_add(1, Ordering::Relaxed);
        let state = *self.state.read().unwrap();
        if state == CircuitState::HalfOpen {
            // Recovery confirmed, close the circuit
            if let Ok(mut s) = self.state.write() {
                *s = CircuitState::Closed;
            }
            self.failure_count.store(0, Ordering::Relaxed);
        } else if state == CircuitState::Closed {
            // Reset failure count on success
            self.failure_count.store(0, Ordering::Relaxed);
        }
    }

    /// Record a failed operation
    pub fn record_failure(&self) {
        let count = self.failure_count.fetch_add(1, Ordering::Relaxed) + 1;
        if let Ok(mut t) = self.last_failure_time.write() {
            *t = Some(Instant::now());
        }

        let state = *self.state.read().unwrap();
        match state {
            CircuitState::Closed if count >= self.config.failure_threshold => {
                if let Ok(mut s) = self.state.write() {
                    *s = CircuitState::Open;
                }
            }
            CircuitState::HalfOpen => {
                // Failed during recovery, reopen
                if let Ok(mut s) = self.state.write() {
                    *s = CircuitState::Open;
                }
            }
            _ => {}
        }
    }

    /// Get current state
    pub fn state(&self) -> CircuitState {
        *self.state.read().unwrap()
    }

    /// Get failure count
    pub fn failure_count(&self) -> u64 {
        self.failure_count.load(Ordering::Relaxed)
    }

    /// Get name
    pub fn name(&self) -> &str {
        &self.config.name
    }

    /// Execute an async operation through the circuit breaker
    pub async fn call<F, T, E>(&self, f: F) -> Result<T, CircuitBreakerError<E>>
    where
        F: std::future::Future<Output = Result<T, E>>,
    {
        if !self.allow_request() {
            return Err(CircuitBreakerError::Open(self.config.name.clone()));
        }

        match f.await {
            Ok(val) => {
                self.record_success();
                Ok(val)
            }
            Err(e) => {
                self.record_failure();
                Err(CircuitBreakerError::Inner(e))
            }
        }
    }
}

/// Error type wrapping the inner error or circuit-open rejection
#[derive(Debug)]
pub enum CircuitBreakerError<E> {
    /// Circuit is open, request rejected without executing
    Open(String),
    /// The inner operation failed
    Inner(E),
}

impl<E: std::fmt::Display> std::fmt::Display for CircuitBreakerError<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Open(name) => write!(f, "Circuit breaker '{}' is open", name),
            Self::Inner(e) => write!(f, "{}", e),
        }
    }
}

impl<E: std::fmt::Display + std::fmt::Debug> std::error::Error for CircuitBreakerError<E> {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_closed_allows_requests() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 3,
            ..Default::default()
        });

        assert!(cb.allow_request());
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn test_opens_after_threshold() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 3,
            recovery_timeout: Duration::from_secs(60),
            name: "test".into(),
        });

        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Closed);

        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
        assert!(!cb.allow_request());
    }

    #[test]
    fn test_success_resets_failure_count() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 3,
            ..Default::default()
        });

        cb.record_failure();
        cb.record_failure();
        cb.record_success(); // Reset
        cb.record_failure();
        cb.record_failure();
        // Should still be closed (only 2 consecutive failures)
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn test_half_open_recovery() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 2,
            recovery_timeout: Duration::from_millis(10),
            name: "test".into(),
        });

        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);

        // Wait for recovery timeout
        std::thread::sleep(Duration::from_millis(20));

        // Should transition to HalfOpen
        assert!(cb.allow_request());
        assert_eq!(cb.state(), CircuitState::HalfOpen);

        // Success closes the circuit
        cb.record_success();
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn test_half_open_failure_reopens() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 2,
            recovery_timeout: Duration::from_millis(10),
            name: "test".into(),
        });

        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);

        std::thread::sleep(Duration::from_millis(20));
        assert!(cb.allow_request()); // → HalfOpen

        cb.record_failure(); // → Back to Open
        assert_eq!(cb.state(), CircuitState::Open);
    }

    #[tokio::test]
    async fn test_call_success() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig::default());

        let result: Result<i32, CircuitBreakerError<String>> =
            cb.call(async { Ok::<_, String>(42) }).await;
        assert_eq!(result.unwrap(), 42);
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[tokio::test]
    async fn test_call_failure_opens() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 2,
            recovery_timeout: Duration::from_secs(60),
            name: "test".into(),
        });

        let _: Result<i32, _> = cb.call(async { Err::<i32, _>("fail".to_string()) }).await;
        let _: Result<i32, _> = cb.call(async { Err::<i32, _>("fail".to_string()) }).await;

        assert_eq!(cb.state(), CircuitState::Open);

        // Next call should be rejected
        let result: Result<i32, CircuitBreakerError<String>> =
            cb.call(async { Ok::<_, String>(1) }).await;
        assert!(matches!(result, Err(CircuitBreakerError::Open(_))));
    }
}
