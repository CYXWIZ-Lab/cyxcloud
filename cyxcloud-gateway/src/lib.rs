//! CyxCloud API Gateway library
//!
//! Re-exports core types for integration testing and external use.

#![allow(clippy::double_ended_iterator_last)]
#![allow(clippy::useless_conversion)]
#![allow(clippy::result_large_err)]
#![allow(clippy::too_many_arguments)]
#![allow(clippy::type_complexity)]
#![allow(dead_code)]

pub mod audit;
pub mod auth;
mod auth_api;
#[cfg(feature = "blockchain")]
pub mod blockchain;
mod data_access;
mod dataset_api;
mod datastream;
mod grpc_api;
pub mod metrics;
mod node_client;
mod node_monitor;
mod payment_daemon;
mod public_registry;
mod rebalancer_daemon;
mod s3_api;
pub mod state;
mod verification;
mod websocket;

pub use auth::{AuthConfig, AuthService};
#[cfg(feature = "blockchain")]
pub use blockchain::{BlockchainConfig, CyxCloudBlockchainClient};
pub use state::{AppState, GatewayConfig};
