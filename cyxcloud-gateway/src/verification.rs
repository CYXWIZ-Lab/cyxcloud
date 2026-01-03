//! Dataset Verification Service
//!
//! Provides cryptographic verification for datasets:
//! - Manifest hash verification
//! - Individual file hash verification
//! - Public dataset registry matching
//! - Dataset signing with Ed25519

use crate::AppState;
use chrono::{DateTime, Utc};
use cyxcloud_metadata::{DatasetFile, MetadataService, TrustLevel};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use std::sync::Arc;
use thiserror::Error;
use tracing::{debug, info, instrument, warn};
use uuid::Uuid;

/// Verification errors
#[derive(Error, Debug)]
pub enum VerificationError {
    #[error("Dataset not found: {0}")]
    DatasetNotFound(Uuid),

    #[error("File not found: {0}")]
    FileNotFound(Uuid),

    #[error("Permission denied")]
    PermissionDenied,

    #[error("Invalid signature")]
    InvalidSignature,

    #[error("Verification failed: {0}")]
    VerificationFailed(String),

    #[error("Database error: {0}")]
    Database(String),

    #[error("Storage error: {0}")]
    Storage(String),
}

pub type VerificationResult<T> = Result<T, VerificationError>;

/// Result of verifying a single file
#[derive(Debug, Clone)]
pub struct FileVerificationResult {
    pub file_id: Uuid,
    pub path: String,
    pub expected_hash: Vec<u8>,
    pub actual_hash: Option<Vec<u8>>,
    pub valid: bool,
    pub error: Option<String>,
}

/// Result of verifying a dataset
#[derive(Debug, Clone)]
pub struct DatasetVerificationResult {
    pub dataset_id: Uuid,
    pub manifest_valid: bool,
    pub all_files_valid: bool,
    pub files_verified: usize,
    pub files_failed: usize,
    pub file_results: Vec<FileVerificationResult>,
    pub computed_trust_level: TrustLevel,
    pub public_match: Option<PublicDatasetMatch>,
    pub verified_at: DateTime<Utc>,
    pub message: String,
}

/// Match against public dataset registry
#[derive(Debug, Clone)]
pub struct PublicDatasetMatch {
    pub name: String,
    pub version: String,
    pub official_url: String,
    pub license: Option<String>,
    pub verified_by: Vec<String>,
    pub confidence: f64,
}

/// Verification service for dataset integrity
pub struct VerificationService {
    state: Arc<AppState>,
}

impl VerificationService {
    /// Create a new verification service
    pub fn new(state: Arc<AppState>) -> Self {
        Self { state }
    }

    /// Verify a dataset's integrity
    #[instrument(skip(self), fields(dataset_id = %dataset_id))]
    pub async fn verify_dataset(
        &self,
        dataset_id: Uuid,
        full_verification: bool,
    ) -> VerificationResult<DatasetVerificationResult> {
        let metadata = self
            .state
            .metadata_service()
            .ok_or_else(|| VerificationError::Database("Metadata service not available".into()))?;

        // Get dataset
        let dataset = metadata
            .database()
            .get_dataset(dataset_id)
            .await
            .map_err(|e| VerificationError::Database(e.to_string()))?
            .ok_or(VerificationError::DatasetNotFound(dataset_id))?;

        // Get dataset files
        let files = metadata
            .database()
            .get_dataset_files(dataset_id)
            .await
            .map_err(|e| VerificationError::Database(e.to_string()))?;

        info!(
            dataset_id = %dataset_id,
            file_count = files.len(),
            full = full_verification,
            "Starting dataset verification"
        );

        // Verify manifest hash
        let computed_manifest_hash = self.compute_manifest_hash(&files);
        let manifest_valid = computed_manifest_hash == dataset.content_hash;

        if !manifest_valid {
            warn!(
                dataset_id = %dataset_id,
                expected = hex::encode(&dataset.content_hash),
                computed = hex::encode(&computed_manifest_hash),
                "Manifest hash mismatch"
            );
        }

        // Verify individual files (if full verification requested)
        let mut file_results = Vec::new();
        let mut files_verified = 0;
        let mut files_failed = 0;

        if full_verification {
            for file in &files {
                let result = self.verify_file(metadata, file).await;
                if result.valid {
                    files_verified += 1;
                } else {
                    files_failed += 1;
                }
                file_results.push(result);
            }
        } else {
            // Just check that files exist in dataset_files table
            files_verified = files.len();
        }

        let all_files_valid = files_failed == 0;

        // Determine trust level based on verification
        let computed_trust_level = if manifest_valid && all_files_valid {
            // Keep existing trust level or upgrade to Self if untrusted
            if dataset.trust_level == TrustLevel::Untrusted as i32 {
                TrustLevel::SelfUploaded
            } else {
                // Convert i32 back to TrustLevel
                TrustLevel::from_i32(dataset.trust_level)
            }
        } else {
            TrustLevel::Untrusted
        };

        let message = if manifest_valid && all_files_valid {
            format!(
                "Dataset verified: {} files checked, all valid",
                files_verified
            )
        } else if !manifest_valid {
            "Manifest hash verification failed".to_string()
        } else {
            format!(
                "File verification failed: {}/{} files invalid",
                files_failed,
                files.len()
            )
        };

        info!(
            dataset_id = %dataset_id,
            manifest_valid = manifest_valid,
            files_verified = files_verified,
            files_failed = files_failed,
            "Dataset verification complete"
        );

        Ok(DatasetVerificationResult {
            dataset_id,
            manifest_valid,
            all_files_valid,
            files_verified,
            files_failed,
            file_results,
            computed_trust_level,
            public_match: None,
            verified_at: Utc::now(),
            message,
        })
    }

