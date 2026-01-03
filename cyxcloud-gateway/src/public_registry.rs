//! Public Dataset Registry Service
//!
//! Manages well-known public datasets (MNIST, CIFAR, ImageNet, etc.)
//! and provides hash-based verification to upgrade trust levels.

use crate::AppState;
use cyxcloud_metadata::{PublicDataset, TrustLevel};
use std::sync::Arc;
use thiserror::Error;
use tracing::{debug, info, instrument, warn};
use uuid::Uuid;

/// Public registry errors
#[derive(Error, Debug)]
pub enum RegistryError {
    #[error("Dataset not found: {0}")]
    DatasetNotFound(String),

    #[error("Public dataset not found: {0}")]
    PublicDatasetNotFound(String),

    #[error("Database error: {0}")]
    Database(String),

    #[error("Hash mismatch")]
    HashMismatch,
}

pub type RegistryResult<T> = Result<T, RegistryError>;

/// Information about a matched public dataset
#[derive(Debug, Clone)]
pub struct PublicDatasetMatch {
    pub id: Uuid,
    pub name: String,
    pub version: String,
    pub official_url: String,
    pub paper_url: Option<String>,
    pub license: Option<String>,
    pub verified_by: Vec<String>,
    pub confidence: f64,
    pub cached: bool,
    pub cached_dataset_id: Option<Uuid>,
}

impl From<PublicDataset> for PublicDatasetMatch {
    fn from(pd: PublicDataset) -> Self {
        Self {
            id: pd.id,
            name: pd.name,
            version: pd.version,
            official_url: pd.official_url,
            paper_url: pd.paper_url,
            license: pd.license,
            verified_by: pd.verified_by,
            confidence: 1.0, // Exact hash match
            cached: pd.cached_dataset_id.is_some(),
            cached_dataset_id: pd.cached_dataset_id,
        }
    }
}

/// Summary of a public dataset for listing
#[derive(Debug, Clone)]
pub struct PublicDatasetSummary {
    pub id: Uuid,
    pub name: String,
    pub version: String,
    pub license: Option<String>,
    pub verified_by: Vec<String>,
    pub cached: bool,
}

impl From<PublicDataset> for PublicDatasetSummary {
    fn from(pd: PublicDataset) -> Self {
        Self {
            id: pd.id,
            name: pd.name,
            version: pd.version,
            license: pd.license,
            verified_by: pd.verified_by,
            cached: pd.cached_dataset_id.is_some(),
        }
    }
}

/// Public Dataset Registry Service
pub struct PublicDatasetRegistry {
    state: Arc<AppState>,
}

impl PublicDatasetRegistry {
    /// Create a new registry service
    pub fn new(state: Arc<AppState>) -> Self {
        Self { state }
    }

    /// List all public datasets
    #[instrument(skip(self))]
    pub async fn list_public_datasets(
        &self,
        name_filter: Option<&str>,
    ) -> RegistryResult<Vec<PublicDatasetSummary>> {
        let metadata = self
            .state
            .metadata_service()
            .ok_or_else(|| RegistryError::Database("Metadata service not available".into()))?;

        let datasets = metadata
            .database()
            .list_public_datasets(name_filter)
            .await
            .map_err(|e| RegistryError::Database(e.to_string()))?;

        Ok(datasets.into_iter().map(PublicDatasetSummary::from).collect())
    }

    /// Get a public dataset by name and version
    #[instrument(skip(self))]
    pub async fn get_public_dataset(
        &self,
        name: &str,
        version: &str,
    ) -> RegistryResult<PublicDatasetMatch> {
        let metadata = self
            .state
            .metadata_service()
            .ok_or_else(|| RegistryError::Database("Metadata service not available".into()))?;

        let dataset = metadata
            .database()
            .get_public_dataset(name, version)
            .await
            .map_err(|e| RegistryError::Database(e.to_string()))?
            .ok_or_else(|| RegistryError::PublicDatasetNotFound(format!("{} v{}", name, version)))?;

        Ok(PublicDatasetMatch::from(dataset))
    }

    /// Find a public dataset by content hash
    #[instrument(skip(self, content_hash))]
    pub async fn find_by_hash(&self, content_hash: &[u8]) -> RegistryResult<Option<PublicDatasetMatch>> {
        let metadata = self
            .state
            .metadata_service()
            .ok_or_else(|| RegistryError::Database("Metadata service not available".into()))?;

        let dataset = metadata
            .database()
            .find_public_dataset_by_hash(content_hash)
            .await
            .map_err(|e| RegistryError::Database(e.to_string()))?;

        Ok(dataset.map(PublicDatasetMatch::from))
    }

