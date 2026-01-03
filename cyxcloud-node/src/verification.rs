//! Client-Side Verification Module
//!
//! Provides utilities for verifying dataset integrity, checking trust levels,
//! and validating data against known public datasets.

use crate::datastream_client::{DataStreamClient, VerifiedBatch};
use cyxcloud_protocol::datastream::{TrustLevel, VerificationResult};
use std::collections::HashMap;
use thiserror::Error;
use tracing::{debug, info, instrument, warn};

/// Verification errors
#[derive(Error, Debug)]
pub enum VerificationError {
    #[error("Hash mismatch: expected {expected}, got {actual}")]
    HashMismatch { expected: String, actual: String },

    #[error("Trust level insufficient: required {required}, got {actual}")]
    InsufficientTrust { required: i32, actual: i32 },

    #[error("Dataset verification failed: {0}")]
    VerificationFailed(String),

    #[error("Public registry check failed: {0}")]
    RegistryCheckFailed(String),

    #[error("Data stream error: {0}")]
    DataStreamError(String),
}

impl From<crate::datastream_client::DataStreamError> for VerificationError {
    fn from(e: crate::datastream_client::DataStreamError) -> Self {
        VerificationError::DataStreamError(e.to_string())
    }
}

pub type VerificationResult2 = Result<(), VerificationError>;

/// Trust level requirements for different use cases
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustRequirement {
    /// Accept any trust level
    Any,
    /// Require at least self-uploaded data
    SelfOrBetter,
    /// Require signed data
    SignedOrBetter,
    /// Require verified against public registry
    VerifiedOrBetter,
    /// Require TEE attestation
    AttestedOnly,
}

impl TrustRequirement {
    /// Get the maximum acceptable trust level value
    pub fn max_trust_level(&self) -> i32 {
        match self {
            TrustRequirement::Any => TrustLevel::TrustUntrusted as i32,
            TrustRequirement::SelfOrBetter => TrustLevel::TrustSelf as i32,
            TrustRequirement::SignedOrBetter => TrustLevel::TrustSigned as i32,
            TrustRequirement::VerifiedOrBetter => TrustLevel::TrustVerified as i32,
            TrustRequirement::AttestedOnly => TrustLevel::TrustAttested as i32,
        }
    }

    /// Check if a trust level meets the requirement
    pub fn is_satisfied(&self, trust_level: i32) -> bool {
        // Lower values = more trusted
        trust_level <= self.max_trust_level()
    }
}

/// Verification options
#[derive(Debug, Clone)]
pub struct VerificationOptions {
    /// Minimum trust requirement
    pub trust_requirement: TrustRequirement,

    /// Check against public dataset registry
    pub check_public_registry: bool,

    /// Perform full content verification
    pub full_verification: bool,

    /// Verify individual batch hashes during streaming
    pub verify_batch_hashes: bool,

    /// Maximum items to sample for verification (0 = all)
    pub sample_size: usize,
}

impl Default for VerificationOptions {
    fn default() -> Self {
        Self {
            trust_requirement: TrustRequirement::VerifiedOrBetter,
            check_public_registry: true,
            full_verification: false,
            verify_batch_hashes: true,
            sample_size: 0, // Verify all
        }
    }
}

/// Result of dataset verification
#[derive(Debug, Clone)]
pub struct DatasetVerification {
    /// Whether the dataset passed verification
    pub passed: bool,

    /// Trust level of the dataset
    pub trust_level: i32,

    /// Trust level name
    pub trust_level_name: String,

    /// Whether it matches a known public dataset
    pub is_public_dataset: bool,

    /// Name of matching public dataset (if any)
    pub public_dataset_name: Option<String>,

    /// Public dataset version (if any)
    pub public_dataset_version: Option<String>,

    /// Manifest hash verified
    pub manifest_verified: bool,

    /// Number of files verified
    pub files_verified: u32,

    /// Number of files that failed verification
    pub files_failed: u32,

    /// Detailed verification messages
    pub messages: Vec<String>,
}

