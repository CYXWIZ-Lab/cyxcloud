//! Download Command
//!
//! Downloads files or directories from CyxCloud storage.

use crate::client::GatewayClient;
use crate::symbols;
use anyhow::{Context, Result};
use console::style;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::path::Path;
use tokio::fs;

/// Download configuration
pub struct DownloadConfig {
    pub bucket: String,
    pub key: Option<String>,
    pub output: String,
    pub prefix: Option<String>,
}

/// Run download command
pub async fn run(client: &GatewayClient, config: DownloadConfig) -> Result<()> {
    let output_path = Path::new(&config.output);

    // If a specific key is provided, download single file
    if let Some(key) = &config.key {
        download_single_file(client, &config.bucket, key, output_path).await?;
    } else {
        // Download all objects with prefix
        download_prefix(
            client,
            &config.bucket,
            config.prefix.as_deref(),
            output_path,
        )
        .await?;
    }

    Ok(())
}

/// Download a single file
async fn download_single_file(
    client: &GatewayClient,
    bucket: &str,
    key: &str,
    output_path: &Path,
) -> Result<()> {
    // Get object metadata first
    let metadata = client
        .head_object(bucket, key)
        .await
        .context("Failed to get object metadata")?;

    // Determine output file path
    let file_path = if output_path.is_dir() {
        // If output is a directory, use the key's filename
        let filename = key.split('/').next_back().unwrap_or(key);
        output_path.join(filename)
    } else {
        output_path.to_path_buf()
    };

    // Ensure parent directory exists
    if let Some(parent) = file_path.parent() {
        fs::create_dir_all(parent).await?;
    }

    // Create progress bar
    let pb = ProgressBar::new(metadata.size);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")
            .unwrap()
            .progress_chars("#>-"),
    );

    pb.set_message(format!("Downloading {}", key));

    // Download file
    let size = client
        .download_to_file(bucket, key, &file_path)
        .await
        .context("Failed to download file")?;

    pb.finish_with_message(format!(
        "{} Downloaded {} ({} bytes)",
        style(symbols::CHECK).green(),
        key,
        size
    ));

    println!(
        "\n{} {}\n  Size: {} bytes\n  Saved to: {}",
        style("Successfully downloaded:").green().bold(),
        key,
        size,
        file_path.display()
    );

    Ok(())
}

/// Download all objects with a prefix
async fn download_prefix(
    client: &GatewayClient,
    bucket: &str,
    prefix: Option<&str>,
    output_dir: &Path,
) -> Result<()> {
    // List objects with prefix
    let response = client
        .list_objects(bucket, prefix, None)
        .await
        .context("Failed to list objects")?;

    if response.objects.is_empty() {
        println!(
            "{} No objects found with prefix: {}",
            style("Warning:").yellow(),
            prefix.unwrap_or("(none)")
        );
        return Ok(());
    }

    println!(
        "{} {} objects to download",
        style("Found").cyan(),
        response.objects.len()
    );

    // Ensure output directory exists
    fs::create_dir_all(output_dir).await?;

    // Create multi-progress bar
    let multi = MultiProgress::new();
    let overall_pb = multi.add(ProgressBar::new(response.objects.len() as u64));
    overall_pb.set_style(
        ProgressStyle::default_bar()
            .template(
                "{spinner:.green} [{elapsed_precise}] [{bar:40.green/white}] {pos}/{len} files",
            )
            .unwrap()
            .progress_chars("█▓░"),
    );

    let mut total_bytes: u64 = 0;
    let mut success_count = 0;
    let mut error_count = 0;

    for obj in &response.objects {
        // Calculate local file path
        let relative_path = if let Some(p) = prefix {
            obj.key
                .strip_prefix(p)
                .unwrap_or(&obj.key)
                .trim_start_matches('/')
        } else {
            &obj.key
        };

        let file_path = output_dir.join(relative_path);

        // Ensure parent directory exists
        if let Some(parent) = file_path.parent() {
            if let Err(e) = fs::create_dir_all(parent).await {
                eprintln!(
                    "{} Failed to create directory {}: {}",
                    style(symbols::CROSS).red(),
                    parent.display(),
                    e
                );
                error_count += 1;
                overall_pb.inc(1);
                continue;
            }
        }

        match client.download_to_file(bucket, &obj.key, &file_path).await {
            Ok(size) => {
                total_bytes += size;
                success_count += 1;
            }
            Err(e) => {
                eprintln!(
                    "{} Failed to download {}: {}",
                    style(symbols::CROSS).red(),
                    obj.key,
                    e
                );
                error_count += 1;
            }
        }

        overall_pb.inc(1);
    }

    overall_pb.finish_with_message("Download complete");

    // Print summary
    println!("\n{}", style("Download Summary:").bold());
    println!(
        "  {} files downloaded successfully",
        style(success_count).green()
    );
    if error_count > 0 {
        println!("  {} files failed", style(error_count).red());
    }
    println!("  {} total bytes transferred", format_bytes(total_bytes));
    println!("  Saved to: {}", output_dir.display());

    Ok(())
}

/// Format bytes as human-readable string
fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} bytes", bytes)
    }
}
