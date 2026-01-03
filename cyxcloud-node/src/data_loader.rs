//! Streaming DataLoader for ML Training
//!
//! Provides a PyTorch-like DataLoader interface for streaming ML training data
//! from CyxCloud Gateway with automatic batching, shuffling, and prefetching.

use crate::datastream_client::{
    DataStreamClient, DataStreamConfig, DataStreamResult, VerifiedBatch,
};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex, RwLock};
use tracing::{debug, info, instrument, warn};

/// Loader state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoaderState {
    /// Not started
    Idle,
    /// Loading data
    Loading,
    /// Paused (can resume)
    Paused,
    /// Completed current epoch
    EpochComplete,
    /// Error occurred
    Error,
    /// Stopped (cannot resume)
    Stopped,
}

/// Configuration for the DataLoader
#[derive(Debug, Clone)]
pub struct DataLoaderConfig {
    /// Dataset ID to load
    pub dataset_id: String,

    /// Batch size
    pub batch_size: i32,

    /// Number of batches to prefetch
    pub prefetch_batches: i32,

    /// Shuffle data each epoch
    pub shuffle: bool,

    /// Random seed for shuffling
    pub seed: Option<i64>,

    /// Drop last incomplete batch
    pub drop_last: bool,

    /// Number of epochs (None = infinite)
    pub num_epochs: Option<u32>,

    /// Gateway connection config
    pub stream_config: DataStreamConfig,
}

impl Default for DataLoaderConfig {
    fn default() -> Self {
        Self {
            dataset_id: String::new(),
            batch_size: 32,
            prefetch_batches: 4,
            shuffle: true,
            seed: None,
            drop_last: false,
            num_epochs: Some(1),
            stream_config: DataStreamConfig::default(),
        }
    }
}

/// Training batch with metadata
#[derive(Debug, Clone)]
pub struct TrainingBatch {
    /// Batch data items
    pub items: Vec<Vec<u8>>,

    /// Global batch index (across epochs)
    pub global_index: u64,

    /// Epoch-local batch index
    pub epoch_index: u64,

    /// Current epoch number (0-based)
    pub epoch: u32,

    /// Total batches in dataset
    pub total_batches: u64,

    /// Is this the last batch in the epoch?
    pub is_epoch_end: bool,
}

impl From<VerifiedBatch> for TrainingBatch {
    fn from(batch: VerifiedBatch) -> Self {
        Self {
            items: batch.items,
            global_index: batch.index,
            epoch_index: batch.index,
            epoch: 0,
            total_batches: batch.total_batches,
            is_epoch_end: batch.is_last,
        }
    }
}

/// Loader statistics
#[derive(Debug, Clone, Default)]
pub struct LoaderStats {
    /// Total batches loaded
    pub batches_loaded: u64,

    /// Total items loaded
    pub items_loaded: u64,

    /// Total bytes loaded
    pub bytes_loaded: u64,

    /// Current epoch
    pub current_epoch: u32,

    /// Batches in current epoch
    pub epoch_batches: u64,

    /// Average batch load time (ms)
    pub avg_batch_time_ms: f64,
}

/// Streaming DataLoader for ML training
pub struct DataLoader {
    config: DataLoaderConfig,
    client: Arc<Mutex<Option<DataStreamClient>>>,
    state: Arc<RwLock<LoaderState>>,
    stats: Arc<RwLock<LoaderStats>>,
    current_epoch: Arc<AtomicU64>,
    global_batch_count: Arc<AtomicU64>,
    should_stop: Arc<AtomicBool>,
    is_paused: Arc<AtomicBool>,
}

