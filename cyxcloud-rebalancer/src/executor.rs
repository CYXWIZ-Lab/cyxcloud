//! Repair Executor
//!
//! Executes repair plans with:
//! - Parallel execution across nodes
//! - Rate limiting per node
//! - Progress tracking
//! - Error handling and retries

use std::collections::HashMap;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::sync::{mpsc, RwLock, Semaphore};
use tokio::time::timeout;
use tracing::{error, info, instrument};

use crate::planner::{RepairPlan, RepairTask};

/// Executor errors
#[derive(Error, Debug, Clone)]
pub enum ExecutorError {
    #[error("Transfer failed: {0}")]
    TransferFailed(String),

    #[error("Timeout during transfer")]
    Timeout,

    #[error("Source node unavailable: {0}")]
    SourceUnavailable(String),

    #[error("Target node unavailable: {0}")]
    TargetUnavailable(String),

    #[error("Rate limit exceeded")]
    RateLimitExceeded,

    #[error("Executor shutdown")]
    Shutdown,
}

pub type Result<T> = std::result::Result<T, ExecutorError>;

/// Result of executing a single repair task
#[derive(Debug, Clone)]
pub struct TaskResult {
    pub task_id: String,
    pub success: bool,
    pub error: Option<ExecutorError>,
    pub bytes_transferred: u64,
    pub duration: Duration,
    pub targets_succeeded: Vec<String>,
    pub targets_failed: Vec<String>,
}

/// Overall execution result
#[derive(Debug, Default)]
pub struct ExecutionResult {
    /// Tasks that completed successfully
    pub succeeded: Vec<TaskResult>,
    /// Tasks that failed
    pub failed: Vec<TaskResult>,
    /// Total bytes transferred
    pub total_bytes: u64,
    /// Total execution time
    pub duration: Duration,
    /// Tasks that were skipped
    pub skipped: usize,
}

impl ExecutionResult {
    /// Success rate as percentage
    pub fn success_rate(&self) -> f64 {
        let total = self.succeeded.len() + self.failed.len();
        if total == 0 {
            100.0
        } else {
            (self.succeeded.len() as f64 / total as f64) * 100.0
        }
    }

    /// Summary string
    pub fn summary(&self) -> String {
        format!(
            "{} succeeded, {} failed, {} bytes in {:?} ({:.1}% success rate)",
            self.succeeded.len(),
            self.failed.len(),
            self.total_bytes,
            self.duration,
            self.success_rate()
        )
    }
}

/// Executor configuration
#[derive(Debug, Clone)]
pub struct ExecutorConfig {
    /// Maximum concurrent repairs
    pub max_concurrent: usize,
    /// Maximum concurrent repairs per source node
    pub max_per_source: usize,
    /// Maximum concurrent repairs per target node
    pub max_per_target: usize,
    /// Transfer timeout per task
    pub transfer_timeout: Duration,
    /// Number of retries for failed transfers
    pub max_retries: u32,
    /// Delay between retries
    pub retry_delay: Duration,
    /// Rate limit per node (bytes per second)
    pub node_rate_limit: u64,
    /// Enable progress reporting
    pub report_progress: bool,
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self {
            max_concurrent: 10,
            max_per_source: 3,
            max_per_target: 3,
            transfer_timeout: Duration::from_secs(300), // 5 minutes
            max_retries: 3,
            retry_delay: Duration::from_secs(5),
            node_rate_limit: 100 * 1024 * 1024, // 100 MB/s
            report_progress: true,
        }
    }
}

