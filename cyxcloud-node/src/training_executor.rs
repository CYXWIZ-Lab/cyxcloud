//! Training Job Executor
//!
//! Integrates the DataLoader and verification modules for executing
//! ML training jobs that stream data from CyxCloud Gateway.

use crate::data_loader::{DataLoader, DataLoaderBuilder, TrainingBatch};
use crate::datastream_client::{DataStreamConfig, DataStreamResult};
use crate::verification::{DatasetVerification, DatasetVerifier, TrustRequirement, VerificationError};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, error, info, instrument};

/// Training job errors
#[derive(Error, Debug)]
pub enum TrainingError {
    #[error("Dataset verification failed: {0}")]
    VerificationFailed(#[from] VerificationError),

    #[error("Data loading error: {0}")]
    LoadingError(String),

    #[error("Training interrupted")]
    Interrupted,

    #[error("Configuration error: {0}")]
    ConfigError(String),

    #[error("Callback error: {0}")]
    CallbackError(String),
}

impl From<crate::datastream_client::DataStreamError> for TrainingError {
    fn from(e: crate::datastream_client::DataStreamError) -> Self {
        TrainingError::LoadingError(e.to_string())
    }
}

/// Training job configuration
#[derive(Debug, Clone)]
pub struct TrainingJobConfig {
    /// Dataset ID to train on
    pub dataset_id: String,

    /// Batch size
    pub batch_size: i32,

    /// Number of training epochs
    pub epochs: u32,

    /// Shuffle data each epoch
    pub shuffle: bool,

    /// Random seed
    pub seed: Option<i64>,

    /// Trust requirement for dataset
    pub trust_requirement: TrustRequirement,

    /// Verify dataset before training
    pub verify_before_training: bool,

    /// Gateway connection settings
    pub gateway_config: DataStreamConfig,

    /// Prefetch batches
    pub prefetch_batches: i32,
}

impl Default for TrainingJobConfig {
    fn default() -> Self {
        Self {
            dataset_id: String::new(),
            batch_size: 32,
            epochs: 1,
            shuffle: true,
            seed: None,
            trust_requirement: TrustRequirement::VerifiedOrBetter,
            verify_before_training: true,
            gateway_config: DataStreamConfig::default(),
            prefetch_batches: 4,
        }
    }
}

/// Training job status
#[derive(Debug, Clone)]
pub struct TrainingStatus {
    /// Current state
    pub state: TrainingState,

    /// Current epoch (0-based)
    pub epoch: u32,

    /// Total epochs
    pub total_epochs: u32,

    /// Batches processed in current epoch
    pub epoch_batches: u64,

    /// Total batches in dataset
    pub total_batches: u64,

    /// Total batches processed across all epochs
    pub global_batches: u64,

    /// Items processed
    pub items_processed: u64,

    /// Time elapsed
    pub elapsed: Duration,

    /// Estimated time remaining
    pub eta: Option<Duration>,

    /// Dataset verification result (if performed)
    pub verification: Option<DatasetVerification>,

    /// Error message (if any)
    pub error: Option<String>,
}

/// Training state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrainingState {
    /// Not started
    Idle,
    /// Verifying dataset
    Verifying,
    /// Initializing data loader
    Initializing,
    /// Training in progress
    Training,
    /// Paused
    Paused,
    /// Completed successfully
    Completed,
    /// Failed with error
    Failed,
    /// Stopped by user
    Stopped,
}

/// Callback for receiving training batches
pub type BatchCallback = Box<dyn Fn(&TrainingBatch) -> Result<(), String> + Send + Sync>;

/// Callback for status updates
pub type StatusCallback = Box<dyn Fn(&TrainingStatus) + Send + Sync>;

/// Training job executor
pub struct TrainingExecutor {
    config: TrainingJobConfig,
    loader: Option<DataLoader>,
    state: Arc<RwLock<TrainingState>>,
    status: Arc<RwLock<TrainingStatus>>,
    verification: Arc<RwLock<Option<DatasetVerification>>>,
    should_stop: Arc<AtomicBool>,
    is_paused: Arc<AtomicBool>,
    batches_processed: Arc<AtomicU64>,
    items_processed: Arc<AtomicU64>,
    batch_callback: Option<Arc<BatchCallback>>,
    status_callback: Option<Arc<StatusCallback>>,
}

