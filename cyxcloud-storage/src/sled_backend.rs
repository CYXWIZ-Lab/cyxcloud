//! Sled-based metadata storage
//!
//! Used for storing file metadata, chunk mappings, and other structured data.
//! Sled provides ACID transactions and is pure Rust.

use cyxcloud_core::chunk::{ChunkId, ChunkMetadata};
use cyxcloud_core::error::{CyxCloudError, Result};
use serde::{de::DeserializeOwned, Serialize};
use std::path::Path;
use tracing::{debug, info};
use uuid::Uuid;

/// File metadata stored in the database
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FileMetadata {
    /// Unique file ID
    pub id: Uuid,

    /// Original filename
    pub name: String,

    /// Total file size in bytes
    pub size: u64,

    /// MIME type (if detected)
    pub content_type: Option<String>,

    /// List of chunk IDs that make up this file
    pub chunks: Vec<ChunkId>,

    /// Whether the file is encrypted
    pub encrypted: bool,

    /// Creation timestamp
    pub created_at: i64,

    /// Last modified timestamp
    pub modified_at: i64,

    /// Owner's public key or wallet address
    pub owner: Option<String>,

    /// Custom metadata (JSON-like)
    pub custom: std::collections::HashMap<String, String>,
}

impl FileMetadata {
    /// Create new file metadata
    pub fn new(name: impl Into<String>, size: u64) -> Self {
        let now = chrono::Utc::now().timestamp();
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            size,
            content_type: None,
            chunks: Vec::new(),
            encrypted: false,
            created_at: now,
            modified_at: now,
            owner: None,
            custom: std::collections::HashMap::new(),
        }
    }

    /// Add chunks to the file
    pub fn with_chunks(mut self, chunks: Vec<ChunkId>) -> Self {
        self.chunks = chunks;
        self
    }

    /// Set owner
    pub fn with_owner(mut self, owner: impl Into<String>) -> Self {
        self.owner = Some(owner.into());
        self
    }

    /// Mark as encrypted
    pub fn with_encryption(mut self) -> Self {
        self.encrypted = true;
        self
    }
}

/// Sled-based metadata store
pub struct SledMetadataStore {
    db: sled::Db,
}

impl SledMetadataStore {
    /// Open or create a metadata store
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        info!(path = ?path.as_ref(), "Opening Sled metadata store");

        let db = sled::open(path.as_ref())
            .map_err(|e| CyxCloudError::Storage(format!("Failed to open Sled: {}", e)))?;

