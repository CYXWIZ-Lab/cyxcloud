//! PostgreSQL-backed metadata client for rebalancer
//!
//! Implements the MetadataClient trait using cyxcloud-metadata's Database.

use crate::detector::{ChunkInfo, MetadataClient};
use cyxcloud_metadata::postgres::Database;
use std::sync::Arc;
use tracing::{debug, instrument};

/// PostgreSQL metadata client
pub struct PostgresMetadataClient {
    db: Arc<Database>,
}

impl PostgresMetadataClient {
    /// Create a new PostgreSQL metadata client
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    /// Get the underlying database reference
    pub fn database(&self) -> &Database {
        &self.db
    }
}

#[async_trait::async_trait]
impl MetadataClient for PostgresMetadataClient {
    #[instrument(skip(self))]
    async fn get_under_replicated_chunks(
        &self,
        limit: usize,
    ) -> Result<Vec<ChunkInfo>, Box<dyn std::error::Error + Send + Sync>> {
        // Query under-replicated chunks from the database
        let chunks = self
            .db
            .get_under_replicated_chunks(limit as i64)
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;

        debug!(count = chunks.len(), "Found under-replicated chunks");

        // Convert to ChunkInfo with node locations
        let mut result = Vec::with_capacity(chunks.len());

        for chunk in chunks {
            // Get node IDs for this chunk
            let locations = self
                .db
                .get_chunk_locations(&chunk.chunk_id)
                .await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;

            // Get the peer_id for each node (the detector uses peer_id as node identifier)
            let mut node_ids = Vec::with_capacity(locations.len());
            for loc in &locations {
                if let Ok(Some(node)) = self.db.get_node(loc.node_id).await {
                    node_ids.push(node.peer_id);
                }
            }

            // Get chunk size from the chunks table
            let chunk_record = self
                .db
                .get_chunk_by_id(&chunk.chunk_id)
                .await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;

            let size = chunk_record.map(|c| c.size_bytes as u64).unwrap_or(0);

            result.push(ChunkInfo {
                chunk_id: chunk.chunk_id,
                node_ids,
                file_id: Some(chunk.file_id.to_string()),
                size,
            });
        }

        Ok(result)
    }

    #[instrument(skip(self))]
    async fn get_orphaned_chunks(
        &self,
        _limit: usize,
    ) -> Result<Vec<ChunkInfo>, Box<dyn std::error::Error + Send + Sync>> {
        // Orphaned chunks are chunks with no file reference
        // For now, return empty - this would need a separate query
        // TODO: Implement orphan detection query
        debug!("Orphaned chunk detection not yet implemented");
        Ok(Vec::new())
    }
}