impl TrainingExecutor {
    /// Create a new training executor
    pub fn new(config: TrainingJobConfig) -> Self {
        let initial_status = TrainingStatus {
            state: TrainingState::Idle,
            epoch: 0,
            total_epochs: config.epochs,
            epoch_batches: 0,
            total_batches: 0,
            global_batches: 0,
            items_processed: 0,
            elapsed: Duration::ZERO,
            eta: None,
            verification: None,
            error: None,
        };

        Self {
            config,
            loader: None,
            state: Arc::new(RwLock::new(TrainingState::Idle)),
            status: Arc::new(RwLock::new(initial_status)),
            verification: Arc::new(RwLock::new(None)),
            should_stop: Arc::new(AtomicBool::new(false)),
            is_paused: Arc::new(AtomicBool::new(false)),
            batches_processed: Arc::new(AtomicU64::new(0)),
            items_processed: Arc::new(AtomicU64::new(0)),
            batch_callback: None,
            status_callback: None,
        }
    }

    /// Set callback for processing batches
    pub fn set_batch_callback<F>(&mut self, callback: F)
    where
        F: Fn(&TrainingBatch) -> Result<(), String> + Send + Sync + 'static,
    {
        self.batch_callback = Some(Arc::new(Box::new(callback)));
    }

    /// Set callback for status updates
    pub fn set_status_callback<F>(&mut self, callback: F)
    where
        F: Fn(&TrainingStatus) + Send + Sync + 'static,
    {
        self.status_callback = Some(Arc::new(Box::new(callback)));
    }

    /// Run the training job
    #[instrument(skip(self))]
    pub async fn run(&mut self) -> Result<TrainingStatus, TrainingError> {
        let start_time = Instant::now();

        // Reset state
        self.should_stop.store(false, Ordering::SeqCst);
        self.is_paused.store(false, Ordering::SeqCst);
        self.batches_processed.store(0, Ordering::SeqCst);
        self.items_processed.store(0, Ordering::SeqCst);

        // Step 1: Verify dataset if required
        if self.config.verify_before_training {
            self.update_state(TrainingState::Verifying).await;
            info!(dataset_id = %self.config.dataset_id, "Verifying dataset");

            let verification = self.verify_dataset().await?;
            *self.verification.write().await = Some(verification.clone());
            {
                let mut status = self.status.write().await;
                status.verification = Some(verification);
            }
        }

        // Check if stopped during verification
        if self.should_stop.load(Ordering::SeqCst) {
            self.update_state(TrainingState::Stopped).await;
            return Ok(self.get_status().await);
        }

        // Step 2: Initialize data loader
        self.update_state(TrainingState::Initializing).await;
        info!("Initializing data loader");

        let loader = self.create_loader();
        let batch_rx = loader.start().await.map_err(|e| {
            TrainingError::LoadingError(format!("Failed to start loader: {}", e))
        })?;

        self.loader = Some(loader);

        // Step 3: Training loop
        self.update_state(TrainingState::Training).await;
        info!(
            epochs = self.config.epochs,
            batch_size = self.config.batch_size,
            "Starting training"
        );

        let result = self.training_loop(batch_rx, start_time).await;

        // Cleanup
        if let Some(ref loader) = self.loader {
            loader.stop();
        }

        match result {
            Ok(()) => {
                self.update_state(TrainingState::Completed).await;
                info!("Training completed successfully");
            }
            Err(ref e) => {
                error!(error = %e, "Training failed");
                self.update_state(TrainingState::Failed).await;
                let mut status = self.status.write().await;
                status.error = Some(e.to_string());
            }
        }

        result?;
        Ok(self.get_status().await)
    }