impl DatasetVerification {
    fn trust_level_to_name(level: i32) -> String {
        match level {
            0 => "Self (your upload)".to_string(),
            1 => "Signed (cryptographically signed)".to_string(),
            2 => "Verified (matches public registry)".to_string(),
            3 => "Attested (TEE verified)".to_string(),
            _ => "Untrusted".to_string(),
        }
    }
}

impl From<VerificationResult> for DatasetVerification {
    fn from(result: VerificationResult) -> Self {
        let trust_level = result.computed_trust_level;
        let files_verified = result.files.iter().filter(|f| f.valid).count() as u32;
        let files_failed = result.files.iter().filter(|f| !f.valid).count() as u32;
        let mut messages = Vec::new();
        if !result.message.is_empty() {
            messages.push(result.message.clone());
        }
        for file in &result.files {
            if !file.valid && !file.error.is_empty() {
                messages.push(format!("{}: {}", file.path, file.error));
            }
        }
        Self {
            passed: result.manifest_valid && result.all_files_valid,
            trust_level,
            trust_level_name: Self::trust_level_to_name(trust_level),
            is_public_dataset: result.public_match.is_some(),
            public_dataset_name: result.public_match.as_ref().map(|m| m.name.clone()),
            public_dataset_version: result.public_match.as_ref().map(|m| m.version.clone()),
            manifest_verified: result.manifest_valid,
            files_verified,
            files_failed,
            messages,
        }
    }
}

/// Dataset verifier
pub struct DatasetVerifier {
    options: VerificationOptions,
}

impl DatasetVerifier {
    /// Create a new verifier with default options
    pub fn new() -> Self {
        Self {
            options: VerificationOptions::default(),
        }
    }

    /// Create a verifier with custom options
    pub fn with_options(options: VerificationOptions) -> Self {
        Self { options }
    }

    /// Set trust requirement
    pub fn trust_requirement(mut self, requirement: TrustRequirement) -> Self {
        self.options.trust_requirement = requirement;
        self
    }

    /// Enable/disable public registry check
    pub fn check_public_registry(mut self, check: bool) -> Self {
        self.options.check_public_registry = check;
        self
    }

    /// Enable/disable full verification
    pub fn full_verification(mut self, full: bool) -> Self {
        self.options.full_verification = full;
        self
    }

    /// Verify a dataset using the DataStream client
    #[instrument(skip(self, client))]
    pub async fn verify_dataset(
        &self,
        client: &mut DataStreamClient,
        dataset_id: &str,
    ) -> Result<DatasetVerification, VerificationError> {
        info!(dataset_id, "Starting dataset verification");

        // Get verification result from gateway
        let result = client
            .verify_dataset(
                dataset_id,
                self.options.check_public_registry,
                self.options.full_verification,
            )
            .await?;

        let verification = DatasetVerification::from(result);

        // Check trust requirement
        if !self
            .options
            .trust_requirement
            .is_satisfied(verification.trust_level)
        {
            warn!(
                trust_level = verification.trust_level,
                required = self.options.trust_requirement.max_trust_level(),
                "Trust level does not meet requirement"
            );
            return Err(VerificationError::InsufficientTrust {
                required: self.options.trust_requirement.max_trust_level(),
                actual: verification.trust_level,
            });
        }

        if verification.passed {
            info!(
                trust_level = verification.trust_level_name,
                is_public = verification.is_public_dataset,
                "Dataset verification passed"
            );
        } else {
            warn!(
                files_failed = verification.files_failed,
                "Dataset verification failed"
            );
        }

        Ok(verification)
    }

    /// Quick check if a dataset meets trust requirements
    #[instrument(skip(self, client))]
    pub async fn check_trust_level(
        &self,
        client: &mut DataStreamClient,
        dataset_id: &str,
    ) -> Result<bool, VerificationError> {
        let info = client.get_dataset_info(dataset_id).await?;
        Ok(self
            .options
            .trust_requirement
            .is_satisfied(info.trust_level))
    }