        Ok(Self { db })
    }

    /// Open an in-memory store (for testing)
    pub fn open_temporary() -> Result<Self> {
        let config = sled::Config::new().temporary(true);
        let db = config
            .open()
            .map_err(|e| CyxCloudError::Storage(format!("Failed to open Sled: {}", e)))?;
        Ok(Self { db })
    }

    /// Get a tree (namespace) for storing data
    fn tree(&self, name: &str) -> Result<sled::Tree> {
        self.db
            .open_tree(name)
            .map_err(|e| CyxCloudError::Storage(e.to_string()))
    }

    /// Store a serializable value
    fn put_value<K: AsRef<[u8]>, V: Serialize>(&self, tree: &sled::Tree, key: K, value: &V) -> Result<()> {
        let encoded = bincode::serialize(value)?;
        tree.insert(key, encoded)
            .map_err(|e| CyxCloudError::Storage(e.to_string()))?;
        Ok(())
    }

    /// Get a deserializable value
    fn get_value<K: AsRef<[u8]>, V: DeserializeOwned>(&self, tree: &sled::Tree, key: K) -> Result<Option<V>> {
        match tree.get(key).map_err(|e| CyxCloudError::Storage(e.to_string()))? {
            Some(bytes) => {
                let value: V = bincode::deserialize(&bytes)?;
                Ok(Some(value))
            }
            None => Ok(None),
        }
    }

    // ===== File Metadata Operations =====

    /// Store file metadata
    pub fn put_file(&self, file: &FileMetadata) -> Result<()> {
        let tree = self.tree("files")?;
        self.put_value(&tree, file.id.as_bytes(), file)?;

        // Also index by name for lookups
        let name_tree = self.tree("files_by_name")?;
        name_tree
            .insert(file.name.as_bytes(), file.id.as_bytes())
            .map_err(|e| CyxCloudError::Storage(e.to_string()))?;

        debug!(file_id = %file.id, name = %file.name, "Stored file metadata");
        Ok(())
    }

    /// Get file metadata by ID
    pub fn get_file(&self, id: Uuid) -> Result<Option<FileMetadata>> {
        let tree = self.tree("files")?;
        self.get_value(&tree, id.as_bytes())
    }

    /// Get file metadata by name
    pub fn get_file_by_name(&self, name: &str) -> Result<Option<FileMetadata>> {
        let name_tree = self.tree("files_by_name")?;

        match name_tree.get(name.as_bytes()).map_err(|e| CyxCloudError::Storage(e.to_string()))? {
            Some(id_bytes) => {
                let id = Uuid::from_slice(&id_bytes)
                    .map_err(|e| CyxCloudError::Storage(e.to_string()))?;
                self.get_file(id)
            }
            None => Ok(None),
        }
    }

    /// Delete file metadata
    pub fn delete_file(&self, id: Uuid) -> Result<bool> {
        let tree = self.tree("files")?;

        // Get file first to remove name index
        if let Some(file) = self.get_file(id)? {
            let name_tree = self.tree("files_by_name")?;
            name_tree
                .remove(file.name.as_bytes())
                .map_err(|e| CyxCloudError::Storage(e.to_string()))?;
        }

        let removed = tree
            .remove(id.as_bytes())
            .map_err(|e| CyxCloudError::Storage(e.to_string()))?;

        Ok(removed.is_some())
    }

    /// List all files
    pub fn list_files(&self) -> Result<Vec<FileMetadata>> {
        let tree = self.tree("files")?;
        let mut files = Vec::new();

        for item in tree.iter() {
            let (_, value) = item.map_err(|e| CyxCloudError::Storage(e.to_string()))?;
            let file: FileMetadata = bincode::deserialize(&value)?;
            files.push(file);
        }

        Ok(files)
    }

    // ===== Chunk Metadata Operations =====

    /// Store chunk metadata
    pub fn put_chunk_metadata(&self, metadata: &ChunkMetadata) -> Result<()> {
        let tree = self.tree("chunks")?;
        self.put_value(&tree, metadata.id.as_bytes(), metadata)?;
        Ok(())
    }

    /// Get chunk metadata
    pub fn get_chunk_metadata(&self, id: ChunkId) -> Result<Option<ChunkMetadata>> {
        let tree = self.tree("chunks")?;
        self.get_value(&tree, id.as_bytes())
    }

    // ===== Node Location Tracking =====

    /// Record which node stores which chunk
    pub fn set_chunk_location(&self, chunk_id: ChunkId, node_id: &str) -> Result<()> {
        let tree = self.tree("chunk_locations")?;

        // Get existing locations
        let mut locations: Vec<String> = self
            .get_value(&tree, chunk_id.as_bytes())?
            .unwrap_or_default();

        // Add if not already present
        if !locations.contains(&node_id.to_string()) {
            locations.push(node_id.to_string());
            self.put_value(&tree, chunk_id.as_bytes(), &locations)?;
        }

        Ok(())
    }

    /// Get all nodes storing a chunk
    pub fn get_chunk_locations(&self, chunk_id: ChunkId) -> Result<Vec<String>> {
        let tree = self.tree("chunk_locations")?;
        Ok(self.get_value(&tree, chunk_id.as_bytes())?.unwrap_or_default())
    }

    /// Remove a node from chunk location
    pub fn remove_chunk_location(&self, chunk_id: ChunkId, node_id: &str) -> Result<()> {
        let tree = self.tree("chunk_locations")?;

        if let Some(mut locations) = self.get_value::<_, Vec<String>>(&tree, chunk_id.as_bytes())? {
            locations.retain(|n| n != node_id);
            self.put_value(&tree, chunk_id.as_bytes(), &locations)?;
        }

        Ok(())
    }

    // ===== General Operations =====

    /// Flush to disk
    pub fn flush(&self) -> Result<()> {
        self.db
            .flush()
            .map_err(|e| CyxCloudError::Storage(e.to_string()))?;
        Ok(())
    }

    /// Get database size estimate
    pub fn size_on_disk(&self) -> u64 {
        self.db.size_on_disk().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_metadata() {
        let store = SledMetadataStore::open_temporary().unwrap();

        let file = FileMetadata::new("test.txt", 1024)
            .with_owner("owner123")
            .with_encryption();

        store.put_file(&file).unwrap();

        // Get by ID
        let retrieved = store.get_file(file.id).unwrap().unwrap();
        assert_eq!(retrieved.name, "test.txt");
        assert_eq!(retrieved.size, 1024);
        assert!(retrieved.encrypted);

        // Get by name
        let by_name = store.get_file_by_name("test.txt").unwrap().unwrap();
        assert_eq!(by_name.id, file.id);
    }

    #[test]
    fn test_list_files() {
        let store = SledMetadataStore::open_temporary().unwrap();

        for i in 0..5 {
            let file = FileMetadata::new(format!("file_{}.txt", i), i * 100);
            store.put_file(&file).unwrap();
        }

        let files = store.list_files().unwrap();
        assert_eq!(files.len(), 5);
    }

    #[test]
    fn test_chunk_locations() {
        let store = SledMetadataStore::open_temporary().unwrap();
        let chunk_id = ChunkId::from_data(b"test");

        store.set_chunk_location(chunk_id, "node1").unwrap();
        store.set_chunk_location(chunk_id, "node2").unwrap();
        store.set_chunk_location(chunk_id, "node1").unwrap(); // Duplicate

        let locations = store.get_chunk_locations(chunk_id).unwrap();
        assert_eq!(locations.len(), 2);
        assert!(locations.contains(&"node1".to_string()));
        assert!(locations.contains(&"node2".to_string()));

        store.remove_chunk_location(chunk_id, "node1").unwrap();
        let locations = store.get_chunk_locations(chunk_id).unwrap();
        assert_eq!(locations.len(), 1);
        assert!(!locations.contains(&"node1".to_string()));
    }

    #[test]
    fn test_delete_file() {
        let store = SledMetadataStore::open_temporary().unwrap();

        let file = FileMetadata::new("delete_me.txt", 100);
        let id = file.id;
        store.put_file(&file).unwrap();

        assert!(store.get_file(id).unwrap().is_some());
        assert!(store.delete_file(id).unwrap());
        assert!(store.get_file(id).unwrap().is_none());
        assert!(store.get_file_by_name("delete_me.txt").unwrap().is_none());
    }
}