    /// Training loop - processes batches
    async fn training_loop(
        &self,
        mut batch_rx: mpsc::Receiver<DataStreamResult<TrainingBatch>>,
        start_time: Instant,
    ) -> Result<(), TrainingError> {
        let mut last_status_update = Instant::now();
        let status_update_interval = Duration::from_millis(500);

        while let Some(batch_result) = batch_rx.recv().await {
            // Check pause state
            while self.is_paused.load(Ordering::SeqCst) {
                self.update_state(TrainingState::Paused).await;
                tokio::time::sleep(Duration::from_millis(100)).await;

                if self.should_stop.load(Ordering::SeqCst) {
                    return Err(TrainingError::Interrupted);
                }
            }

            // Check stop state
            if self.should_stop.load(Ordering::SeqCst) {
                return Err(TrainingError::Interrupted);
            }

            self.update_state(TrainingState::Training).await;

            let batch = batch_result.map_err(|e| {
                TrainingError::LoadingError(format!("Batch loading failed: {}", e))
            })?;

            // Process batch via callback
            if let Some(ref callback) = self.batch_callback {
                callback(&batch).map_err(TrainingError::CallbackError)?;
            }

            // Update counters
            let global_batches = self.batches_processed.fetch_add(1, Ordering::SeqCst) + 1;
            let items = self.items_processed.fetch_add(batch.items.len() as u64, Ordering::SeqCst)
                + batch.items.len() as u64;

            // Update status periodically
            if last_status_update.elapsed() >= status_update_interval {
                let elapsed = start_time.elapsed();

                let eta = if global_batches > 0 && batch.total_batches > 0 {
                    let batches_per_epoch = batch.total_batches;
                    let total_batches = batches_per_epoch * self.config.epochs as u64;
                    let remaining_batches = total_batches.saturating_sub(global_batches);
                    let avg_batch_time = elapsed.as_secs_f64() / global_batches as f64;
                    Some(Duration::from_secs_f64(
                        avg_batch_time * remaining_batches as f64,
                    ))
                } else {
                    None
                };

                {
                    let mut status = self.status.write().await;
                    status.epoch = batch.epoch;
                    status.epoch_batches = batch.epoch_index + 1;
                    status.total_batches = batch.total_batches;
                    status.global_batches = global_batches;
                    status.items_processed = items;
                    status.elapsed = elapsed;
                    status.eta = eta;
                }

                // Call status callback
                if let Some(ref callback) = self.status_callback {
                    callback(&self.get_status().await);
                }

                last_status_update = Instant::now();
            }

            debug!(
                epoch = batch.epoch,
                batch = batch.epoch_index,
                items = batch.items.len(),
                "Processed batch"
            );
        }

        Ok(())
    }

    /// Verify the dataset
    async fn verify_dataset(&self) -> Result<DatasetVerification, VerificationError> {
        let mut client = crate::datastream_client::DataStreamClient::connect(
            self.config.gateway_config.clone(),
        )
        .await?;

        let verifier = DatasetVerifier::new().trust_requirement(self.config.trust_requirement);

        verifier
            .verify_dataset(&mut client, &self.config.dataset_id)
            .await
    }

    /// Create the data loader
    fn create_loader(&self) -> DataLoader {
        DataLoaderBuilder::new(&self.config.dataset_id)
            .batch_size(self.config.batch_size)
            .prefetch(self.config.prefetch_batches)
            .shuffle(self.config.shuffle)
            .epochs(self.config.epochs)
            .gateway_addr(&self.config.gateway_config.gateway_addr)
            .access_token(
                self.config
                    .gateway_config
                    .access_token
                    .clone()
                    .unwrap_or_default(),
            )
            .max_trust_level(self.config.trust_requirement.max_trust_level())
            .build()
    }

    /// Update state
    async fn update_state(&self, state: TrainingState) {
        *self.state.write().await = state;
        let mut status = self.status.write().await;
        status.state = state;
    }

    /// Get current status
    pub async fn get_status(&self) -> TrainingStatus {
        self.status.read().await.clone()
    }

    /// Get current state
    pub async fn state(&self) -> TrainingState {
        *self.state.read().await
    }

    /// Pause training
    pub fn pause(&self) {
        self.is_paused.store(true, Ordering::SeqCst);
        if let Some(ref loader) = self.loader {
            loader.pause();
        }
        debug!("Training paused");
    }

