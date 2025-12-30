//! CyxCloud Storage Node Library
//!
//! Provides components for running a distributed storage node:
//! - Configuration management
//! - Prometheus metrics and health checking
//! - Heartbeat service for central server registration
//! - Command execution (repair, delete, transfer chunks)
//! - P2P network announcements
//! - CyxWiz API integration for machine management
//! - Blockchain integration for Solana (optional)

pub mod command_executor;
pub mod config;
pub mod cyxwiz_api_client;
pub mod health;
pub mod machine_service;
pub mod metrics;
pub mod symbols;

#[cfg(feature = "blockchain")]
pub mod blockchain;

pub use config::{
    BlockchainSettings, CentralServerSettings, ConfigError, CyxWizApiSettings, MetricsSettings,
    NetworkSettings, NodeConfig, NodeIdentity, StorageSettings,
};

#[cfg(feature = "blockchain")]
pub use blockchain::{
    constants as blockchain_constants, DiskType, NodeBlockchainConfig, ProofChallenge,
    ProofOfStorage, StorageNodeBlockchainClient, StorageNodeStatus, StorageSpec,
};
pub use cyxwiz_api_client::{
    CyxWizApiClient, DetectedHardware, CpuInfo, GpuInfo, LoginResponse, SavedCredentials, UserInfo,
};
pub use health::{
    HealthChecker, HeartbeatService, NodeAnnouncement, NodeAnnouncer,
    NodeCapacity2 as NodeCapacity, NodeStatus2 as NodeStatus,
};
pub use command_executor::{CommandBatchSummary, CommandExecutor, CommandResult, CommandType};
pub use machine_service::MachineService;
pub use metrics::{init_metrics, HealthState, MetricsServer, NodeMetrics};
