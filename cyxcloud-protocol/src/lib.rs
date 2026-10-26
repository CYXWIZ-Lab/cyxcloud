//! CyxCloud Protocol Definitions
//!
//! Generated Rust code from Protocol Buffers.
//!
//! # Services
//! - `ChunkService` - Store/retrieve data chunks
//! - `MetadataService` - File and chunk index operations
//! - `NodeService` - Node registration and health
//! - `DataService` - Stream ML training data (basic)
//! - `DataStreamService` - Zero-copy ML training data with verification

// This crate is only tonic/prost output. Newer clippy (CI's stable, not yet
// 1.91 locally) flags the generated clients: `#[must_use]` on methods that
// return a Future, and `Result<_, tonic::Status>` being large. Neither is ours
// to change, so they are allowed here and nowhere else.
#![allow(clippy::double_must_use, clippy::result_large_err)]

/// Chunk service messages and client/server
pub mod chunk {
    tonic::include_proto!("cyxcloud.chunk");
}

/// Metadata service messages and client/server
pub mod metadata {
    tonic::include_proto!("cyxcloud.metadata");
}

/// Node service messages and client/server
pub mod node {
    tonic::include_proto!("cyxcloud.node");
}

/// Data streaming service messages and client/server
pub mod data {
    tonic::include_proto!("cyxcloud.data");
}

/// DataStream service - zero-copy ML training data with verification
pub mod datastream {
    tonic::include_proto!("cyxcloud.datastream");
}

// Re-export commonly used types
pub use chunk::chunk_service_client::ChunkServiceClient;
pub use chunk::chunk_service_server::{ChunkService, ChunkServiceServer};
pub use data::data_service_client::DataServiceClient;
pub use data::data_service_server::{DataService, DataServiceServer};
pub use datastream::data_stream_service_client::DataStreamServiceClient;
pub use datastream::data_stream_service_server::{DataStreamService, DataStreamServiceServer};
pub use metadata::metadata_service_client::MetadataServiceClient;
pub use metadata::metadata_service_server::{MetadataService, MetadataServiceServer};
pub use node::node_service_client::NodeServiceClient;
pub use node::node_service_server::{NodeService, NodeServiceServer};
