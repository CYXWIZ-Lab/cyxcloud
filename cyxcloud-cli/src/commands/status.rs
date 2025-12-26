//! Status Command
//!
//! Shows storage status and health information.

use crate::client::GatewayClient;
use anyhow::Result;
use console::style;

/// Status configuration
pub struct StatusConfig {
    pub bucket: Option<String>,
    pub verbose: bool,
}

/// Run status command
pub async fn run(client: &GatewayClient, config: StatusConfig) -> Result<()> {
    // Check gateway health
    let healthy = client.health().await.unwrap_or(false);

    println!("{}", style("CyxCloud Storage Status").bold().underlined());
    println!();

    // Gateway status
    let status_icon = if healthy {
        style("●").green()
    } else {
        style("●").red()
    };
    let status_text = if healthy {
        style("Online").green()
    } else {
        style("Offline").red()
    };
    println!("Gateway Status: {} {}", status_icon, status_text);
    println!();

    if !healthy {
        println!(
            "{}",
            style("Cannot retrieve storage status: gateway is offline").yellow()
        );
        return Ok(());
    }

    // If a specific bucket is requested, show its details
    if let Some(bucket) = &config.bucket {
        show_bucket_status(client, bucket, config.verbose).await?;
    } else {
        show_overall_status(config.verbose);
    }

    Ok(())
}

/// Show status for a specific bucket
async fn show_bucket_status(client: &GatewayClient, bucket: &str, verbose: bool) -> Result<()> {
    println!("{}", style(format!("Bucket: {}", bucket)).bold());
    println!();

    // List objects to get stats
    let response = client.list_objects(bucket, None, Some(1000)).await;

    match response {
        Ok(list) => {
            let total_size: u64 = list.objects.iter().map(|o| o.size).sum();
            let object_count = list.objects.len();

            println!("  Objects:      {}", style(object_count).cyan());
            println!("  Total Size:   {}", style(format_bytes(total_size)).cyan());

            if list.is_truncated {
                println!(
                    "  {}",
                    style("(showing first 1000 objects)").dim()
                );
            }

            if verbose && !list.objects.is_empty() {
                println!();
                println!("  {}", style("Recent Objects:").bold());

                // Show last 5 objects
                let recent: Vec<_> = list.objects.iter().take(5).collect();
                for obj in recent {
                    println!(
                        "    {} ({}) - {}",
                        obj.key,
                        format_bytes(obj.size),
                        if obj.last_modified.is_empty() {
                            "unknown".to_string()
                        } else {
                            obj.last_modified.clone()
                        }
                    );
                }

                if list.objects.len() > 5 {
                    println!(
                        "    {} more objects...",
                        style(format!("... and {}", list.objects.len() - 5)).dim()
                    );
                }
            }
        }
        Err(e) => {
            println!(
                "  {} Failed to get bucket info: {}",
                style("Error:").red(),
                e
            );
        }
    }

    Ok(())
}

/// Show overall storage status
fn show_overall_status(verbose: bool) {
    println!("{}", style("Storage Overview").bold());
    println!();
    println!("  Use 'cyxcloud status --bucket <name>' to view bucket details");
    println!("  Use 'cyxcloud list <bucket>' to list objects");

    if verbose {
        println!();
        println!("{}", style("Available Commands:").bold());
        println!("  upload    Upload files to storage");
        println!("  download  Download files from storage");
        println!("  list      List objects in a bucket");
        println!("  status    Show storage status (this command)");
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
        format!("{} bytes", bytes)
    }
}
