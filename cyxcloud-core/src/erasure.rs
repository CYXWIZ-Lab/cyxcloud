//! Reed-Solomon Erasure Coding
//!
//! Implements (k=10, m=4) erasure coding where:
//! - k=10 data shards (minimum required to reconstruct)
//! - m=4 parity shards (redundancy)
//! - Total 14 shards distributed across nodes
//! - Can tolerate loss of ANY 4 nodes

use crate::error::{CyxCloudError, Result};
use crate::{DATA_SHARDS, PARITY_SHARDS};
use bytes::Bytes;
use rayon::prelude::*;
use reed_solomon_erasure::galois_8::ReedSolomon;
use serde::{Deserialize, Serialize};

/// Erasure coding configuration
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ErasureConfig {
    /// Number of data shards (k)
    pub data_shards: usize,
    /// Number of parity shards (m)
    pub parity_shards: usize,
}

impl Default for ErasureConfig {
    fn default() -> Self {
        Self {
            data_shards: DATA_SHARDS,
            parity_shards: PARITY_SHARDS,
        }
    }
}

impl ErasureConfig {
    /// Create a new erasure config
    pub fn new(data_shards: usize, parity_shards: usize) -> Result<Self> {
        if data_shards == 0 {
            return Err(CyxCloudError::Configuration(
                "data_shards must be > 0".to_string(),
            ));
        }
        if parity_shards == 0 {
            return Err(CyxCloudError::Configuration(
                "parity_shards must be > 0".to_string(),
            ));
        }
        Ok(Self {
            data_shards,
            parity_shards,
        })
    }

    /// Total number of shards
    pub fn total_shards(&self) -> usize {
        self.data_shards + self.parity_shards
    }

    /// Storage overhead ratio (parity/data)
    pub fn overhead_ratio(&self) -> f64 {
        self.parity_shards as f64 / self.data_shards as f64
    }

    /// Maximum number of failures that can be tolerated
    pub fn max_failures(&self) -> usize {
        self.parity_shards
    }
}

/// A single shard of erasure-coded data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShardData {
    /// Shard index (0 to total_shards-1)
    pub index: u8,
    /// Shard data
    pub data: Bytes,
    /// Whether this is a parity shard
    pub is_parity: bool,
}

impl ShardData {
    /// Create a new shard
    pub fn new(index: u8, data: Bytes, is_parity: bool) -> Self {
        Self {
            index,
            data,
            is_parity,
        }
    }

    /// Get shard size
    pub fn size(&self) -> usize {
        self.data.len()
    }
}

/// Reed-Solomon encoder/decoder
pub struct ErasureEncoder {
    config: ErasureConfig,
    encoder: ReedSolomon,
}

impl ErasureEncoder {
    /// Create a new encoder with default configuration (10, 4)
    pub fn new() -> Result<Self> {
        Self::with_config(ErasureConfig::default())
    }

    /// Create a new encoder with custom configuration
    pub fn with_config(config: ErasureConfig) -> Result<Self> {
        let encoder = ReedSolomon::new(config.data_shards, config.parity_shards)?;
        Ok(Self { config, encoder })
    }

    /// Get the erasure configuration
    pub fn config(&self) -> &ErasureConfig {
        &self.config
    }

    /// Encode data into shards
    ///
    /// Returns a vector of shards (data + parity)
    pub fn encode(&self, data: &[u8]) -> Result<Vec<ShardData>> {
        let shard_size = self.calculate_shard_size(data.len());

        // Pad data to be evenly divisible by data_shards
        let padded_size = shard_size * self.config.data_shards;
        let mut padded_data = data.to_vec();
        padded_data.resize(padded_size, 0);

        // Split into data shards
        let mut shards: Vec<Vec<u8>> = padded_data.chunks(shard_size).map(|c| c.to_vec()).collect();

        // Add empty parity shards
        for _ in 0..self.config.parity_shards {
            shards.push(vec![0u8; shard_size]);
        }

        // Encode (fills in parity shards)
        self.encoder.encode(&mut shards)?;

        // Convert to ShardData
        let result: Vec<ShardData> = shards
            .into_iter()
            .enumerate()
            .map(|(i, shard_data)| {
                let is_parity = i >= self.config.data_shards;
                ShardData::new(i as u8, Bytes::from(shard_data), is_parity)
            })
            .collect();

        Ok(result)
    }

