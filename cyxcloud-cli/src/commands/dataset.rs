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
use console::{style, Term};

/// Configuration for listing datasets
pub struct ListConfig {
    pub include_shared: bool,
    pub limit: i32,
}

/// Configuration for listing public datasets
pub struct ListPublicConfig {
    pub name_filter: Option<String>,
}

/// Configuration for creating a dataset
pub struct CreateConfig {
    pub name: String,
    pub description: Option<String>,
    pub file_ids: Vec<String>,
    pub bucket: String,
    pub prefix: Option<String>,
}

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

/// Configuration for getting dataset info
pub struct InfoConfig {
    pub dataset_id: String,
}

/// List user's datasets
pub async fn list(client: &GatewayClient, config: ListConfig) -> Result<()> {
    let term = Term::stdout();

    term.write_line(&format!(
        "{} Fetching datasets...",
        symbols::LOOKING_GLASS
    ))?;

    // Call gateway API to list datasets
    // For now, show placeholder - will integrate with gRPC DataStreamService
    let datasets = client.list_datasets(config.include_shared, config.limit).await?;

    term.clear_last_lines(1)?;

    if datasets.is_empty() {
        term.write_line(&format!(
            "{} No datasets found. Create one with: cyxcloud dataset create <name>",
            symbols::INFO
        ))?;
        return Ok(());
    }

    // Header
    term.write_line(&format!(
        "{:<36}  {:<20}  {:>10}  {:>8}  {}",
        style("ID").bold(),
        style("Name").bold(),
        style("Size").bold(),
        style("Files").bold(),
        style("Trust").bold()
    ))?;
    term.write_line(&"-".repeat(90))?;

    for dataset in &datasets {
        let trust_icon = match dataset.trust_level {
            0 => format!("{} self", symbols::LOCK),
            1 => format!("{} signed", symbols::PEN),
            2 => format!("{} verified", symbols::CHECK),
            3 => format!("{} attested", symbols::SHIELD),
            _ => format!("{} untrusted", symbols::WARNING),
        };

        term.write_line(&format!(
            "{:<36}  {:<20}  {:>10}  {:>8}  {}",
            &dataset.id[..36.min(dataset.id.len())],
            truncate(&dataset.name, 20),
            format_bytes(dataset.size_bytes),
            dataset.file_count,
            trust_icon
        ))?;
    }

    term.write_line("")?;
    term.write_line(&format!(
        "{} {} dataset(s)",
        symbols::INFO,
        datasets.len()
    ))?;

    Ok(())
}

/// List public datasets
pub async fn list_public(client: &GatewayClient, config: ListPublicConfig) -> Result<()> {
    let term = Term::stdout();

    term.write_line(&format!(
        "{} Fetching public datasets...",
        symbols::LOOKING_GLASS
    ))?;

    let datasets = client.list_public_datasets(config.name_filter.as_deref()).await?;

    term.clear_last_lines(1)?;

    if datasets.is_empty() {
        term.write_line(&format!(
            "{} No public datasets found",
            symbols::INFO
        ))?;
        return Ok(());
    }

    // Header
    term.write_line(&format!(
        "{:<20}  {:<10}  {:<15}  {:<30}  {}",
        style("Name").bold(),
        style("Version").bold(),
        style("License").bold(),
        style("Verified By").bold(),
        style("Cached").bold()
    ))?;
    term.write_line(&"-".repeat(90))?;

    for dataset in &datasets {
        let cached_icon = if dataset.cached {
            format!("{} yes", symbols::CHECK)
        } else {
            format!("{} no", symbols::CROSS)
        };

        let verified_by = dataset.verified_by.join(", ");

        term.write_line(&format!(
            "{:<20}  {:<10}  {:<15}  {:<30}  {}",
            truncate(&dataset.name, 20),
            truncate(&dataset.version, 10),
            truncate(&dataset.license.as_deref().unwrap_or("-"), 15),
            truncate(&verified_by, 30),
            cached_icon
        ))?;
    }

    term.write_line("")?;
    term.write_line(&format!(
        "{} {} public dataset(s) available",
        symbols::INFO,
        datasets.len()
    ))?;

    Ok(())
}

/// Create a dataset from files
pub async fn create(client: &GatewayClient, config: CreateConfig) -> Result<()> {
    let term = Term::stdout();

    term.write_line(&format!(
        "{} Creating dataset '{}'...",
        symbols::PACKAGE,
        config.name
    ))?;

    let dataset = client.create_dataset(
        &config.name,
        config.description.as_deref(),
        &config.bucket,
        config.prefix.as_deref(),
    ).await?;

    term.clear_last_lines(1)?;

    term.write_line(&format!(
        "{} Dataset created successfully!",
        symbols::CHECK
    ))?;
    term.write_line(&format!("   ID:    {}", dataset.id))?;
    term.write_line(&format!("   Name:  {}", dataset.name))?;
    term.write_line(&format!("   Files: {}", dataset.file_count))?;
    term.write_line(&format!("   Size:  {}", format_bytes(dataset.size_bytes)))?;
    term.write_line(&format!("   Hash:  {}", hex::encode(&dataset.content_hash[..16.min(dataset.content_hash.len())])))?;

    Ok(())
}

