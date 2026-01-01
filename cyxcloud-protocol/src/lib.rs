//! CyxCloud Protocol Definitions
//!
//! Generated Rust code from Protocol Buffers.
//!
//! # Services
//! - `ChunkService` - Store/retrieve data chunks
//! - `MetadataService` - File and chunk index operations
//! - `NodeService` - Node registration and health
//! - `DataService` - Stream ML training data

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

// Re-export commonly used types
pub use chunk::chunk_service_client::ChunkServiceClient;
pub use chunk::chunk_service_server::{ChunkService, ChunkServiceServer};
pub use data::data_service_client::DataServiceClient;
pub use data::data_service_server::{DataService, DataServiceServer};
pub use metadata::metadata_service_client::MetadataServiceClient;
pub use metadata::metadata_service_server::{MetadataService, MetadataServiceServer};
pub use node::node_service_client::NodeServiceClient;
pub use node::node_service_server::{NodeService, NodeServiceServer};
