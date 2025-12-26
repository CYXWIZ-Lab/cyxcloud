//! Chunk types and metadata
//!
//! Chunks are the fundamental unit of storage in CyxCloud.
//! Each chunk is content-addressed using Blake3 hashing.

use crate::crypto::ContentHash;
use crate::error::{CyxCloudError, Result};
use crate::{MAX_CHUNK_SIZE, MIN_CHUNK_SIZE};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

/// Content-addressed chunk identifier (CID)
///
/// Format: `cyx://` + base58-encoded Blake3 hash
/// Example: `cyx://2DrjgbN3Y5AHN4inRXnff2MrzqXJC1JYopNtXmMTBCry`
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ChunkId([u8; 32]);

impl ChunkId {
    /// Create a new ChunkId from raw bytes
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Create a ChunkId from a Blake3 hash
    pub fn from_hash(hash: &ContentHash) -> Self {
        Self(*hash.as_bytes())
    }

    /// Compute ChunkId from data (content-addressing)
    pub fn from_data(data: &[u8]) -> Self {
        let hash = ContentHash::compute(data);
        Self::from_hash(&hash)
    }

    /// Get the raw bytes
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Convert to base58 string (for URLs and display)
    pub fn to_base58(&self) -> String {
        bs58::encode(&self.0).into_string()
    }

    /// Parse from base58 string
    pub fn from_base58(s: &str) -> Result<Self> {
        let bytes = bs58::decode(s)
            .into_vec()
            .map_err(|e| CyxCloudError::InvalidChunkId(e.to_string()))?;

        if bytes.len() != 32 {
            return Err(CyxCloudError::InvalidChunkId(format!(
                "Invalid length: expected 32, got {}",
                bytes.len()
            )));
        }

        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        Ok(Self(arr))
    }

    /// Convert to CyxCloud URI format
    pub fn to_uri(&self) -> String {
        format!("cyx://{}", self.to_base58())
    }

    /// Parse from CyxCloud URI format
    pub fn from_uri(uri: &str) -> Result<Self> {
        let base58 = uri
            .strip_prefix("cyx://")
            .ok_or_else(|| CyxCloudError::InvalidChunkId(
                "URI must start with cyx://".to_string()
            ))?;
        Self::from_base58(base58)
    }
}

impl fmt::Debug for ChunkId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ChunkId({})", &self.to_base58()[..8])
    }
}

impl fmt::Display for ChunkId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_base58())
    }
}

/// Chunk metadata stored alongside chunk data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkMetadata {
    /// Content-addressed identifier
    pub id: ChunkId,

    /// Size of the chunk data in bytes
    pub size: u64,

    /// Index within the parent file (for ordered reconstruction)
    pub index: u32,

    /// Total number of chunks in the parent file
    pub total_chunks: u32,

    /// Parent file/dataset this chunk belongs to
    pub parent_id: Option<Uuid>,

    /// Unix timestamp when chunk was created
    pub created_at: i64,

    /// Whether this chunk is encrypted
    pub encrypted: bool,

    /// Erasure coding shard index (0-13 for our config)
    /// None if this is the original chunk before sharding
    pub shard_index: Option<u8>,
}

impl ChunkMetadata {
    /// Create metadata for a new chunk
    pub fn new(id: ChunkId, size: u64, index: u32, total_chunks: u32) -> Self {
        Self {
            id,
            size,
            index,
            total_chunks,
            parent_id: None,
            created_at: chrono::Utc::now().timestamp(),
            encrypted: false,
            shard_index: None,
        }
    }

    /// Set the parent file ID
    pub fn with_parent(mut self, parent_id: Uuid) -> Self {
        self.parent_id = Some(parent_id);
        self
    }

    /// Mark as encrypted
    pub fn with_encryption(mut self) -> Self {
        self.encrypted = true;
        self
    }

    /// Set shard index for erasure-coded chunks
    pub fn with_shard_index(mut self, index: u8) -> Self {
        self.shard_index = Some(index);
        self
    }
}

/// A chunk of data with its metadata
#[derive(Debug, Clone)]
pub struct Chunk {
    /// Chunk metadata
    pub metadata: ChunkMetadata,

    /// Raw chunk data
    pub data: Bytes,
}