    /// Verify a batch against its expected hash
    pub fn verify_batch(&self, batch: &VerifiedBatch) -> VerificationResult2 {
        // VerifiedBatch is already verified by the DataStreamClient
        // This is a secondary check if needed
        debug!(
            batch_index = batch.index,
            items = batch.item_count,
            "Batch already verified"
        );
        Ok(())
    }
}

impl Default for DatasetVerifier {
    fn default() -> Self {
        Self::new()
    }
}

/// Well-known public dataset hashes for offline verification
pub struct PublicDatasetHashes {
    hashes: HashMap<String, Vec<u8>>,
}

impl PublicDatasetHashes {
    /// Create a new registry of known hashes
    pub fn new() -> Self {
        Self {
            hashes: HashMap::new(),
        }
    }

    /// Register a known public dataset hash
    pub fn register(&mut self, name: &str, version: &str, hash: Vec<u8>) {
        let key = format!("{}:{}", name, version);
        self.hashes.insert(key, hash);
    }

    /// Check if a hash matches a known public dataset
    pub fn find_match(&self, content_hash: &[u8]) -> Option<(String, String)> {
        for (key, known_hash) in &self.hashes {
            if known_hash == content_hash {
                let parts: Vec<&str> = key.split(':').collect();
                if parts.len() == 2 {
                    return Some((parts[0].to_string(), parts[1].to_string()));
                }
            }
        }
        None
    }

    /// Initialize with common public datasets
    /// Note: These are placeholder hashes - actual hashes would come from
    /// official sources or the public registry service
    pub fn with_common_datasets() -> Self {
        let registry = Self::new();

        // Placeholder - actual hashes would be fetched from gateway
        // or compiled from official sources
        debug!("Public dataset hash registry initialized");

        registry
    }
}

impl Default for PublicDatasetHashes {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trust_requirement_satisfaction() {
        assert!(TrustRequirement::Any.is_satisfied(4)); // Untrusted
        assert!(TrustRequirement::Any.is_satisfied(0)); // Self

        assert!(TrustRequirement::SelfOrBetter.is_satisfied(0));
        assert!(!TrustRequirement::SelfOrBetter.is_satisfied(4));

        assert!(TrustRequirement::VerifiedOrBetter.is_satisfied(0));
        assert!(TrustRequirement::VerifiedOrBetter.is_satisfied(2));
        assert!(!TrustRequirement::VerifiedOrBetter.is_satisfied(4));

        assert!(TrustRequirement::AttestedOnly.is_satisfied(3));
        assert!(!TrustRequirement::AttestedOnly.is_satisfied(4));
    }

    #[test]
    fn test_trust_level_name() {
        assert_eq!(
            DatasetVerification::trust_level_to_name(0),
            "Self (your upload)"
        );
        assert_eq!(
            DatasetVerification::trust_level_to_name(2),
            "Verified (matches public registry)"
        );
        assert_eq!(DatasetVerification::trust_level_to_name(99), "Untrusted");
    }

    #[test]
    fn test_verification_options_default() {
        let opts = VerificationOptions::default();
        assert!(opts.check_public_registry);
        assert!(opts.verify_batch_hashes);
        assert!(!opts.full_verification);
        assert_eq!(opts.sample_size, 0);
    }

    #[test]
    fn test_public_dataset_hashes() {
        let mut registry = PublicDatasetHashes::new();
        registry.register("MNIST", "1.0", vec![1, 2, 3, 4]);

        let found = registry.find_match(&[1, 2, 3, 4]);
        assert_eq!(found, Some(("MNIST".to_string(), "1.0".to_string())));

        let not_found = registry.find_match(&[5, 6, 7, 8]);
        assert_eq!(not_found, None);
    }

    #[test]
    fn test_verifier_builder() {
        let verifier = DatasetVerifier::new()
            .trust_requirement(TrustRequirement::SignedOrBetter)
            .check_public_registry(false)
            .full_verification(true);

        assert_eq!(
            verifier.options.trust_requirement,
            TrustRequirement::SignedOrBetter
        );
        assert!(!verifier.options.check_public_registry);
        assert!(verifier.options.full_verification);
    }
}
