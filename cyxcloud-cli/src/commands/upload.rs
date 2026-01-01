//! Upload Command
//!
//! Uploads files or directories to CyxCloud storage.

use crate::client::GatewayClient;
use crate::symbols;
use anyhow::{Context, Result};
use console::style;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::path::Path;
use tokio::fs;

/// Upload configuration
pub struct UploadConfig {
    pub path: String,
    pub bucket: String,
    pub prefix: Option<String>,
    pub encrypt: bool,
}

/// Run upload command
pub async fn run(client: &GatewayClient, config: UploadConfig) -> Result<()> {
    let path = Path::new(&config.path);

    if !path.exists() {
        anyhow::bail!("Path does not exist: {}", config.path);
    }

    // Ensure bucket exists
    client
        .create_bucket(&config.bucket)
        .await
        .context("Failed to create bucket")?;

    if path.is_file() {
        upload_single_file(client, &config.bucket, path, config.prefix.as_deref()).await?;
    } else if path.is_dir() {
        upload_directory(client, &config.bucket, path, config.prefix.as_deref()).await?;
    } else {
        anyhow::bail!("Path is neither a file nor directory: {}", config.path);
    }

    Ok(())
}

/// Upload a single file
async fn upload_single_file(
    client: &GatewayClient,
    bucket: &str,
    path: &Path,
    prefix: Option<&str>,
) -> Result<()> {
    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");

    let key = match prefix {
        Some(p) => format!("{}/{}", p.trim_matches('/'), file_name),
        None => file_name.to_string(),
    };

    let metadata = fs::metadata(path).await?;
    let size = metadata.len();

    // Create progress bar
    let pb = ProgressBar::new(size);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")
            .unwrap()
            .progress_chars("#>-"),
    );

    pb.set_message(format!("Uploading {}", file_name));

    // Upload file
    let (etag, uploaded_size) = client
        .upload_local_file(bucket, &key, path)
        .await
        .context("Failed to upload file")?;

    pb.finish_with_message(format!(
        "{} Uploaded {} ({} bytes)",
        style(symbols::CHECK).green(),
        key,
        uploaded_size
    ));

    println!(
        "\n{} {}/{}\n  ETag: {}\n  Size: {} bytes",
        style("Successfully uploaded:").green().bold(),
        bucket,
        key,
        etag,
        uploaded_size
    );

    Ok(())
}

/// Upload a directory recursively
async fn upload_directory(
    client: &GatewayClient,
    bucket: &str,
    dir_path: &Path,
    prefix: Option<&str>,
) -> Result<()> {
    // Collect all files first
    let files = collect_files(dir_path).await?;

    if files.is_empty() {
        println!("{}", style("No files to upload").yellow());
        return Ok(());
    }

    println!("{} {} files to upload", style("Found").cyan(), files.len());

    // Create multi-progress bar
    let multi = MultiProgress::new();
    let overall_pb = multi.add(ProgressBar::new(files.len() as u64));
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

    for file_path in &files {
        // Calculate key from relative path
        let relative = file_path.strip_prefix(dir_path).unwrap_or(file_path);

        let key = match prefix {
            Some(p) => format!("{}/{}", p.trim_matches('/'), relative.display()),
            None => relative.display().to_string(),
        };

        // Replace backslashes with forward slashes for S3 compatibility
        let key = key.replace('\\', "/");

        match client.upload_local_file(bucket, &key, file_path).await {
            Ok((_, size)) => {
                total_bytes += size;
                success_count += 1;
            }
            Err(e) => {
                eprintln!(
                    "{} Failed to upload {}: {}",
                    style(symbols::CROSS).red(),
                    file_path.display(),
                    e
                );
                error_count += 1;
            }
        }

        overall_pb.inc(1);
    }

    overall_pb.finish_with_message("Upload complete");

    // Print summary
    println!("\n{}", style("Upload Summary:").bold());
    println!(
        "  {} files uploaded successfully",
        style(success_count).green()
    );
    if error_count > 0 {
        println!("  {} files failed", style(error_count).red());
    }
    println!("  {} total bytes transferred", format_bytes(total_bytes));

    Ok(())
}

/// Collect all files in a directory recursively
async fn collect_files(dir: &Path) -> Result<Vec<std::path::PathBuf>> {
    let mut files = Vec::new();
    let mut stack = vec![dir.to_path_buf()];

    while let Some(current) = stack.pop() {
        let mut entries = fs::read_dir(&current).await?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();

            if path.is_dir() {
                stack.push(path);
            } else if path.is_file() {
                files.push(path);
            }
        }
    }

    files.sort();
    Ok(files)
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