    /// Resume training
    pub fn resume(&self) {
        self.is_paused.store(false, Ordering::SeqCst);
        if let Some(ref loader) = self.loader {
            loader.resume();
        }
        debug!("Training resumed");
    }

    /// Stop training
    pub fn stop(&self) {
        self.should_stop.store(true, Ordering::SeqCst);
        self.is_paused.store(false, Ordering::SeqCst);
        if let Some(ref loader) = self.loader {
            loader.stop();
        }
        debug!("Training stopped");
    }

    /// Check if paused
    pub fn is_paused(&self) -> bool {
        self.is_paused.load(Ordering::SeqCst)
    }

    /// Check if stopped
    pub fn is_stopped(&self) -> bool {
        self.should_stop.load(Ordering::SeqCst)
    }
}

/// Builder for TrainingExecutor
pub struct TrainingExecutorBuilder {
    config: TrainingJobConfig,
}

impl TrainingExecutorBuilder {
    /// Create a new builder
    pub fn new(dataset_id: impl Into<String>) -> Self {
        Self {
            config: TrainingJobConfig {
                dataset_id: dataset_id.into(),
                ..Default::default()
            },
        }
    }

    /// Set batch size
    pub fn batch_size(mut self, size: i32) -> Self {
        self.config.batch_size = size;
        self
    }

    /// Set number of epochs
    pub fn epochs(mut self, epochs: u32) -> Self {
        self.config.epochs = epochs;
        self
    }

    /// Enable/disable shuffling
    pub fn shuffle(mut self, shuffle: bool) -> Self {
        self.config.shuffle = shuffle;
        self
    }

    /// Set random seed
    pub fn seed(mut self, seed: i64) -> Self {
        self.config.seed = Some(seed);
        self
    }

    /// Set trust requirement
    pub fn trust_requirement(mut self, requirement: TrustRequirement) -> Self {
        self.config.trust_requirement = requirement;
        self
    }

    /// Enable/disable pre-training verification
    pub fn verify_before_training(mut self, verify: bool) -> Self {
        self.config.verify_before_training = verify;
        self
    }

    /// Set gateway address
    pub fn gateway_addr(mut self, addr: impl Into<String>) -> Self {
        self.config.gateway_config.gateway_addr = addr.into();
        self
    }

    /// Set access token
    pub fn access_token(mut self, token: impl Into<String>) -> Self {
        self.config.gateway_config.access_token = Some(token.into());
        self
    }

    /// Set prefetch batches
    pub fn prefetch(mut self, count: i32) -> Self {
        self.config.prefetch_batches = count;
        self
    }

    /// Build the executor
    pub fn build(self) -> TrainingExecutor {
        TrainingExecutor::new(self.config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let config = TrainingJobConfig::default();
        assert_eq!(config.batch_size, 32);
        assert_eq!(config.epochs, 1);
        assert!(config.shuffle);
        assert!(config.verify_before_training);
    }

    #[test]
    fn test_builder() {
        let executor = TrainingExecutorBuilder::new("test-dataset")
            .batch_size(64)
            .epochs(10)
            .shuffle(false)
            .seed(42)
            .trust_requirement(TrustRequirement::Any)
            .verify_before_training(false)
            .gateway_addr("http://gateway:50052")
            .build();

        assert_eq!(executor.config.dataset_id, "test-dataset");
        assert_eq!(executor.config.batch_size, 64);
        assert_eq!(executor.config.epochs, 10);
        assert!(!executor.config.shuffle);
        assert_eq!(executor.config.seed, Some(42));
        assert!(!executor.config.verify_before_training);
    }

    #[test]
    fn test_initial_state() {
        let executor = TrainingExecutorBuilder::new("test").build();
        assert!(!executor.is_paused());
        assert!(!executor.is_stopped());
    }

    #[tokio::test]
    async fn test_status_initial() {
        let executor = TrainingExecutorBuilder::new("test").epochs(5).build();
        let status = executor.get_status().await;

        assert_eq!(status.state, TrainingState::Idle);
        assert_eq!(status.epoch, 0);
        assert_eq!(status.total_epochs, 5);
        assert_eq!(status.global_batches, 0);
    }
}