    /// Verify a user's dataset against the public registry
    /// Returns the matching public dataset if found, and upgrades trust level
    #[instrument(skip(self), fields(dataset_id = %dataset_id))]
    pub async fn verify_dataset(
        &self,
        dataset_id: Uuid,
    ) -> RegistryResult<Option<PublicDatasetMatch>> {
        let metadata = self
            .state
            .metadata_service()
            .ok_or_else(|| RegistryError::Database("Metadata service not available".into()))?;

        // Get the user's dataset
        let dataset = metadata
            .database()
            .get_dataset(dataset_id)
            .await
            .map_err(|e| RegistryError::Database(e.to_string()))?
            .ok_or_else(|| RegistryError::DatasetNotFound(dataset_id.to_string()))?;

        // Search for matching public dataset by hash
        let public_match = self.find_by_hash(&dataset.content_hash).await?;

        if let Some(ref matched) = public_match {
            info!(
                dataset_id = %dataset_id,
                public_name = %matched.name,
                public_version = %matched.version,
                "Dataset matches public registry"
            );

            // Upgrade trust level to Verified
            if let Err(e) = metadata
                .database()
                .update_dataset_trust(dataset_id, TrustLevel::Verified as i32, None)
                .await
            {
                warn!(error = %e, "Failed to upgrade trust level");
            }
        } else {
            debug!(dataset_id = %dataset_id, "No matching public dataset found");
        }

        Ok(public_match)
    }

    /// Cache a public dataset in CyxCloud
    /// Creates a dataset record linked to the public dataset entry
    #[instrument(skip(self), fields(public_name = %name, version = %version))]
    pub async fn cache_public_dataset(
        &self,
        name: &str,
        version: &str,
        cached_dataset_id: Uuid,
    ) -> RegistryResult<()> {
        let metadata = self
            .state
            .metadata_service()
            .ok_or_else(|| RegistryError::Database("Metadata service not available".into()))?;

        // Get the public dataset
        let public_dataset = metadata
            .database()
            .get_public_dataset(name, version)
            .await
            .map_err(|e| RegistryError::Database(e.to_string()))?
            .ok_or_else(|| RegistryError::PublicDatasetNotFound(format!("{} v{}", name, version)))?;

        // Update the cache reference
        metadata
            .database()
            .update_public_dataset_cache(public_dataset.id, cached_dataset_id)
            .await
            .map_err(|e| RegistryError::Database(e.to_string()))?;

        info!(
            public_name = %name,
            version = %version,
            cached_dataset_id = %cached_dataset_id,
            "Public dataset cached"
        );

        Ok(())
    }

    /// Get cached version of a public dataset if available
    #[instrument(skip(self))]
    pub async fn get_cached_dataset(
        &self,
        name: &str,
        version: &str,
    ) -> RegistryResult<Option<Uuid>> {
        let public_dataset = self.get_public_dataset(name, version).await?;
        Ok(public_dataset.cached_dataset_id)
    }

    /// Check if a content hash matches any known public dataset
    pub async fn is_known_public_dataset(&self, content_hash: &[u8]) -> RegistryResult<bool> {
        let matched = self.find_by_hash(content_hash).await?;
        Ok(matched.is_some())
    }

    /// Get the official hash for a public dataset
    #[instrument(skip(self))]
    pub async fn get_official_hash(
        &self,
        name: &str,
        version: &str,
    ) -> RegistryResult<Vec<u8>> {
        let metadata = self
            .state
            .metadata_service()
            .ok_or_else(|| RegistryError::Database("Metadata service not available".into()))?;

        let public_dataset = metadata
            .database()
            .get_public_dataset(name, version)
            .await
            .map_err(|e| RegistryError::Database(e.to_string()))?
            .ok_or_else(|| RegistryError::PublicDatasetNotFound(format!("{} v{}", name, version)))?;

        Ok(public_dataset.official_hash)
    }
}

/// Well-known public dataset names
pub mod datasets {
    pub const MNIST: &str = "MNIST";
    pub const FASHION_MNIST: &str = "Fashion-MNIST";
    pub const CIFAR10: &str = "CIFAR-10";
    pub const CIFAR100: &str = "CIFAR-100";
    pub const IMAGENET_1K: &str = "ImageNet-1K";
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn test_public_dataset_summary_from() {
        let pd = PublicDataset {
            id: Uuid::new_v4(),
            name: "MNIST".to_string(),
            version: "1.0".to_string(),
            official_url: "http://example.com".to_string(),
            official_hash: vec![1, 2, 3],
            paper_url: Some("http://paper.com".to_string()),
            license: Some("MIT".to_string()),
            cached_dataset_id: None,
            cached_at: None,
            verified_by: vec!["torchvision".to_string()],
            verified_at: Utc::now(),
            created_at: Utc::now(),
        };

        let summary = PublicDatasetSummary::from(pd.clone());
        assert_eq!(summary.name, "MNIST");
        assert_eq!(summary.version, "1.0");
        assert!(!summary.cached);

        let matched = PublicDatasetMatch::from(pd);
        assert_eq!(matched.name, "MNIST");
        assert_eq!(matched.confidence, 1.0);
    }
}