impl Chunk {
    /// Create a new chunk from data
    pub fn new(data: impl Into<Bytes>, index: u32, total_chunks: u32) -> Result<Self> {
        let data: Bytes = data.into();

        // Validate size
        if data.len() > MAX_CHUNK_SIZE {
            return Err(CyxCloudError::ChunkTooLarge {
                size: data.len(),
                max: MAX_CHUNK_SIZE,
            });
        }

        // Compute content-addressed ID
        let id = ChunkId::from_data(&data);
        let metadata = ChunkMetadata::new(id, data.len() as u64, index, total_chunks);

        Ok(Self { metadata, data })
    }

    /// Verify the chunk's integrity by recomputing its hash
    pub fn verify(&self) -> bool {
        let computed_id = ChunkId::from_data(&self.data);
        computed_id == self.metadata.id
    }

    /// Get the chunk ID
    pub fn id(&self) -> ChunkId {
        self.metadata.id
    }

    /// Get the chunk size
    pub fn size(&self) -> usize {
        self.data.len()
    }
}

/// Split a file into chunks
pub fn split_into_chunks(
    data: &[u8],
    chunk_size: usize,
    parent_id: Option<Uuid>,
) -> Result<Vec<Chunk>> {
    let chunk_size = chunk_size.clamp(MIN_CHUNK_SIZE, MAX_CHUNK_SIZE);
    let total_chunks = (data.len() + chunk_size - 1) / chunk_size;

    let chunks: Vec<Chunk> = data
        .chunks(chunk_size)
        .enumerate()
        .map(|(index, chunk_data)| {
            let mut chunk = Chunk::new(
                Bytes::copy_from_slice(chunk_data),
                index as u32,
                total_chunks as u32,
            )?;

            if let Some(pid) = parent_id {
                chunk.metadata = chunk.metadata.with_parent(pid);
            }

            Ok(chunk)
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(chunks)
}

/// Reassemble chunks into original data
pub fn reassemble_chunks(chunks: &[Chunk]) -> Result<Bytes> {
    if chunks.is_empty() {
        return Ok(Bytes::new());
    }

    // Sort by index
    let mut sorted: Vec<&Chunk> = chunks.iter().collect();
    sorted.sort_by_key(|c| c.metadata.index);

    // Verify we have all chunks
    let expected_total = sorted[0].metadata.total_chunks as usize;
    if sorted.len() != expected_total {
        return Err(CyxCloudError::InsufficientShards {
            available: sorted.len(),
            required: expected_total,
        });
    }

    // Verify indices are contiguous
    for (i, chunk) in sorted.iter().enumerate() {
        if chunk.metadata.index != i as u32 {
            return Err(CyxCloudError::InvalidShardIndex {
                index: chunk.metadata.index as usize,
                max: expected_total - 1,
            });
        }
    }

    // Concatenate data
    let total_size: usize = sorted.iter().map(|c| c.data.len()).sum();
    let mut result = Vec::with_capacity(total_size);
    for chunk in sorted {
        result.extend_from_slice(&chunk.data);
    }

    Ok(Bytes::from(result))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunk_id_roundtrip() {
        let data = b"hello world";
        let id = ChunkId::from_data(data);

        // Base58 roundtrip
        let base58 = id.to_base58();
        let recovered = ChunkId::from_base58(&base58).unwrap();
        assert_eq!(id, recovered);

        // URI roundtrip
        let uri = id.to_uri();
        assert!(uri.starts_with("cyx://"));
        let recovered = ChunkId::from_uri(&uri).unwrap();
        assert_eq!(id, recovered);
    }

    #[test]
    fn test_chunk_verification() {
        let chunk = Chunk::new(
            Bytes::from_static(b"test data"),
            0,
            1,
        ).unwrap();

        assert!(chunk.verify());
    }

    #[test]
    fn test_split_and_reassemble() {
        let original = vec![0u8; 1024 * 1024]; // 1 MB
        let chunk_size = 256 * 1024; // 256 KB

        let chunks = split_into_chunks(&original, chunk_size, None).unwrap();
        assert_eq!(chunks.len(), 4); // 1MB / 256KB = 4 chunks

        let reassembled = reassemble_chunks(&chunks).unwrap();
        assert_eq!(reassembled.as_ref(), original.as_slice());
    }

    #[test]
    fn test_chunk_too_large() {
        let data = vec![0u8; MAX_CHUNK_SIZE + 1];
        let result = Chunk::new(Bytes::from(data), 0, 1);
        assert!(matches!(result, Err(CyxCloudError::ChunkTooLarge { .. })));
    }
}
