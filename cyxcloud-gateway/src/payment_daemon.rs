//! Payment Daemon
//!
//! Background task that manages payment epochs:
//! - Accumulates uptime for all nodes every minute
//! - Ends epochs and distributes payments when due
//! - Handles slashing for extended downtime
//! - Claims platform and community shares

use crate::state::AppState;
use chrono::{DateTime, Utc};
use cyxcloud_metadata::{MetadataService, NodeWeight, SlashReason};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::time::interval;
use tracing::{debug, error, info, warn};

#[cfg(feature = "blockchain")]
use crate::blockchain::CyxCloudBlockchainClient;
#[cfg(feature = "blockchain")]
use solana_sdk::pubkey::Pubkey;
#[cfg(feature = "blockchain")]
use std::str::FromStr;

/// Stub type when blockchain feature is disabled
#[cfg(not(feature = "blockchain"))]
pub struct CyxCloudBlockchainClientStub;

/// Type alias for blockchain client reference
#[cfg(feature = "blockchain")]
type BlockchainRef<'a> = Option<&'a CyxCloudBlockchainClient>;

#[cfg(not(feature = "blockchain"))]
type BlockchainRef<'a> = Option<&'a CyxCloudBlockchainClientStub>;

/// Payment daemon configuration
#[derive(Debug, Clone)]
pub struct PaymentDaemonConfig {
    /// How often to accumulate uptime (default: 60 seconds)
    pub accumulate_interval: Duration,
    /// Extended downtime threshold for slashing (default: 4 hours)
    pub extended_downtime_threshold: Duration,
    /// Enable blockchain transactions (false for dry-run mode)
    pub enable_blockchain: bool,
}

impl Default for PaymentDaemonConfig {
    fn default() -> Self {
        Self {
            accumulate_interval: Duration::from_secs(60),
            extended_downtime_threshold: Duration::from_secs(4 * 60 * 60), // 4 hours
            enable_blockchain: true,
        }
    }
}