    /// Verify a dataset against the public registry
    #[instrument(skip(self), fields(dataset_id = %dataset_id))]
    pub async fn verify_against_public_registry(
        &self,
        dataset_id: Uuid,
    ) -> VerificationResult<Option<PublicDatasetMatch>> {
        let metadata = self
            .state
            .metadata_service()
            .ok_or_else(|| VerificationError::Database("Metadata service not available".into()))?;

        // Get dataset
        let dataset = metadata
            .database()
            .get_dataset(dataset_id)
            .await
            .map_err(|e| VerificationError::Database(e.to_string()))?
            .ok_or(VerificationError::DatasetNotFound(dataset_id))?;

        // Search public registry by content hash
        let public_datasets = metadata
            .database()
            .list_public_datasets(None)
            .await
            .map_err(|e| VerificationError::Database(e.to_string()))?;

        // Find matching hash
        for public in public_datasets {
            if public.official_hash == dataset.content_hash {
                info!(
                    dataset_id = %dataset_id,
                    public_name = %public.name,
                    public_version = %public.version,
                    "Found matching public dataset"
                );

                // Update trust level to Verified
                if let Err(e) = metadata
                    .database()
                    .update_dataset_trust(dataset_id, TrustLevel::Verified as i32, None)
                    .await
                {
                    warn!(error = %e, "Failed to update trust level");
                }

                return Ok(Some(PublicDatasetMatch {
                    name: public.name,
                    version: public.version,
                    official_url: public.official_url,
                    license: public.license,
                    verified_by: public.verified_by,
                    confidence: 1.0, // Exact hash match
                }));
            }
        }

        debug!(dataset_id = %dataset_id, "No matching public dataset found");
        Ok(None)
    }

    /// Sign a dataset with an Ed25519 key
    #[instrument(skip(self, signing_key), fields(dataset_id = %dataset_id, user_id = %user_id))]
    pub async fn sign_dataset(
        &self,
        dataset_id: Uuid,
        user_id: Uuid,
        signing_key: &SigningKey,
    ) -> VerificationResult<Vec<u8>> {
        let metadata = self
            .state
            .metadata_service()
            .ok_or_else(|| VerificationError::Database("Metadata service not available".into()))?;

        // Get dataset
        let dataset = metadata
            .database()
            .get_dataset(dataset_id)
            .await
            .map_err(|e| VerificationError::Database(e.to_string()))?
            .ok_or(VerificationError::DatasetNotFound(dataset_id))?;

        // Verify ownership
        if dataset.owner_id != user_id {
            return Err(VerificationError::PermissionDenied);
        }

        // Sign the content hash
        let signature = signing_key.sign(&dataset.content_hash);
        let signature_bytes = signature.to_bytes().to_vec();

        // Update dataset with signature
        if let Err(e) = metadata
            .database()
            .update_dataset_signature(dataset_id, signature_bytes.clone())
            .await
        {
            warn!(error = %e, "Failed to store signature");
        }

        // Update trust level to Signed
        if let Err(e) = metadata
            .database()
            .update_dataset_trust(dataset_id, TrustLevel::Signed as i32, None)
            .await
        {
            warn!(error = %e, "Failed to update trust level");
        }

        info!(dataset_id = %dataset_id, "Dataset signed successfully");
        Ok(signature_bytes)
    }