/// Progress update for a task
#[derive(Debug, Clone)]
pub struct ProgressUpdate {
    pub task_id: String,
    pub bytes_transferred: u64,
    pub total_bytes: u64,
    pub percent: f32,
    pub status: ProgressStatus,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProgressStatus {
    Pending,
    Running,
    Completed,
    Failed(String),
    Retrying(u32),
}

/// Repair executor
pub struct Executor {
    config: ExecutorConfig,
    /// Semaphore for global concurrency
    global_semaphore: Arc<Semaphore>,
    /// Per-node semaphores
    node_semaphores: Arc<RwLock<HashMap<String, Arc<Semaphore>>>>,
    /// Bytes transferred per node
    node_bytes: Arc<RwLock<HashMap<String, AtomicU64>>>,
    /// Progress channel
    progress_tx: Option<mpsc::Sender<ProgressUpdate>>,
    /// Shutdown flag
    shutdown: Arc<RwLock<bool>>,
}

impl Executor {
    /// Create a new executor
    pub fn new(config: ExecutorConfig) -> Self {
        let global_semaphore = Arc::new(Semaphore::new(config.max_concurrent));

        Self {
            config,
            global_semaphore,
            node_semaphores: Arc::new(RwLock::new(HashMap::new())),
            node_bytes: Arc::new(RwLock::new(HashMap::new())),
            progress_tx: None,
            shutdown: Arc::new(RwLock::new(false)),
        }
    }

    /// Create executor with progress channel
    pub fn with_progress(config: ExecutorConfig) -> (Self, mpsc::Receiver<ProgressUpdate>) {
        let (tx, rx) = mpsc::channel(100);
        let mut executor = Self::new(config);
        executor.progress_tx = Some(tx);
        (executor, rx)
    }

    /// Execute a repair plan
    #[instrument(skip(self, plan, transfer_fn))]
    pub async fn execute<F, Fut>(
        &self,
        plan: RepairPlan,
        transfer_fn: F,
    ) -> ExecutionResult
    where
        F: Fn(String, String, Vec<u8>, Vec<String>) -> Fut + Clone + Send + Sync + 'static,
        Fut: std::future::Future<Output = std::result::Result<Vec<String>, String>> + Send,
    {
        let start = Instant::now();
        let mut result = ExecutionResult::default();

        if plan.tasks.is_empty() {
            info!("No tasks to execute");
            return result;
        }

        info!(tasks = plan.tasks.len(), "Executing repair plan");

        // Execute tasks in parallel
        let mut handles = Vec::new();

        for task in plan.tasks {
            // Check shutdown
            if *self.shutdown.read().await {
                result.skipped += 1;
                continue;
            }

            let executor = self.clone_for_task();
            let transfer = transfer_fn.clone();

            let handle = tokio::spawn(async move {
                executor.execute_task(task, transfer).await
            });

            handles.push(handle);
        }

        // Collect results
        for handle in handles {
            match handle.await {
                Ok(task_result) => {
                    result.total_bytes += task_result.bytes_transferred;

                    if task_result.success {
                        result.succeeded.push(task_result);
                    } else {
                        result.failed.push(task_result);
                    }
                }
                Err(e) => {
                    error!(error = %e, "Task panicked");
                }
            }
        }

        result.duration = start.elapsed();

        info!(summary = %result.summary(), "Repair plan execution complete");

        result
    }