impl DataLoader {
    /// Create a new DataLoader
    pub fn new(config: DataLoaderConfig) -> Self {
        let mut stream_config = config.stream_config.clone();
        stream_config.batch_size = config.batch_size;
        stream_config.prefetch_batches = config.prefetch_batches;

        Self {
            config: DataLoaderConfig {
                stream_config,
                ..config
            },
            client: Arc::new(Mutex::new(None)),
            state: Arc::new(RwLock::new(LoaderState::Idle)),
            stats: Arc::new(RwLock::new(LoaderStats::default())),
            current_epoch: Arc::new(AtomicU64::new(0)),
            global_batch_count: Arc::new(AtomicU64::new(0)),
            should_stop: Arc::new(AtomicBool::new(false)),
            is_paused: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Connect to the DataStream gateway
    #[instrument(skip(self))]
    pub async fn connect(&self) -> DataStreamResult<()> {
        let client = DataStreamClient::connect(self.config.stream_config.clone()).await?;
        *self.client.lock().await = Some(client);
        info!(dataset_id = %self.config.dataset_id, "DataLoader connected");
        Ok(())
    }

    /// Start loading data and return a receiver for batches
    #[instrument(skip(self))]
    pub async fn start(&self) -> DataStreamResult<mpsc::Receiver<DataStreamResult<TrainingBatch>>> {
        // Connect if not already connected
        if self.client.lock().await.is_none() {
            self.connect().await?;
        }

        // Reset state
        self.should_stop.store(false, Ordering::SeqCst);
        self.is_paused.store(false, Ordering::SeqCst);
        self.current_epoch.store(0, Ordering::SeqCst);
        self.global_batch_count.store(0, Ordering::SeqCst);

        *self.state.write().await = LoaderState::Loading;
        *self.stats.write().await = LoaderStats::default();

        let (tx, rx) = mpsc::channel(self.config.prefetch_batches as usize);

        // Clone references for the background task
        let client = self.client.clone();
        let config = self.config.clone();
        let state = self.state.clone();
        let stats = self.stats.clone();
        let current_epoch = self.current_epoch.clone();
        let global_batch_count = self.global_batch_count.clone();
        let should_stop = self.should_stop.clone();
        let is_paused = self.is_paused.clone();

        // Spawn background loader task
        tokio::spawn(async move {
            let mut epoch = 0u32;

            loop {
                // Check epoch limit
                if let Some(max_epochs) = config.num_epochs {
                    if epoch >= max_epochs {
                        debug!(epoch, max_epochs, "Reached epoch limit");
                        break;
                    }
                }

                // Check if should stop
                if should_stop.load(Ordering::SeqCst) {
                    debug!("Loader stopped by request");
                    break;
                }

                current_epoch.store(epoch as u64, Ordering::SeqCst);

                // Update epoch seed for shuffling
                let epoch_seed = config.seed.map(|s| s + epoch as i64);

                info!(epoch, shuffle = config.shuffle, "Starting epoch");

                // Get stream for this epoch
                let mut guard = client.lock().await;
                let client_ref = match guard.as_mut() {
                    Some(c) => c,
                    None => {
                        warn!("DataLoader client not connected");
                        *state.write().await = LoaderState::Error;
                        break;
                    }
                };

                let stream_result = client_ref
                    .stream_batches(&config.dataset_id, 0, config.shuffle, epoch_seed)
                    .await;

                let mut batch_rx = match stream_result {
                    Ok(rx) => rx,
                    Err(e) => {
                        warn!(error = %e, "Failed to start batch stream");
                        *state.write().await = LoaderState::Error;
                        let _ = tx.send(Err(e)).await;
                        break;
                    }
                };

                drop(guard); // Release lock while streaming

                // Stream batches for this epoch
                let mut epoch_batch_count = 0u64;

                while let Some(batch_result) = batch_rx.recv().await {
                    // Check pause state
                    while is_paused.load(Ordering::SeqCst) {
                        *state.write().await = LoaderState::Paused;
                        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

                        if should_stop.load(Ordering::SeqCst) {
                            break;
                        }
                    }

                    if should_stop.load(Ordering::SeqCst) {
                        break;
                    }

                    *state.write().await = LoaderState::Loading;

                    match batch_result {
                        Ok(verified_batch) => {
                            let is_last = verified_batch.is_last;
                            let item_count = verified_batch.item_count;
                            let bytes: u64 =
                                verified_batch.items.iter().map(|i| i.len() as u64).sum();

                            // Convert to TrainingBatch
                            let global_idx = global_batch_count.fetch_add(1, Ordering::SeqCst);
                            let training_batch = TrainingBatch {
                                items: verified_batch.items,
                                global_index: global_idx,
                                epoch_index: epoch_batch_count,
                                epoch,
                                total_batches: verified_batch.total_batches,
                                is_epoch_end: is_last,
                            };

                            epoch_batch_count += 1;

                            // Update stats
                            {
                                let mut s = stats.write().await;
                                s.batches_loaded += 1;
                                s.items_loaded += item_count as u64;
                                s.bytes_loaded += bytes;
                                s.current_epoch = epoch;
                                s.epoch_batches = epoch_batch_count;
                            }

                            // Send batch
                            if tx.send(Ok(training_batch)).await.is_err() {
                                debug!("Batch receiver dropped");
                                *state.write().await = LoaderState::Stopped;
                                return;
                            }

                            if is_last {
                                break;
                            }
                        }
                        Err(e) => {
                            warn!(error = %e, "Batch stream error");
                            *state.write().await = LoaderState::Error;
                            let _ = tx.send(Err(e)).await;
                            return;
                        }
                    }
                }

                // Check if stopped during epoch
                if should_stop.load(Ordering::SeqCst) {
                    debug!("Loader stopped during epoch");
                    break;
                }

                *state.write().await = LoaderState::EpochComplete;
                info!(epoch, batches = epoch_batch_count, "Epoch complete");

                epoch += 1;
            }

            *state.write().await = LoaderState::Stopped;
            debug!("DataLoader finished");
        });

        Ok(rx)
    }

    /// Pause loading
    pub fn pause(&self) {
        self.is_paused.store(true, Ordering::SeqCst);
        debug!("DataLoader paused");
    }

    /// Resume loading
    pub fn resume(&self) {
        self.is_paused.store(false, Ordering::SeqCst);
        debug!("DataLoader resumed");
    }

    /// Stop loading
    pub fn stop(&self) {
        self.should_stop.store(true, Ordering::SeqCst);
        self.is_paused.store(false, Ordering::SeqCst);
        debug!("DataLoader stopped");
    }

    /// Get current state
    pub async fn state(&self) -> LoaderState {
        *self.state.read().await
    }

    /// Get current statistics
    pub async fn stats(&self) -> LoaderStats {
        self.stats.read().await.clone()
    }

    /// Get current epoch
    pub fn current_epoch(&self) -> u32 {
        self.current_epoch.load(Ordering::SeqCst) as u32
    }

    /// Get total batches loaded
    pub fn total_batches(&self) -> u64 {
        self.global_batch_count.load(Ordering::SeqCst)
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

/// Builder for DataLoader
pub struct DataLoaderBuilder {
    config: DataLoaderConfig,
}

impl DataLoaderBuilder {
    /// Create a new builder
    pub fn new(dataset_id: impl Into<String>) -> Self {
        Self {
            config: DataLoaderConfig {
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

    /// Set prefetch count
    pub fn prefetch(mut self, count: i32) -> Self {
        self.config.prefetch_batches = count;
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

    /// Set drop_last behavior
    pub fn drop_last(mut self, drop: bool) -> Self {
        self.config.drop_last = drop;
        self
    }

    /// Set number of epochs
    pub fn epochs(mut self, epochs: u32) -> Self {
        self.config.num_epochs = Some(epochs);
        self
    }

    /// Set infinite epochs
    pub fn infinite(mut self) -> Self {
        self.config.num_epochs = None;
        self
    }

    /// Set gateway address
    pub fn gateway_addr(mut self, addr: impl Into<String>) -> Self {
        self.config.stream_config.gateway_addr = addr.into();
        self
    }

    /// Set access token
    pub fn access_token(mut self, token: impl Into<String>) -> Self {
        self.config.stream_config.access_token = Some(token.into());
        self
    }

    /// Set maximum trust level
    pub fn max_trust_level(mut self, level: i32) -> Self {
        self.config.stream_config.max_trust_level = level;
        self
    }

    /// Build the DataLoader
    pub fn build(self) -> DataLoader {
        DataLoader::new(self.config)
    }
}

impl Default for DataLoaderBuilder {
    fn default() -> Self {
        Self::new("")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_loader_config_default() {
        let config = DataLoaderConfig::default();
        assert_eq!(config.batch_size, 32);
        assert_eq!(config.prefetch_batches, 4);
        assert!(config.shuffle);
        assert_eq!(config.num_epochs, Some(1));
    }

    #[test]
    fn test_builder() {
        let loader = DataLoaderBuilder::new("test-dataset")
            .batch_size(64)
            .prefetch(8)
            .shuffle(false)
            .seed(42)
            .epochs(10)
            .gateway_addr("http://example.com:50052")
            .access_token("test-token")
            .build();

        assert_eq!(loader.config.dataset_id, "test-dataset");
        assert_eq!(loader.config.batch_size, 64);
        assert_eq!(loader.config.prefetch_batches, 8);
        assert!(!loader.config.shuffle);
        assert_eq!(loader.config.seed, Some(42));
        assert_eq!(loader.config.num_epochs, Some(10));
    }

    #[test]
    fn test_loader_initial_state() {
        let loader = DataLoaderBuilder::new("test-dataset").build();

        assert!(!loader.is_paused());
        assert!(!loader.is_stopped());
        assert_eq!(loader.current_epoch(), 0);
        assert_eq!(loader.total_batches(), 0);
    }

    #[test]
    fn test_training_batch_conversion() {
        let verified = VerifiedBatch {
            index: 5,
            items: vec![vec![1, 2, 3]],
            item_count: 1,
            is_last: true,
            total_batches: 10,
        };

        let training: TrainingBatch = verified.into();
        assert_eq!(training.global_index, 5);
        assert_eq!(training.epoch, 0);
        assert!(training.is_epoch_end);
        assert_eq!(training.total_batches, 10);
    }
}
