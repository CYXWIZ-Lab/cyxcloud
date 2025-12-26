//! List Command
//!
//! Lists objects in CyxCloud storage buckets.

use crate::client::GatewayClient;
use anyhow::{Context, Result};
use console::style;

/// List configuration
pub struct ListConfig {
    pub bucket: String,
    pub prefix: Option<String>,
    pub long_format: bool,
    pub human_readable: bool,
    pub max_keys: Option<i32>,
}

/// Run list command
pub async fn run(client: &GatewayClient, config: ListConfig) -> Result<()> {
    let response = client
        .list_objects(&config.bucket, config.prefix.as_deref(), config.max_keys)
        .await
        .context("Failed to list objects")?;

    if response.objects.is_empty() {
        println!(
            "{} No objects found in bucket '{}' with prefix '{}'",
            style("Info:").cyan(),
            config.bucket,
            config.prefix.as_deref().unwrap_or("(none)")
        );
        return Ok(());
    }

    // Print header
    if config.long_format {
        println!(
            "{:<40} {:>12} {:>24} {}",
            style("KEY").bold(),
            style("SIZE").bold(),
            style("LAST MODIFIED").bold(),
            style("ETAG").bold()
        );
        println!("{}", "-".repeat(100));
    }

    let mut total_size: u64 = 0;

    for obj in &response.objects {
        total_size += obj.size;

        if config.long_format {
            let size_str = if config.human_readable {
                format_bytes(obj.size)
            } else {
                obj.size.to_string()
            };

            println!(
                "{:<40} {:>12} {:>24} {}",
                truncate_key(&obj.key, 40),
                size_str,
                truncate_string(&obj.last_modified, 24),
                truncate_string(&obj.etag, 16)
            );
        } else {
            println!("{}", obj.key);
        }
    }

    // Print summary
    if config.long_format {
        println!("{}", "-".repeat(100));
        println!(
            "{} objects, {} total",
            style(response.objects.len()).green(),
            if config.human_readable {
                format_bytes(total_size)
            } else {
                format!("{} bytes", total_size)
            }
        );

        if response.is_truncated {
            println!(
                "{}",
                style("(results truncated, use --max-keys to retrieve more)").yellow()
            );
        }
    }

    Ok(())
}

/// Truncate a key for display
fn truncate_key(key: &str, max_len: usize) -> String {
    if key.len() <= max_len {
        key.to_string()
    } else {
        format!("...{}", &key[key.len() - (max_len - 3)..])
    }
}

/// Truncate a string for display
fn truncate_string(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len - 3])
    }
}

/// Format bytes as human-readable string
fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    const TB: u64 = GB * 1024;

    if bytes >= TB {
        format!("{:.2} TB", bytes as f64 / TB as f64)
    } else if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_key() {
        assert_eq!(truncate_key("short", 40), "short");
        // Key is 42 chars, max is 20, so we show last 17 chars prefixed with "..."
        assert_eq!(
            truncate_key("this/is/a/very/long/path/to/some/file.txt", 20),
            ".../to/some/file.txt"
        );
    }

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(500), "500 B");
        assert_eq!(format_bytes(1024), "1.00 KB");
        assert_eq!(format_bytes(1536), "1.50 KB");
        assert_eq!(format_bytes(1024 * 1024), "1.00 MB");
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1.00 GB");
        assert_eq!(format_bytes(1024u64 * 1024 * 1024 * 1024), "1.00 TB");
    }
}
