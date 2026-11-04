//! CyxCloud Network Layer
//!
//! Node-to-node chunk transfer over gRPC (tonic):
//! - `grpc_server`: the ChunkService a storage node serves (store, get,
//!   delete, stream, verify);
//! - `grpc_client`: `ChunkClient`, used by the gateway's peers (the node's
//!   command executor, the rebalancer) to reach a node.
//!
//! Nodes find each other through the gateway's node registry, not peer to
//! peer (the unused libp2p discovery was removed, TOFIX192).

#![allow(clippy::await_holding_lock)]
#![allow(clippy::redundant_locals)]
#![allow(clippy::result_large_err)]
#![allow(clippy::derivable_impls)]
#![allow(clippy::should_implement_trait)]

pub mod grpc_client;
pub mod grpc_server;

pub use grpc_client::{ChunkClient, ChunkClientConfig};
pub use grpc_server::{ChunkServiceImpl, GrpcServerConfig};
