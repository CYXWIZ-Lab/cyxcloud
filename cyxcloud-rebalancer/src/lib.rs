//! CyxCloud Rebalancer Library
//!
//! This crate provides data rebalancing and repair functionality for CyxCloud.
//!
//! The rebalancer monitors cluster health and performs:
//! - Failure detection (find under-replicated chunks)
//! - Data repair (replicate chunks to restore target replication factor)
//! - Rebalancing (distribute data evenly across nodes)
//! - Hot-swap support (drain nodes before shutdown)

pub mod config;
pub mod detector;
pub mod executor;
pub mod metadata_client;
pub mod network_client;
pub mod planner;
pub mod transfer;

// Re-export main types
pub use config::RebalancerConfig;
pub use detector::{
    ChunkHealth, ChunkInfo, ChunkIssue, Detector, DetectorConfig, MetadataClient, NetworkClient,
    NodeAvailability, ScanResult,
};
pub use executor::{
    Executor, ExecutorConfig, ExecutorError, ProgressStatus, ProgressUpdate, TaskResult,
};
pub use metadata_client::PostgresMetadataClient;
pub use network_client::GrpcNetworkClient;
pub use planner::{NodeInfo, Planner, PlannerConfig, RepairPlan, RepairTask};
pub use transfer::{ChunkTransferService, TransferError};