    /// Encode data into shards using parallel processing
    ///
    /// More efficient for large chunks (> 1MB)
    pub fn encode_parallel(&self, data: &[u8]) -> Result<Vec<ShardData>> {
        let shard_size = self.calculate_shard_size(data.len());

        // Pad data
        let padded_size = shard_size * self.config.data_shards;
        let mut padded_data = data.to_vec();
        padded_data.resize(padded_size, 0);

        // Split into data shards (parallel)
        let mut shards: Vec<Vec<u8>> = padded_data
            .par_chunks(shard_size)
            .map(|c| c.to_vec())
            .collect();

        // Add parity shards
        for _ in 0..self.config.parity_shards {
            shards.push(vec![0u8; shard_size]);
        }

        // Encode
        self.encoder.encode(&mut shards)?;

        // Convert to ShardData (parallel)
        let result: Vec<ShardData> = shards
            .into_par_iter()
            .enumerate()
            .map(|(i, shard_data)| {
                let is_parity = i >= self.config.data_shards;
                ShardData::new(i as u8, Bytes::from(shard_data), is_parity)
            })
            .collect();

        Ok(result)
    }

    /// Decode shards back into original data
    ///
    /// Requires at least `data_shards` number of shards.
    /// Missing shards should be represented as `None`.
    pub fn decode(&self, shards: &[Option<ShardData>], original_size: usize) -> Result<Bytes> {
        let total_shards = self.config.total_shards();

        if shards.len() != total_shards {
            return Err(CyxCloudError::ShardSizeMismatch {
                expected: total_shards,
                actual: shards.len(),
            });
        }

        // Count available shards
        let available = shards.iter().filter(|s| s.is_some()).count();
        if available < self.config.data_shards {
            return Err(CyxCloudError::InsufficientShards {
                available,
                required: self.config.data_shards,
            });
        }

        // Determine shard size from available shards
        let shard_size = shards
            .iter()
            .find_map(|s| s.as_ref().map(|s| s.size()))
            .ok_or(CyxCloudError::InsufficientShards {
                available: 0,
                required: self.config.data_shards,
            })?;

        // Convert to mutable shard vectors
        let mut shard_vecs: Vec<Option<Vec<u8>>> = shards
            .iter()
            .map(|opt| opt.as_ref().map(|s| s.data.to_vec()))
            .collect();

        // Reconstruct missing shards
        self.encoder.reconstruct(&mut shard_vecs)?;

        // Extract data shards and concatenate
        let mut result = Vec::with_capacity(shard_size * self.config.data_shards);
        for shard_opt in shard_vecs.iter().take(self.config.data_shards) {
            if let Some(ref shard) = shard_opt {
                result.extend_from_slice(shard);
            } else {
                return Err(CyxCloudError::Internal("Reconstruction failed".to_string()));
            }
        }

        // Trim to original size
        result.truncate(original_size);
        Ok(Bytes::from(result))
    }

    /// Calculate the size of each shard given the data size
    fn calculate_shard_size(&self, data_size: usize) -> usize {
        // Round up to ensure all data fits
        data_size.div_ceil(self.config.data_shards)
    }

    /// Verify that shards are consistent (for health checking)
    pub fn verify_shards(&self, shards: &[ShardData]) -> Result<bool> {
        if shards.len() != self.config.total_shards() {
            return Ok(false);
        }

        // Check all shards have same size
        let expected_size = shards.first().map(|s| s.size()).unwrap_or(0);
        if !shards.iter().all(|s| s.size() == expected_size) {
            return Ok(false);
        }

        // Convert to option refs for verification
        let shard_refs: Vec<&[u8]> = shards.iter().map(|s| s.data.as_ref()).collect();

        // Verify parity shards are correct
        Ok(self.encoder.verify(&shard_refs)?)
    }
}

impl Default for ErasureEncoder {
    fn default() -> Self {
        Self::new().expect("Default erasure config should always work")
    }
}

/// Convenience function to encode data with default configuration
pub fn encode(data: &[u8]) -> Result<Vec<ShardData>> {
    ErasureEncoder::new()?.encode(data)
}