    /// Execute a single repair task
    async fn execute_task<F, Fut>(&self, task: RepairTask, transfer_fn: F) -> TaskResult
    where
        F: Fn(String, String, Vec<u8>, Vec<String>) -> Fut + Clone,
        Fut: std::future::Future<Output = std::result::Result<Vec<String>, String>> + Send,
    {
        let start = Instant::now();
        let task_id = task.task_id.clone();

        // Report progress: pending
        self.report_progress(ProgressUpdate {
            task_id: task_id.clone(),
            bytes_transferred: 0,
            total_bytes: task.chunk_size,
            percent: 0.0,
            status: ProgressStatus::Pending,
        })
        .await;

        // Acquire global semaphore
        let _global_permit = match self.global_semaphore.acquire().await {
            Ok(p) => p,
            Err(_) => {
                return TaskResult {
                    task_id,
                    success: false,
                    error: Some(ExecutorError::Shutdown),
                    bytes_transferred: 0,
                    duration: start.elapsed(),
                    targets_succeeded: Vec::new(),
                    targets_failed: task.target_nodes.clone(),
                };
            }
        };

        // Acquire node semaphores
        let source_sem = self.get_node_semaphore(&task.source_node).await;
        let _source_permit = match source_sem.acquire().await {
            Ok(p) => p,
            Err(_) => {
                return TaskResult {
                    task_id,
                    success: false,
                    error: Some(ExecutorError::SourceUnavailable(task.source_node.clone())),
                    bytes_transferred: 0,
                    duration: start.elapsed(),
                    targets_succeeded: Vec::new(),
                    targets_failed: task.target_nodes.clone(),
                };
            }
        };

        // Report progress: running
        self.report_progress(ProgressUpdate {
            task_id: task_id.clone(),
            bytes_transferred: 0,
            total_bytes: task.chunk_size,
            percent: 0.0,
            status: ProgressStatus::Running,
        })
        .await;

        // Execute with retries
        let mut last_error = None;
        let mut targets_succeeded = Vec::new();
        let mut targets_failed = task.target_nodes.clone();

        for attempt in 0..=self.config.max_retries {
            if attempt > 0 {
                // Report retry
                self.report_progress(ProgressUpdate {
                    task_id: task_id.clone(),
                    bytes_transferred: 0,
                    total_bytes: task.chunk_size,
                    percent: 0.0,
                    status: ProgressStatus::Retrying(attempt),
                })
                .await;

                tokio::time::sleep(self.config.retry_delay).await;
            }

            // Execute transfer
            match timeout(
                self.config.transfer_timeout,
                transfer_fn(
                    task.source_node.clone(),
                    task_id.clone(),
                    task.chunk_id.clone(),
                    targets_failed.clone(),
                ),
            )
            .await
            {
                Ok(Ok(succeeded)) => {
                    // Some or all targets succeeded
                    for s in &succeeded {
                        if !targets_succeeded.contains(s) {
                            targets_succeeded.push(s.clone());
                        }
                    }
                    targets_failed.retain(|t| !succeeded.contains(t));

                    if targets_failed.is_empty() {
                        // All targets succeeded
                        break;
                    }
                    last_error = Some(ExecutorError::TransferFailed(format!(
                        "Partial success: {} of {} targets",
                        targets_succeeded.len(),
                        task.target_nodes.len()
                    )));
                }
                Ok(Err(e)) => {
                    last_error = Some(ExecutorError::TransferFailed(e));
                }
                Err(_) => {
                    last_error = Some(ExecutorError::Timeout);
                }
            }
        }

        let success = targets_failed.is_empty();
        let bytes_transferred = if success { task.chunk_size } else { 0 };

        // Report completion
        self.report_progress(ProgressUpdate {
            task_id: task_id.clone(),
            bytes_transferred,
            total_bytes: task.chunk_size,
            percent: if success { 100.0 } else { 0.0 },
            status: if success {
                ProgressStatus::Completed
            } else {
                ProgressStatus::Failed(
                    last_error
                        .as_ref()
                        .map(|e| e.to_string())
                        .unwrap_or_default(),
                )
            },
        })
        .await;

        TaskResult {
            task_id,
            success,
            error: if success { None } else { last_error },
            bytes_transferred,
            duration: start.elapsed(),
            targets_succeeded,
            targets_failed,
        }
    }

    /// Get or create node semaphore
    async fn get_node_semaphore(&self, node_id: &str) -> Arc<Semaphore> {
        let semaphores = self.node_semaphores.read().await;
        if let Some(sem) = semaphores.get(node_id) {
            return sem.clone();
        }
        drop(semaphores);

        let mut semaphores = self.node_semaphores.write().await;
        semaphores
            .entry(node_id.to_string())
            .or_insert_with(|| Arc::new(Semaphore::new(self.config.max_per_source)))
            .clone()
    }

    /// Report progress update
    async fn report_progress(&self, update: ProgressUpdate) {
        if let Some(tx) = &self.progress_tx {
            let _ = tx.send(update).await;
        }
    }

    /// Clone executor state for spawning task
    fn clone_for_task(&self) -> Self {
        Self {
            config: self.config.clone(),
            global_semaphore: self.global_semaphore.clone(),
            node_semaphores: self.node_semaphores.clone(),
            node_bytes: self.node_bytes.clone(),
            progress_tx: self.progress_tx.clone(),
            shutdown: self.shutdown.clone(),
        }
    }