/// Verify a dataset
pub async fn verify(client: &GatewayClient, config: VerifyConfig) -> Result<()> {
    let term = Term::stdout();

    term.write_line(&format!(
        "{} Verifying dataset {}...",
        symbols::LOOKING_GLASS,
        config.dataset_id
    ))?;

    let result = client.verify_dataset(
        &config.dataset_id,
        config.check_public,
        config.full_verification,
    ).await?;

    term.clear_last_lines(1)?;

    if result.manifest_valid && result.all_files_valid {
        term.write_line(&format!(
            "{} Dataset verification passed!",
            symbols::CHECK
        ))?;
    } else {
        term.write_line(&format!(
            "{} Dataset verification failed!",
            symbols::CROSS
        ))?;
    }

    term.write_line(&format!("   Manifest:     {}", if result.manifest_valid { "valid" } else { "INVALID" }))?;
    term.write_line(&format!("   Files:        {}/{} valid", result.files_verified, result.files_verified + result.files_failed))?;
    term.write_line(&format!("   Trust level:  {:?}", result.trust_level))?;

    if let Some(public_match) = result.public_match {
        term.write_line("")?;
        term.write_line(&format!(
            "{} Matches public dataset: {} v{}",
            symbols::SPARKLES,
            public_match.name,
            public_match.version
        ))?;
        term.write_line(&format!("   Verified by: {}", public_match.verified_by.join(", ")))?;
        term.write_line(&format!("   License:     {}", public_match.license.unwrap_or_else(|| "-".to_string())))?;
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

    let result = client.share_dataset(
        &config.dataset_id,
        &config.share_with,
        &config.permissions,
    ).await?;

    term.clear_last_lines(1)?;

    if result.success {
        term.write_line(&format!(
            "{} Dataset shared successfully!",
            symbols::CHECK
        ))?;
        term.write_line(&format!("   Share ID:    {}", result.share_id))?;
        term.write_line(&format!("   Shared with: {}", config.share_with))?;
        term.write_line(&format!("   Permissions: {}", config.permissions.join(", ")))?;
    } else {
        term.write_line(&format!(
            "{} Failed to share dataset",
            symbols::CROSS
        ))?;
    }

    Ok(())
}

/// Get dataset info
pub async fn info(client: &GatewayClient, config: InfoConfig) -> Result<()> {
    let term = Term::stdout();

    term.write_line(&format!(
        "{} Fetching dataset info...",
        symbols::LOOKING_GLASS
    ))?;

    let dataset = client.get_dataset_info(&config.dataset_id).await?;

    term.clear_last_lines(1)?;

    let trust_label = match dataset.trust_level {
        0 => "self (your upload)",
        1 => "signed (cryptographically signed)",
        2 => "verified (matches public registry)",
        3 => "attested (TEE verified)",
        _ => "untrusted",
    };

    term.write_line(&format!("{} Dataset Information", symbols::INFO))?;
    term.write_line(&format!("   ID:          {}", dataset.id))?;
    term.write_line(&format!("   Name:        {}", dataset.name))?;
    term.write_line(&format!("   Description: {}", dataset.description.unwrap_or_else(|| "-".to_string())))?;
    term.write_line(&format!("   Owner:       {}", dataset.owner_id))?;
    term.write_line(&format!("   Files:       {}", dataset.file_count))?;
    term.write_line(&format!("   Size:        {}", format_bytes(dataset.size_bytes)))?;
    term.write_line(&format!("   Hash:        {}", hex::encode(&dataset.content_hash)))?;
    term.write_line(&format!("   Trust:       {}", trust_label))?;
    term.write_line(&format!("   Version:     {}", dataset.version))?;
    term.write_line(&format!("   Created:     {}", dataset.created_at))?;
    term.write_line(&format!("   Updated:     {}", dataset.updated_at))?;

    Ok(())
}

// Helper functions

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() > max_len {
        format!("{}...", &s[..max_len - 3])
    } else {
        s.to_string()
    }
}

fn format_bytes(bytes: i64) -> String {
    const KB: i64 = 1024;
    const MB: i64 = KB * 1024;
    const GB: i64 = MB * 1024;
    const TB: i64 = GB * 1024;

    if bytes >= TB {
        format!("{:.1} TB", bytes as f64 / TB as f64)
    } else if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}