/// Convenience function to decode shards with default configuration
pub fn decode(shards: &[Option<ShardData>], original_size: usize) -> Result<Bytes> {
    ErasureEncoder::new()?.decode(shards, original_size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_erasure_config() {
        let config = ErasureConfig::default();
        assert_eq!(config.data_shards, 10);
        assert_eq!(config.parity_shards, 4);
        assert_eq!(config.total_shards(), 14);
        assert_eq!(config.max_failures(), 4);
        assert!((config.overhead_ratio() - 0.4).abs() < 0.001);
    }

    #[test]
    fn test_encode_decode_simple() {
        let encoder = ErasureEncoder::new().unwrap();
        let original = b"Hello, CyxCloud!";

        // Encode
        let shards = encoder.encode(original).unwrap();
        assert_eq!(shards.len(), 14);

        // Decode with all shards
        let shard_opts: Vec<Option<ShardData>> = shards.into_iter().map(Some).collect();
        let decoded = encoder.decode(&shard_opts, original.len()).unwrap();
        assert_eq!(decoded.as_ref(), original);
    }

    #[test]
    fn test_encode_decode_with_missing_shards() {
        let encoder = ErasureEncoder::new().unwrap();
        let original = vec![0u8; 1024 * 1024]; // 1 MB

        // Encode
        let shards = encoder.encode(&original).unwrap();

        // Remove 4 shards (maximum we can lose)
        let mut shard_opts: Vec<Option<ShardData>> = shards.into_iter().map(Some).collect();
        shard_opts[0] = None; // Remove shard 0
        shard_opts[5] = None; // Remove shard 5
        shard_opts[10] = None; // Remove shard 10 (parity)
        shard_opts[13] = None; // Remove shard 13 (parity)

        // Should still decode successfully
        let decoded = encoder.decode(&shard_opts, original.len()).unwrap();
        assert_eq!(decoded.as_ref(), original.as_slice());
    }

    #[test]
    fn test_too_many_missing_shards() {
        let encoder = ErasureEncoder::new().unwrap();
        let original = b"test data";

        let shards = encoder.encode(original).unwrap();

        // Remove 5 shards (one more than maximum)
        let mut shard_opts: Vec<Option<ShardData>> = shards.into_iter().map(Some).collect();
        for i in 0..5 {
            shard_opts[i] = None;
        }

        // Should fail
        let result = encoder.decode(&shard_opts, original.len());
        assert!(matches!(
            result,
            Err(CyxCloudError::InsufficientShards { .. })
        ));
    }

    #[test]
    fn test_encode_parallel() {
        let encoder = ErasureEncoder::new().unwrap();
        let original = vec![42u8; 10 * 1024 * 1024]; // 10 MB

        // Both methods should produce identical results
        let shards_seq = encoder.encode(&original).unwrap();
        let shards_par = encoder.encode_parallel(&original).unwrap();

        assert_eq!(shards_seq.len(), shards_par.len());
        for (s1, s2) in shards_seq.iter().zip(shards_par.iter()) {
            assert_eq!(s1.data, s2.data);
            assert_eq!(s1.index, s2.index);
            assert_eq!(s1.is_parity, s2.is_parity);
        }
    }

    #[test]
    fn test_verify_shards() {
        let encoder = ErasureEncoder::new().unwrap();
        let original = b"verify test";

        let shards = encoder.encode(original).unwrap();
        assert!(encoder.verify_shards(&shards).unwrap());

        // Corrupt a shard
        let mut corrupted_shards = shards.clone();
        if !corrupted_shards[0].data.is_empty() {
            let mut data = corrupted_shards[0].data.to_vec();
            data[0] ^= 0xFF;
            corrupted_shards[0].data = Bytes::from(data);
        }
        assert!(!encoder.verify_shards(&corrupted_shards).unwrap());
    }

    #[test]
    fn test_custom_config() {
        // Smaller config for testing
        let config = ErasureConfig::new(3, 2).unwrap();
        let encoder = ErasureEncoder::with_config(config).unwrap();

        let original = b"small config test";
        let shards = encoder.encode(original).unwrap();
        assert_eq!(shards.len(), 5); // 3 data + 2 parity

        let shard_opts: Vec<Option<ShardData>> = shards.into_iter().map(Some).collect();
        let decoded = encoder.decode(&shard_opts, original.len()).unwrap();
        assert_eq!(decoded.as_ref(), original);
    }

    #[test]
    fn test_shard_indices() {
        let encoder = ErasureEncoder::new().unwrap();
        let shards = encoder.encode(b"index test").unwrap();

        for (i, shard) in shards.iter().enumerate() {
            assert_eq!(shard.index as usize, i);
            assert_eq!(shard.is_parity, i >= 10);
        }
    }
}