    /// Signal shutdown
    pub async fn shutdown(&self) {
        let mut shutdown = self.shutdown.write().await;
        *shutdown = true;
        info!("Executor shutdown signaled");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detector::ChunkHealth;
    use crate::planner::RepairTask;
    use std::time::Instant as StdInstant;

    fn make_task(id: &str, source: &str, targets: Vec<&str>) -> RepairTask {
        RepairTask {
            task_id: id.to_string(),
            chunk_id: vec![1, 2, 3],
            source_node: source.to_string(),
            target_nodes: targets.iter().map(|s| s.to_string()).collect(),
            chunk_size: 1024 * 1024,
            priority: 100,
            issue: crate::detector::ChunkIssue {
                chunk_id: vec![1, 2, 3],
                health: ChunkHealth::UnderReplicated {
                    current: 1,
                    target: 3,
                },
                current_nodes: vec![source.to_string()],
                file_id: None,
                priority: 100,
                detected_at: StdInstant::now(),
            },
        }
    }

    #[test]
    fn test_executor_config_default() {
        let config = ExecutorConfig::default();
        assert_eq!(config.max_concurrent, 10);
        assert_eq!(config.max_retries, 3);
        assert!(config.report_progress);
    }

    #[test]
    fn test_execution_result_success_rate() {
        let mut result = ExecutionResult::default();
        assert_eq!(result.success_rate(), 100.0);

        result.succeeded.push(TaskResult {
            task_id: "1".to_string(),
            success: true,
            error: None,
            bytes_transferred: 100,
            duration: Duration::from_secs(1),
            targets_succeeded: vec!["n1".to_string()],
            targets_failed: vec![],
        });

        result.failed.push(TaskResult {
            task_id: "2".to_string(),
            success: false,
            error: Some(ExecutorError::Timeout),
            bytes_transferred: 0,
            duration: Duration::from_secs(1),
            targets_succeeded: vec![],
            targets_failed: vec!["n2".to_string()],
        });

        assert_eq!(result.success_rate(), 50.0);
    }

    #[tokio::test]
    async fn test_executor_execute_empty_plan() {
        let executor = Executor::new(ExecutorConfig::default());
        let plan = RepairPlan::default();

        let result = executor
            .execute(plan, |_, _, _, _| async { Ok(vec![]) })
            .await;

        assert_eq!(result.succeeded.len(), 0);
        assert_eq!(result.failed.len(), 0);
    }

    #[tokio::test]
    async fn test_executor_execute_success() {
        let executor = Executor::new(ExecutorConfig::default());

        let mut plan = RepairPlan::default();
        plan.add_task(make_task("task1", "n1", vec!["n2", "n3"]));

        let result = executor
            .execute(plan, |_, _, _, targets| async move {
                // Simulate successful transfer to all targets
                Ok(targets)
            })
            .await;

        assert_eq!(result.succeeded.len(), 1);
        assert_eq!(result.failed.len(), 0);
        assert!(result.succeeded[0].targets_failed.is_empty());
    }

    #[tokio::test]
    async fn test_executor_execute_failure() {
        let executor = Executor::new(ExecutorConfig {
            max_retries: 0, // No retries
            ..Default::default()
        });

        let mut plan = RepairPlan::default();
        plan.add_task(make_task("task1", "n1", vec!["n2"]));

        let result = executor
            .execute(plan, |_, _, _, _| async {
                Err("Transfer failed".to_string())
            })
            .await;

        assert_eq!(result.succeeded.len(), 0);
        assert_eq!(result.failed.len(), 1);
    }

    #[test]
    fn test_progress_status_display() {
        let update = ProgressUpdate {
            task_id: "test".to_string(),
            bytes_transferred: 500,
            total_bytes: 1000,
            percent: 50.0,
            status: ProgressStatus::Running,
        };

        assert_eq!(update.percent, 50.0);
        assert_eq!(update.status, ProgressStatus::Running);
    }
}
