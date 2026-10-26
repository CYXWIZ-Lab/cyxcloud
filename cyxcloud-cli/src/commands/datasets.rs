//! `cyxcloud dataset put | ls | get | info | rm` and `cyxcloud token create`
//! (TOFIX177 slice 2): datasets are the user's objects, the same ones the
//! Engine's Data Input > CyxCloud shows.

use crate::cloud::{self, short_id, Cloud};
use crate::server::{human_bytes, human_duration};
use crate::symbols;
use anyhow::{bail, Context, Result};
use console::style;
use indicatif::{ProgressBar, ProgressStyle};
use std::path::PathBuf;

fn bar(len: u64, label: &str) -> ProgressBar {
    let pb = ProgressBar::new(len);
    pb.set_style(
        ProgressStyle::with_template("  {msg:<24} {bytes:>9} {bar:24} {percent:>3}%")
            .unwrap()
            .progress_chars("█░ "),
    );
    pb.set_message(label.to_string());
    pb
}

fn when(unix: i64) -> String {
    chrono::DateTime::from_timestamp(unix, 0)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|| "-".into())
}

/// `cyxcloud dataset put <PATH>... --name NAME`
pub async fn put(
    cloud: &Cloud,
    paths: Vec<PathBuf>,
    name: String,
    description: Option<String>,
) -> Result<()> {
    if paths.is_empty() {
        bail!("give at least one file");
    }
    let mut files = Vec::new();
    for p in &paths {
        if p.is_dir() {
            let mut entries: Vec<PathBuf> = std::fs::read_dir(p)?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|e| e.is_file())
                .collect();
            entries.sort();
            files.extend(entries);
        } else if p.is_file() {
            files.push(p.clone());
        } else {
            bail!("{} is not a file or a folder", p.display());
        }
    }
    let mut ids = Vec::new();
    let mut total = 0u64;
    for f in &files {
        let size = std::fs::metadata(f)?.len();
        let label = f
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let pb = bar(size, &label);
        let id = cloud.upload(f, |sent| pb.set_position(sent)).await;
        match id {
            Ok(id) => {
                pb.finish();
                ids.push(id);
                total += size;
            }
            Err(e) => {
                pb.abandon();
                return Err(e.context(format!("{} was not uploaded", f.display())));
            }
        }
    }
    let ds = cloud
        .create(&name, &description.unwrap_or_default(), ids)
        .await
        .context("the files were uploaded but the dataset was not created")?;
    println!(
        "{} Dataset {} ({}) · {} file(s) · {}",
        style(symbols::CHECK).green(),
        style(&ds.name).cyan(),
        short_id(&ds.id),
        ds.file_count,
        human_bytes(total)
    );
    println!("  In the Engine: Data Input > CyxCloud > {}", ds.name);
    Ok(())
}

/// `cyxcloud dataset ls`
pub async fn ls(cloud: &Cloud, shared: bool, public: bool, filter: Option<String>) -> Result<()> {
    if public {
        let list = cloud.list_public(filter).await?;
        if list.is_empty() {
            println!("No public datasets");
            return Ok(());
        }
        println!(
            "  {:<28} {:<10} {:<12} {:<7} ID",
            "NAME", "VERSION", "LICENSE", "CACHED"
        );
        for d in list {
            println!(
                "  {:<28} {:<10} {:<12} {:<7} {}",
                d.name,
                d.version,
                d.license,
                if d.cached { "yes" } else { "no" },
                short_id(&d.id)
            );
        }
        return Ok(());
    }
    let mut list = cloud.list(shared).await?;
    if let Some(f) = &filter {
        list.retain(|d| d.name.contains(f.as_str()));
    }
    if list.is_empty() {
        println!("No datasets yet · cyxcloud dataset put <files> --name NAME");
        return Ok(());
    }
    list.sort_by(|a, b| a.name.cmp(&b.name));
    println!(
        "  {:<28} {:>5} {:>9}  {:<16}  ID",
        "NAME", "FILES", "SIZE", "UPDATED"
    );
    for d in list {
        println!(
            "  {:<28} {:>5} {:>9}  {:<16}  {}",
            d.name,
            d.file_count,
            human_bytes(d.total_size_bytes.max(0) as u64),
            when(d.updated_at),
            short_id(&d.id)
        );
    }
    Ok(())
}

/// `cyxcloud dataset info NAME|ID`
pub async fn info(cloud: &Cloud, which: &str) -> Result<()> {
    let all = cloud.list(true).await?;
    let d = cloud::resolve_dataset(&all, which)?;
    let (d, files) = cloud.info(&d.id).await?;
    println!("  {} · {}", style(&d.name).cyan().bold(), d.id);
    if !d.description.is_empty() {
        println!("  {}", d.description);
    }
    println!(
        "  {} file(s) · {} · created {} · updated {}",
        d.file_count,
        human_bytes(d.total_size_bytes.max(0) as u64),
        when(d.created_at),
        when(d.updated_at)
    );
    for f in files {
        println!(
            "    {:<36} {:>9}  {}",
            f.path_in_dataset
                .rsplit('/')
                .next()
                .unwrap_or(&f.path_in_dataset),
            human_bytes(f.size_bytes.max(0) as u64),
            short_id(&f.id)
        );
    }
    Ok(())
}