impl PaymentDaemonConfig {
    /// Create configuration from environment variables
    pub fn from_env() -> Self {
        Self {
            accumulate_interval: Duration::from_secs(
                std::env::var("PAYMENT_ACCUMULATE_INTERVAL_SECS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(60),
            ),
            extended_downtime_threshold: Duration::from_secs(
                std::env::var("EXTENDED_DOWNTIME_THRESHOLD_SECS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(4 * 60 * 60),
            ),
            enable_blockchain: std::env::var("ENABLE_BLOCKCHAIN_PAYMENTS")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(true),
        }
    }
}

/// Payment daemon metrics
#[derive(Debug, Default, Clone)]
pub struct PaymentDaemonMetrics {
    pub current_epoch: i64,
    pub epoch_start: Option<DateTime<Utc>>,
    pub uptime_updates: u64,
    pub epochs_finalized: u64,
    pub nodes_paid: u64,
    pub total_paid_amount: u64,
    pub slashing_events: u64,
    pub last_accumulate_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

/// Epoch duration in seconds (7 days)
const EPOCH_DURATION_SECONDS: i64 = 7 * 24 * 60 * 60;

/// Payment daemon for managing node payments
pub struct PaymentDaemon {
    config: PaymentDaemonConfig,
    metrics: Arc<RwLock<PaymentDaemonMetrics>>,
}

impl PaymentDaemon {
    /// Create a new payment daemon
    pub fn new(config: PaymentDaemonConfig) -> Self {
        Self {
            config,
            metrics: Arc::new(RwLock::new(PaymentDaemonMetrics::default())),
        }
    }

    /// Start the background payment daemon loop
    pub fn start(self: Arc<Self>, state: Arc<AppState>) -> tokio::task::JoinHandle<()> {
        let daemon = self;
        let accumulate_interval = daemon.config.accumulate_interval;

        tokio::spawn(async move {
            let mut timer = interval(accumulate_interval);

            info!(
                interval_secs = accumulate_interval.as_secs(),
                epoch_duration_days = EPOCH_DURATION_SECONDS / 86400,
                extended_downtime_threshold_hours = daemon.config.extended_downtime_threshold.as_secs() / 3600,
                blockchain_enabled = daemon.config.enable_blockchain,
                "Payment daemon started"
            );

            // Initialize current epoch on startup
            if let Some(metadata) = state.metadata_service() {
                if let Err(e) = daemon.ensure_current_epoch(metadata).await {
                    error!(error = %e, "Failed to initialize payment epoch");
                }
            }

            loop {
                timer.tick().await;

                let metadata = state.metadata_service();

                if let Some(metadata) = metadata {
                    #[cfg(feature = "blockchain")]
                    let blockchain: BlockchainRef = state.blockchain_client();
                    #[cfg(not(feature = "blockchain"))]
                    let blockchain: BlockchainRef = None;
                    // Step 1: Accumulate uptime for all nodes
                    if let Err(e) = daemon.accumulate_uptime(metadata).await {
                        error!(error = %e, "Failed to accumulate uptime");
                        daemon.record_error(&e.to_string()).await;
                    }

                    // Step 2: Check if epoch should end
                    if let Err(e) = daemon.check_epoch_end(metadata, blockchain).await {
                        error!(error = %e, "Failed to check epoch end");
                        daemon.record_error(&e.to_string()).await;
                    }
                } else {
                    debug!("Metadata service not available, skipping payment daemon cycle");
                }
            }
        })
    }

    /// Ensure we have a current epoch in the database
    async fn ensure_current_epoch(&self, metadata: &MetadataService) -> anyhow::Result<()> {
        let db = metadata.database();

        // Check if we have a current (non-finalized) epoch
        if let Some(epoch) = db.get_current_payment_epoch().await? {
            let mut metrics = self.metrics.write().await;
            metrics.current_epoch = epoch.epoch;
            metrics.epoch_start = Some(epoch.started_at);
            info!(epoch = epoch.epoch, started_at = %epoch.started_at, "Resuming existing epoch");
            return Ok(());
        }

        // No current epoch, create epoch 1
        let epoch = db.create_payment_epoch(1).await?;
        let now = Utc::now();

        // Initialize uptime records for all nodes
        let nodes_initialized = db.initialize_epoch_for_all_nodes(1, now).await?;

        let mut metrics = self.metrics.write().await;
        metrics.current_epoch = 1;
        metrics.epoch_start = Some(epoch.started_at);

        info!(
            epoch = 1,
            nodes_initialized = nodes_initialized,
            "Created initial payment epoch"
        );

        Ok(())
    }

    /// Accumulate uptime for all nodes based on current status
    async fn accumulate_uptime(&self, metadata: &MetadataService) -> anyhow::Result<()> {
        let db = metadata.database();
        let seconds = self.config.accumulate_interval.as_secs() as i64;

        // Get current epoch
        let current_epoch = {
            let metrics = self.metrics.read().await;
            metrics.current_epoch
        };

        if current_epoch == 0 {
            return Ok(()); // Not initialized yet
        }

        // Get all nodes and their status
        let nodes = db.get_all_nodes().await?;
        let mut online_count = 0;
        let mut offline_count = 0;

        for node in &nodes {
            // Ensure node has an uptime record for this epoch
            let epoch_start = {
                let metrics = self.metrics.read().await;
                metrics.epoch_start.unwrap_or_else(Utc::now)
            };
            db.create_or_get_epoch_uptime(node.id, current_epoch, epoch_start).await?;

            // Update uptime based on status
            match node.status.as_str() {
                "online" | "recovering" => {
                    db.update_uptime_online(node.id, current_epoch, seconds).await?;
                    online_count += 1;
                }
                _ => {
                    db.update_uptime_offline(node.id, current_epoch, seconds).await?;
                    offline_count += 1;
                }
            }
        }

        // Update metrics
        {
            let mut metrics = self.metrics.write().await;
            metrics.uptime_updates += 1;
            metrics.last_accumulate_at = Some(Utc::now());
        }

        debug!(
            epoch = current_epoch,
            online_nodes = online_count,
            offline_nodes = offline_count,
            seconds = seconds,
            "Uptime accumulated"
        );

        Ok(())
    }

    /// Check if the current epoch should end and process payments
    async fn check_epoch_end(
        &self,
        metadata: &MetadataService,
        blockchain: BlockchainRef<'_>,
    ) -> anyhow::Result<()> {
        let db = metadata.database();

        // Get current epoch info
        let (current_epoch, epoch_start) = {
            let metrics = self.metrics.read().await;
            (metrics.current_epoch, metrics.epoch_start)
        };

        if current_epoch == 0 {
            return Ok(());
        }

        let epoch_start = match epoch_start {
            Some(start) => start,
            None => return Ok(()),
        };

        // Check if epoch duration has elapsed
        let now = Utc::now();
        let elapsed = (now - epoch_start).num_seconds();

        if elapsed < EPOCH_DURATION_SECONDS {
            return Ok(()); // Epoch not ended yet
        }

        info!(
            epoch = current_epoch,
            elapsed_seconds = elapsed,
            "Epoch duration elapsed, finalizing payments"
        );

        // Step 1: End uptime tracking for this epoch
        db.end_epoch_uptime(current_epoch).await?;

        // Step 2: Check for slashing conditions
        self.check_and_slash_nodes(metadata, blockchain, current_epoch).await?;

        // Step 3: Calculate weights and distribute payments
        self.finalize_and_pay_epoch(metadata, blockchain, current_epoch).await?;

        // Step 4: Start new epoch
        let new_epoch = current_epoch + 1;
        let new_epoch_record = db.create_payment_epoch(new_epoch).await?;
        let nodes_initialized = db.initialize_epoch_for_all_nodes(new_epoch, now).await?;

        // Update metrics
        {
            let mut metrics = self.metrics.write().await;
            metrics.current_epoch = new_epoch;
            metrics.epoch_start = Some(new_epoch_record.started_at);
            metrics.epochs_finalized += 1;
        }

        info!(
            old_epoch = current_epoch,
            new_epoch = new_epoch,
            nodes_initialized = nodes_initialized,
            "New epoch started"
        );

        Ok(())
    }

    /// Check for slashing conditions and apply penalties
    #[cfg(feature = "blockchain")]
    async fn check_and_slash_nodes(
        &self,
        metadata: &MetadataService,
        blockchain: BlockchainRef<'_>,
        epoch: i64,
    ) -> anyhow::Result<()> {
        let db = metadata.database();
        let threshold_seconds = self.config.extended_downtime_threshold.as_secs() as i64;

        // Get nodes with extended downtime
        let extended_downtime_nodes = db
            .get_nodes_with_extended_downtime(epoch, threshold_seconds)
            .await?;

        if extended_downtime_nodes.is_empty() {
            debug!(epoch = epoch, "No nodes with extended downtime");
            return Ok(());
        }

        info!(
            epoch = epoch,
            count = extended_downtime_nodes.len(),
            threshold_hours = threshold_seconds / 3600,
            "Found nodes with extended downtime, applying slashing"
        );

        for uptime in &extended_downtime_nodes {
            // Get node info for wallet address
            let node = match db.get_node(uptime.node_id).await? {
                Some(n) => n,
                None => continue,
            };

            let reason = SlashReason::ExtendedDowntime;
            let slash_percent = reason.slash_percent();

            // Record slashing event in database
            let details = serde_json::json!({
                "seconds_offline": uptime.seconds_offline,
                "threshold_seconds": threshold_seconds,
            });

            // Execute slash on blockchain if enabled
            let tx_signature = if self.config.enable_blockchain {
                if let Some(blockchain) = blockchain {
                    if let Some(ref wallet) = node.wallet_address {
                        if let Ok(owner) = Pubkey::from_str(wallet) {
                            match blockchain
                                .slash_node(&owner, &node.peer_id, 0, &reason.to_string())
                                .await
                            {
                                Ok(sig) => Some(sig),
                                Err(e) => {
                                    warn!(
                                        error = %e,
                                        node_id = %node.id,
                                        "Failed to execute slash on blockchain"
                                    );
                                    None
                                }
                            }
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            };

            db.record_slashing_event(
                uptime.node_id,
                epoch,
                &reason.to_string(),
                slash_percent,
                None, // Amount calculated on-chain
                tx_signature.as_deref(),
                Some(details),
            )
            .await?;

            // Update metrics
            {
                let mut metrics = self.metrics.write().await;
                metrics.slashing_events += 1;
            }

            info!(
                node_id = %uptime.node_id,
                peer_id = %node.peer_id,
                reason = %reason,
                slash_percent = slash_percent,
                seconds_offline = uptime.seconds_offline,
                "Slashing applied for extended downtime"
            );
        }

        Ok(())
    }

    /// Check for slashing conditions and apply penalties (no-op when blockchain disabled)
    #[cfg(not(feature = "blockchain"))]
    async fn check_and_slash_nodes(
        &self,
        metadata: &MetadataService,
        _blockchain: BlockchainRef<'_>,
        epoch: i64,
    ) -> anyhow::Result<()> {
        let db = metadata.database();
        let threshold_seconds = self.config.extended_downtime_threshold.as_secs() as i64;

        // Get nodes with extended downtime
        let extended_downtime_nodes = db
            .get_nodes_with_extended_downtime(epoch, threshold_seconds)
            .await?;

        if extended_downtime_nodes.is_empty() {
            debug!(epoch = epoch, "No nodes with extended downtime");
            return Ok(());
        }

        info!(
            epoch = epoch,
            count = extended_downtime_nodes.len(),
            threshold_hours = threshold_seconds / 3600,
            "Found nodes with extended downtime, recording slashing (blockchain disabled)"
        );

        for uptime in &extended_downtime_nodes {
            let node = match db.get_node(uptime.node_id).await? {
                Some(n) => n,
                None => continue,
            };

            let reason = SlashReason::ExtendedDowntime;
            let slash_percent = reason.slash_percent();

            let details = serde_json::json!({
                "seconds_offline": uptime.seconds_offline,
                "threshold_seconds": threshold_seconds,
            });

            db.record_slashing_event(
                uptime.node_id,
                epoch,
                &reason.to_string(),
                slash_percent,
                None,
                None, // No tx signature without blockchain
                Some(details),
            )
            .await?;

            {
                let mut metrics = self.metrics.write().await;
                metrics.slashing_events += 1;
            }

            info!(
                node_id = %uptime.node_id,
                peer_id = %node.peer_id,
                reason = %reason,
                slash_percent = slash_percent,
                "Slashing recorded (blockchain disabled)"
            );
        }

        Ok(())
    }

    /// Finalize epoch and distribute payments (with blockchain)
    #[cfg(feature = "blockchain")]
    async fn finalize_and_pay_epoch(
        &self,
        metadata: &MetadataService,
        blockchain: BlockchainRef<'_>,
        epoch: i64,
    ) -> anyhow::Result<()> {
        let db = metadata.database();

        // Get all uptime records for this epoch
        let uptimes = db.get_epoch_uptime(epoch).await?;

        if uptimes.is_empty() {
            warn!(epoch = epoch, "No uptime records for epoch, skipping payment");
            return Ok(());
        }

        // Get node info for each uptime record to calculate weights
        let mut weights: Vec<NodeWeight> = Vec::new();

        for uptime in &uptimes {
            if let Some(node) = db.get_node(uptime.node_id).await? {
                let reputation = 5000u16; // Average reputation
                let weight = NodeWeight::calculate(
                    uptime.node_id,
                    node.peer_id.clone(),
                    node.wallet_address.clone(),
                    node.storage_total,
                    uptime.seconds_online,
                    EPOCH_DURATION_SECONDS,
                    reputation,
                );
                weights.push(weight);
            }
        }

        if weights.is_empty() {
            warn!(epoch = epoch, "No valid node weights for epoch");
            return Ok(());
        }

        let total_weight: u64 = weights.iter().map(|w| w.weight).sum();

        // Get pool amount from blockchain
        let (total_pool_amount, nodes_share) = if self.config.enable_blockchain {
            if let Some(blockchain) = blockchain {
                if let Ok(Some(rewards)) = blockchain.get_epoch_rewards(epoch as u64).await {
                    (rewards.total_pool_amount, rewards.nodes_share)
                } else {
                    warn!(epoch = epoch, "No pool data available for epoch");
                    return Ok(());
                }
            } else {
                (0, 0)
            }
        } else {
            (1_000_000_000, 850_000_000) // Dry-run mode
        };

        if nodes_share == 0 {
            warn!(epoch = epoch, "No tokens in pool for distribution");
            return Ok(());
        }

        info!(
            epoch = epoch,
            total_pool = total_pool_amount,
            nodes_share = nodes_share,
            total_weight = total_weight,
            node_count = weights.len(),
            "Distributing epoch rewards"
        );

        let mut nodes_paid = 0;
        let mut total_paid = 0u64;

        for weight in &weights {
            let reward = weight.calculate_share(nodes_share, total_weight);
            if reward == 0 {
                continue;
            }

            let tx_signature = if self.config.enable_blockchain {
                if let Some(blockchain) = blockchain {
                    if let Some(ref wallet) = weight.wallet_address {
                        if let Ok(owner) = Pubkey::from_str(wallet) {
                            match blockchain
                                .allocate_node_reward(epoch as u64, &owner, &weight.peer_id, reward)
                                .await
                            {
                                Ok(sig) => Some(sig),
                                Err(e) => {
                                    warn!(error = %e, node_id = %weight.node_id, "Failed to allocate reward");
                                    None
                                }
                            }
                        } else { None }
                    } else { None }
                } else { None }
            } else { None };

            db.mark_payment_allocated(weight.node_id, epoch, reward as i64, tx_signature.as_deref()).await?;
            nodes_paid += 1;
            total_paid += reward;

            debug!(
                node_id = %weight.node_id,
                peer_id = %weight.peer_id,
                reward = reward,
                weight = weight.weight,
                "Reward allocated"
            );
        }

        let finalize_tx = if self.config.enable_blockchain {
            if let Some(blockchain) = blockchain {
                blockchain.finalize_epoch(epoch as u64).await.ok()
            } else { None }
        } else { None };

        let platform_share = (total_pool_amount as f64 * 0.10) as i64;
        let community_share = (total_pool_amount as f64 * 0.05) as i64;

        db.finalize_payment_epoch(
            epoch, total_pool_amount as i64, nodes_share as i64,
            platform_share, community_share, nodes_paid, finalize_tx.as_deref(),
        ).await?;

        {
            let mut metrics = self.metrics.write().await;
            metrics.nodes_paid += nodes_paid as u64;
            metrics.total_paid_amount += total_paid;
        }

        info!(epoch = epoch, nodes_paid = nodes_paid, total_paid = total_paid, "Epoch payment complete");
        Ok(())
    }

    /// Finalize epoch and distribute payments (without blockchain - dry-run mode)
    #[cfg(not(feature = "blockchain"))]
    async fn finalize_and_pay_epoch(
        &self,
        metadata: &MetadataService,
        _blockchain: BlockchainRef<'_>,
        epoch: i64,
    ) -> anyhow::Result<()> {
        let db = metadata.database();

        let uptimes = db.get_epoch_uptime(epoch).await?;
        if uptimes.is_empty() {
            warn!(epoch = epoch, "No uptime records for epoch, skipping payment");
            return Ok(());
        }

        let mut weights: Vec<NodeWeight> = Vec::new();
        for uptime in &uptimes {
            if let Some(node) = db.get_node(uptime.node_id).await? {
                let reputation = 5000u16;
                let weight = NodeWeight::calculate(
                    uptime.node_id, node.peer_id.clone(), node.wallet_address.clone(),
                    node.storage_total, uptime.seconds_online, EPOCH_DURATION_SECONDS, reputation,
                );
                weights.push(weight);
            }
        }

        if weights.is_empty() {
            warn!(epoch = epoch, "No valid node weights for epoch");
            return Ok(());
        }

        let total_weight: u64 = weights.iter().map(|w| w.weight).sum();

        // Dry-run mode: calculate theoretical amounts
        let total_pool_amount = 1_000_000_000u64;
        let nodes_share = 850_000_000u64;

        info!(
            epoch = epoch,
            total_pool = total_pool_amount,
            nodes_share = nodes_share,
            total_weight = total_weight,
            node_count = weights.len(),
            "Distributing epoch rewards (dry-run)"
        );

        let mut nodes_paid = 0;
        let mut total_paid = 0u64;

        for weight in &weights {
            let reward = weight.calculate_share(nodes_share, total_weight);
            if reward == 0 { continue; }

            db.mark_payment_allocated(weight.node_id, epoch, reward as i64, None).await?;
            nodes_paid += 1;
            total_paid += reward;

            debug!(
                node_id = %weight.node_id,
                peer_id = %weight.peer_id,
                reward = reward,
                "Reward allocated (dry-run)"
            );
        }

        let platform_share = (total_pool_amount as f64 * 0.10) as i64;
        let community_share = (total_pool_amount as f64 * 0.05) as i64;

        db.finalize_payment_epoch(
            epoch, total_pool_amount as i64, nodes_share as i64,
            platform_share, community_share, nodes_paid, None,
        ).await?;

        {
            let mut metrics = self.metrics.write().await;
            metrics.nodes_paid += nodes_paid as u64;
            metrics.total_paid_amount += total_paid;
        }

        info!(epoch = epoch, nodes_paid = nodes_paid, total_paid = total_paid, "Epoch payment complete (dry-run)");
        Ok(())
    }

    /// Record an error in metrics
    async fn record_error(&self, error: &str) {
        let mut metrics = self.metrics.write().await;
        metrics.last_error = Some(error.to_string());
    }

    /// Get current metrics
    pub async fn get_metrics(&self) -> PaymentDaemonMetrics {
        self.metrics.read().await.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_payment_daemon_config_default() {
        let config = PaymentDaemonConfig::default();
        assert_eq!(config.accumulate_interval.as_secs(), 60);
        assert_eq!(config.extended_downtime_threshold.as_secs(), 4 * 60 * 60);
        assert!(config.enable_blockchain);
    }

    #[test]
    fn test_epoch_duration() {
        assert_eq!(EPOCH_DURATION_SECONDS, 7 * 24 * 60 * 60);
        assert_eq!(EPOCH_DURATION_SECONDS, 604800);
    }

    #[test]
    fn test_payment_daemon_metrics_default() {
        let metrics = PaymentDaemonMetrics::default();
        assert_eq!(metrics.current_epoch, 0);
        assert_eq!(metrics.epochs_finalized, 0);
        assert_eq!(metrics.nodes_paid, 0);
    }
}