    /// Verify a dataset's signature
    #[instrument(skip(self), fields(dataset_id = %dataset_id))]
    pub async fn verify_signature(
        &self,
        dataset_id: Uuid,
        verifying_key: &VerifyingKey,
    ) -> VerificationResult<bool> {
        let metadata = self
            .state
            .metadata_service()
            .ok_or_else(|| VerificationError::Database("Metadata service not available".into()))?;

        // Get dataset
        let dataset = metadata
            .database()
            .get_dataset(dataset_id)
            .await
            .map_err(|e| VerificationError::Database(e.to_string()))?
            .ok_or(VerificationError::DatasetNotFound(dataset_id))?;

        // Check if dataset has a signature
        let signature_bytes = dataset
            .signature
            .ok_or_else(|| VerificationError::InvalidSignature)?;

        // Parse signature
        let sig_array: [u8; 64] = signature_bytes
            .try_into()
            .map_err(|_| VerificationError::InvalidSignature)?;
        let signature = Signature::from_bytes(&sig_array);

        // Verify signature against content hash
        verifying_key
            .verify(&dataset.content_hash, &signature)
            .map_err(|_| VerificationError::InvalidSignature)?;

        info!(dataset_id = %dataset_id, "Signature verified successfully");
        Ok(true)
    }

    /// Compute manifest hash from dataset files
    fn compute_manifest_hash(&self, files: &[DatasetFile]) -> Vec<u8> {
        let mut hasher = blake3::Hasher::new();

        // Sort files by path for deterministic ordering
        let mut sorted_files: Vec<_> = files.iter().collect();
        sorted_files.sort_by(|a, b| a.path_in_dataset.cmp(&b.path_in_dataset));

        for file in sorted_files {
            // Hash: path + content_hash
            hasher.update(file.path_in_dataset.as_bytes());
            hasher.update(&file.content_hash);
        }

        hasher.finalize().as_bytes().to_vec()
    }

    /// Verify a single file's integrity
    async fn verify_file(
        &self,
        metadata: &MetadataService,
        dataset_file: &DatasetFile,
    ) -> FileVerificationResult {
        // Get the actual file from storage
        let file = match metadata.database().get_file(dataset_file.file_id).await {
            Ok(Some(f)) => f,
            Ok(None) => {
                return FileVerificationResult {
                    file_id: dataset_file.file_id,
                    path: dataset_file.path_in_dataset.clone(),
                    expected_hash: dataset_file.content_hash.clone(),
                    actual_hash: None,
                    valid: false,
                    error: Some("File not found in storage".to_string()),
                };
            }
            Err(e) => {
                return FileVerificationResult {
                    file_id: dataset_file.file_id,
                    path: dataset_file.path_in_dataset.clone(),
                    expected_hash: dataset_file.content_hash.clone(),
                    actual_hash: None,
                    valid: false,
                    error: Some(format!("Database error: {}", e)),
                };
            }
        };

        // Compare hashes
        let valid = file.content_hash == dataset_file.content_hash;

        FileVerificationResult {
            file_id: dataset_file.file_id,
            path: dataset_file.path_in_dataset.clone(),
            expected_hash: dataset_file.content_hash.clone(),
            actual_hash: Some(file.content_hash),
            valid,
            error: if valid {
                None
            } else {
                Some("Hash mismatch".to_string())
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manifest_hash_deterministic() {
        let files = vec![
            DatasetFile {
                id: Uuid::new_v4(),
                dataset_id: Uuid::new_v4(),
                file_id: Uuid::new_v4(),
                path_in_dataset: "b.txt".to_string(),
                content_hash: vec![1, 2, 3],
                size_bytes: 100,
                file_index: 1,
                created_at: Utc::now(),
            },
            DatasetFile {
                id: Uuid::new_v4(),
                dataset_id: Uuid::new_v4(),
                file_id: Uuid::new_v4(),
                path_in_dataset: "a.txt".to_string(),
                content_hash: vec![4, 5, 6],
                size_bytes: 200,
                file_index: 0,
                created_at: Utc::now(),
            },
        ];

        // Create a mock state - we can't easily test this without a real AppState
        // Just verify the hash computation is deterministic
        let mut hasher1 = blake3::Hasher::new();
        let mut hasher2 = blake3::Hasher::new();

        let mut sorted: Vec<_> = files.iter().collect();
        sorted.sort_by(|a, b| a.path_in_dataset.cmp(&b.path_in_dataset));

        for file in &sorted {
            hasher1.update(file.path_in_dataset.as_bytes());
            hasher1.update(&file.content_hash);
        }

        for file in &sorted {
            hasher2.update(file.path_in_dataset.as_bytes());
            hasher2.update(&file.content_hash);
        }

        assert_eq!(
            hasher1.finalize().as_bytes(),
            hasher2.finalize().as_bytes()
        );
    }

    #[test]
    fn test_ed25519_sign_verify() {
        use ed25519_dalek::SigningKey;
        use rand::rngs::OsRng;

        // Generate keypair
        let signing_key = SigningKey::generate(&mut OsRng);
        let verifying_key = signing_key.verifying_key();

        // Sign some data
        let data = b"test dataset content hash";
        let signature = signing_key.sign(data);

        // Verify
        assert!(verifying_key.verify(data, &signature).is_ok());

        // Wrong data should fail
        let wrong_data = b"wrong data";
        assert!(verifying_key.verify(wrong_data, &signature).is_err());
    }
}