/// `cyxcloud dataset get NAME|ID [-o DIR]`
pub async fn get(cloud: &Cloud, which: &str, out: PathBuf) -> Result<()> {
    let all = cloud.list(true).await?;
    let d = cloud::resolve_dataset(&all, which)?;
    let (d, files) = cloud.info(&d.id).await?;
    if files.is_empty() {
        bail!("dataset {} has no files", d.name);
    }
    std::fs::create_dir_all(&out).with_context(|| format!("cannot create {}", out.display()))?;
    let (token, _) = cloud.token(&d.id, 3600).await?;
    let mut written = 0;
    for f in &files {
        let target = cloud::safe_file_name(&out, &f.path_in_dataset).with_context(|| {
            format!(
                "the gateway sent an unusable file name '{}'",
                f.path_in_dataset
            )
        })?;
        let label = target
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let pb = bar(f.size_bytes.max(0) as u64, &label);
        match cloud
            .download(&f.id, &token, &target, |n| pb.set_position(n))
            .await
        {
            Ok(_) => {
                pb.finish_with_message(format!("{} sha256 verified", label));
                written += 1;
            }
            Err(e) => {
                pb.abandon();
                return Err(e.context(format!("{} was not downloaded", label)));
            }
        }
    }
    println!(
        "{} {} file(s) written to {}",
        style(symbols::CHECK).green(),
        written,
        out.display()
    );
    Ok(())
}

/// `cyxcloud dataset rm NAME|ID [--force]`
pub async fn rm(cloud: &Cloud, which: &str, force: bool) -> Result<()> {
    let all = cloud.list(false).await?;
    let d = cloud::resolve_dataset(&all, which)?;
    if !force {
        if !console::user_attended() {
            bail!(
                "removing {} needs --force when not run in a terminal",
                d.name
            );
        }
        print!(
            "  Remove dataset {} ({}, {} file(s))? This cannot be undone. [y/N] ",
            d.name,
            short_id(&d.id),
            d.file_count
        );
        std::io::Write::flush(&mut std::io::stdout())?;
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if !answer.trim().eq_ignore_ascii_case("y") {
            println!("  Kept.");
            return Ok(());
        }
    }
    cloud.delete(&d.id).await?;
    println!(
        "{} Dataset {} removed",
        style(symbols::CHECK).green(),
        d.name
    );
    Ok(())
}

/// `cyxcloud token create NAME|ID [--ttl 1h]`: prints only the token, so
/// scripts can capture it; the rest goes to stderr.
pub async fn token_create(cloud: &Cloud, which: &str, ttl: &str) -> Result<()> {
    let seconds = parse_ttl(ttl).map_err(anyhow::Error::msg)?;
    let all = cloud.list(true).await?;
    let d = cloud::resolve_dataset(&all, which)?;
    let (token, expires) = cloud.token(&d.id, seconds).await?;
    eprintln!(
        "{} Access token for {} (read, stream), valid for {} (until {})",
        style(symbols::CHECK).green(),
        d.name,
        human_duration(seconds),
        when(expires)
    );
    println!("{}", token);
    Ok(())
}

/// "90s", "15m", "1h", "2d", or seconds.
pub fn parse_ttl(text: &str) -> Result<i64, String> {
    let t = text.trim().to_ascii_lowercase();
    let (number, factor) = match t.chars().last() {
        Some('s') => (&t[..t.len() - 1], 1),
        Some('m') => (&t[..t.len() - 1], 60),
        Some('h') => (&t[..t.len() - 1], 3600),
        Some('d') => (&t[..t.len() - 1], 86_400),
        _ => (t.as_str(), 1),
    };
    let n: i64 = number
        .parse()
        .map_err(|_| format!("'{}' is not a duration (try 15m, 1h, 2d)", text))?;
    let secs = n * factor;
    if !(60..=30 * 86_400).contains(&secs) {
        return Err("a token lives between 1 minute and 30 days".into());
    }
    Ok(secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_lifetimes_are_read_in_words() {
        assert_eq!(parse_ttl("1h"), Ok(3600));
        assert_eq!(parse_ttl("15m"), Ok(900));
        assert_eq!(parse_ttl("2d"), Ok(172_800));
        assert_eq!(parse_ttl("600"), Ok(600));
        assert!(parse_ttl("10s").is_err(), "under a minute");
        assert!(parse_ttl("31d").is_err());
        assert!(parse_ttl("soon").is_err());
    }
}
