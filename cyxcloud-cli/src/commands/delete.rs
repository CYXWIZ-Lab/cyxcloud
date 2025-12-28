//! Delete Command
//!
//! Deletes files from CyxCloud storage.

use crate::client::GatewayClient;
use crate::symbols;
use anyhow::{Context, Result};
use console::style;

/// Delete configuration
pub struct DeleteConfig {
    pub bucket: String,
    pub key: String,
    pub force: bool,
}

/// Run delete command
pub async fn run(client: &GatewayClient, config: DeleteConfig) -> Result<()> {
    // Check if object exists first
    match client.head_object(&config.bucket, &config.key).await {
        Ok(info) => {
            if !config.force {
                println!(
                    "{} About to delete: {}/{}",
                    style("Warning:").yellow(),
                    config.bucket,
                    config.key
                );
                println!("  Size: {} bytes", info.size);
                println!("  Last modified: {}", info.last_modified);
                println!("\nUse --force to delete without confirmation.");
                return Ok(());
            }
        }
        Err(crate::client::ClientError::NotFound(_)) => {
            println!(
                "{} Object not found: {}/{}",
                style("Error:").red(),
                config.bucket,
                config.key
            );
            return Ok(());
        }
        Err(e) => {
            return Err(e).context("Failed to check object");
        }
    }

    // Delete the object
    client
        .delete_object(&config.bucket, &config.key)
        .await
        .context("Failed to delete object")?;

    println!(
        "{} Deleted: {}/{}",
        style(symbols::CHECK).green(),
        config.bucket,
        config.key
    );

    Ok(())
}
