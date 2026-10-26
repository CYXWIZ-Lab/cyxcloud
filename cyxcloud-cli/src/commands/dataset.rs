//! Dataset Commands
//!
//! Commands for managing datasets in CyxCloud:
//! - list: List user's datasets
//! - list-public: List public datasets (MNIST, CIFAR, etc.)
//! - create: Create a dataset from uploaded files
//! - verify: Verify dataset integrity and check public registry
//! - share: Share a dataset with another user
//! - info: Show detailed dataset information

use crate::client::GatewayClient;
use crate::symbols;
use anyhow::Result;
use console::Term;

/// Configuration for verifying a dataset
pub struct VerifyConfig {
    pub dataset_id: String,
    pub check_public: bool,
    pub full_verification: bool,
}

/// Configuration for sharing a dataset
pub struct ShareConfig {
    pub dataset_id: String,
    pub share_with: String,
    pub permissions: Vec<String>,
}

/// Verify a dataset
pub async fn verify(client: &GatewayClient, config: VerifyConfig) -> Result<()> {
    let term = Term::stdout();

    term.write_line(&format!(
        "{} Verifying dataset {}...",
        symbols::LOOKING_GLASS,
        config.dataset_id
    ))?;

    let result = client
        .verify_dataset(
            &config.dataset_id,
            config.check_public,
            config.full_verification,
        )
        .await?;

    term.clear_last_lines(1)?;

    if result.manifest_valid && result.all_files_valid {
        term.write_line(&format!("{} Dataset verification passed!", symbols::CHECK))?;
    } else {
        term.write_line(&format!("{} Dataset verification failed!", symbols::CROSS))?;
    }

    term.write_line(&format!(
        "   Manifest:     {}",
        if result.manifest_valid {
            "valid"
        } else {
            "INVALID"
        }
    ))?;
    term.write_line(&format!(
        "   Files:        {}/{} valid",
        result.files_verified,
        result.files_verified + result.files_failed
    ))?;
    term.write_line(&format!("   Trust level:  {:?}", result.trust_level))?;

    if let Some(public_match) = result.public_match {
        term.write_line("")?;
        term.write_line(&format!(
            "{} Matches public dataset: {} v{}",
            symbols::SPARKLES,
            public_match.name,
            public_match.version
        ))?;
        term.write_line(&format!(
            "   Verified by: {}",
            public_match.verified_by.join(", ")
        ))?;
        term.write_line(&format!(
            "   License:     {}",
            public_match.license.unwrap_or_else(|| "-".to_string())
        ))?;
    }

    Ok(())
}

/// Share a dataset
pub async fn share(client: &GatewayClient, config: ShareConfig) -> Result<()> {
    let term = Term::stdout();

    term.write_line(&format!(
        "{} Sharing dataset with {}...",
        symbols::LINK,
        config.share_with
    ))?;

    let result = client
        .share_dataset(&config.dataset_id, &config.share_with, &config.permissions)
        .await?;

    term.clear_last_lines(1)?;

    if result.success {
        term.write_line(&format!("{} Dataset shared successfully!", symbols::CHECK))?;
        term.write_line(&format!("   Share ID:    {}", result.share_id))?;
        term.write_line(&format!("   Shared with: {}", config.share_with))?;
        term.write_line(&format!(
            "   Permissions: {}",
            config.permissions.join(", ")
        ))?;
    } else {
        term.write_line(&format!("{} Failed to share dataset", symbols::CROSS))?;
    }

    Ok(())
}
