//! `cyxcloud node service install | uninstall`: start the node with the
//! machine (TOFIX177 slice 2). No administrator rights needed:
//!
//! - Linux: a systemd user unit (`~/.config/systemd/user/cyxcloud-node.service`)
//!   that restarts the node when it fails; `loginctl enable-linger` lets it
//!   start at boot without anybody logging in.
//! - macOS: a launchd agent (`~/Library/LaunchAgents/ai.cyxwiz.cyxcloud-node.plist`)
//!   with KeepAlive; it starts at login.
//! - Windows: this user's Run key (HKCU\...\CurrentVersion\Run) runs
//!   `cyxcloud node start` at logon (it detaches the node and keeps its log).
//!   A scheduled logon task would need administrator rights.

use crate::commands::node;
use crate::symbols;
use anyhow::{bail, Context, Result};
use console::style;
use std::path::{Path, PathBuf};

pub const SYSTEMD_UNIT: &str = "cyxcloud-node.service";
pub const LAUNCHD_LABEL: &str = "ai.cyxwiz.cyxcloud-node";
pub const WINDOWS_RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
pub const WINDOWS_RUN_VALUE: &str = "CyxCloud Node";

// ============================================================================
// Pure pieces (tested)
// ============================================================================

/// The systemd user unit for the node.
pub fn systemd_unit(node_bin: &Path, config: &Path, log: &Path, workdir: &Path) -> String {
    let q = |p: &Path| systemd_quote(&p.to_string_lossy());
    format!(
        "# Written by `cyxcloud node service install`; remove with `cyxcloud node service uninstall`.\n\
         [Unit]\n\
         Description=CyxCloud storage node\n\
         After=network-online.target\n\
         Wants=network-online.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         WorkingDirectory={workdir}\n\
         ExecStart={bin} --config {config}\n\
         Restart=on-failure\n\
         RestartSec=10\n\
         StandardOutput=append:{log}\n\
         StandardError=append:{log}\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n",
        workdir = q(workdir),
        bin = q(node_bin),
        config = q(config),
        log = log.to_string_lossy(),
    )
}

/// systemd command-line quoting: plain when safe, else double quotes.
fn systemd_quote(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-+:@".contains(c))
    {
        s.to_string()
    } else {
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

/// The launchd agent for the node.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))] // used on macOS; tested everywhere
pub fn launchd_plist(node_bin: &Path, config: &Path, log: &Path, workdir: &Path) -> String {
    let x = |p: &Path| xml_escape(&p.to_string_lossy());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<!-- Written by `cyxcloud node service install`; remove with `cyxcloud node service uninstall`. -->
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{label}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{bin}</string>
    <string>--config</string>
    <string>{config}</string>
  </array>
  <key>WorkingDirectory</key>
  <string>{workdir}</string>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <dict>
    <key>SuccessfulExit</key>
    <false/>
  </dict>
  <key>ThrottleInterval</key>
  <integer>10</integer>
  <key>StandardOutPath</key>
  <string>{log}</string>
  <key>StandardErrorPath</key>
  <string>{log}</string>
</dict>
</plist>
"#,
        label = LAUNCHD_LABEL,
        bin = x(node_bin),
        config = x(config),
        workdir = x(workdir),
        log = x(log),
    )
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// The Run-key command that starts the node at logon.
pub fn run_key_command(cyxcloud_exe: &Path) -> String {
    format!("\"{}\" node start", cyxcloud_exe.display())
}

/// `reg` arguments that set the Run-key value.
pub fn reg_add_args(cyxcloud_exe: &Path) -> Vec<String> {
    vec![
        "add".into(),
        WINDOWS_RUN_KEY.into(),
        "/v".into(),
        WINDOWS_RUN_VALUE.into(),
        "/t".into(),
        "REG_SZ".into(),
        "/d".into(),
        run_key_command(cyxcloud_exe),
        "/f".into(),
    ]
}

/// An absolute path for `program`: as given if absolute, else searched on PATH.
pub fn resolve_program(program: &Path, path_var: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    if program.is_absolute() {
        return program.exists().then(|| program.to_path_buf());
    }
    let path_var = path_var?;
    let exts: Vec<String> = if cfg!(windows) {
        vec!["".into(), ".exe".into()]
    } else {
        vec!["".into()]
    };
    for dir in std::env::split_paths(path_var) {
        for ext in &exts {
            let candidate = dir.join(format!("{}{}", program.display(), ext));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

// ============================================================================
// Which service is installed
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Installed {
    Systemd,
    Launchd,
    WindowsRunKey,
}

fn systemd_unit_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("systemd").join("user").join(SYSTEMD_UNIT))
}

fn launchd_plist_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| {
        h.join("Library")
            .join("LaunchAgents")
            .join(format!("{}.plist", LAUNCHD_LABEL))
    })
}

/// The start-at-boot service installed for this user, if any.
pub fn installed() -> Option<Installed> {
    if cfg!(target_os = "linux") {
        systemd_unit_path()
            .filter(|p| p.exists())
            .map(|_| Installed::Systemd)
    } else if cfg!(target_os = "macos") {
        launchd_plist_path()
            .filter(|p| p.exists())
            .map(|_| Installed::Launchd)
    } else if cfg!(windows) {
        std::process::Command::new("reg")
            .args(["query", WINDOWS_RUN_KEY, "/v", WINDOWS_RUN_VALUE])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .ok()
            .filter(|s| s.success())
            .map(|_| Installed::WindowsRunKey)
    } else {
        None
    }
}

fn run(program: &str, args: &[&str]) -> Result<std::process::Output> {
    std::process::Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("cannot run {}", program))
}

fn run_ok(program: &str, args: &[&str]) -> Result<()> {
    let out = run(program, args)?;
    if !out.status.success() {
        bail!(
            "{} {} failed: {}",
            program,
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

#[cfg(unix)]
fn uid() -> String {
    run("id", &["-u"])
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

// ============================================================================
// Commands
// ============================================================================

/// `cyxcloud node service install`
pub async fn install() -> Result<()> {
    let config = node::node_config_path()?;
    if !config.exists() {
        bail!("no node is set up on this machine: run `cyxcloud node init` first");
    }
    let workdir = node::node_dir()?;
    let log = node::log_path()?;
    let node_bin = resolve_program(&node::node_binary(), std::env::var_os("PATH").as_deref())
        .context(
        "cyxcloud-node was not found next to cyxcloud or on the PATH (or set CYXCLOUD_NODE_BIN)",
    )?;

    // Where the service manager runs the node itself, a node started by hand
    // would hold the port it wants
    if (cfg!(target_os = "linux") || cfg!(target_os = "macos"))
        && node::read_pid().is_some_and(node::is_our_node)
    {
        node::stop().await?;
    }

    if cfg!(target_os = "linux") {
        let unit = systemd_unit_path().context("no config directory for systemd user units")?;
        std::fs::create_dir_all(unit.parent().unwrap())?;
        std::fs::write(&unit, systemd_unit(&node_bin, &config, &log, &workdir))?;
        run_ok("systemctl", &["--user", "daemon-reload"])?;
        run_ok("systemctl", &["--user", "enable", "--now", SYSTEMD_UNIT])?;
        let user = std::env::var("USER").unwrap_or_default();
        let linger = run("loginctl", &["enable-linger", &user])
            .map(|o| o.status.success())
            .unwrap_or(false);
        println!(
            "{} systemd user unit {} installed and started (restarts on failure)",
            style(symbols::CHECK).green(),
            SYSTEMD_UNIT
        );
        if linger {
            println!(
                "  starts at boot, before anyone logs in (lingering enabled for {})",
                user
            );
        } else {
            println!(
                "{} starts when {} logs in; to start at boot run: sudo loginctl enable-linger {}",
                style("!").yellow(),
                user,
                user
            );
        }
    } else if cfg!(target_os = "macos") {
        #[cfg(unix)]
        {
            let plist = launchd_plist_path().context("no home directory")?;
            std::fs::create_dir_all(plist.parent().unwrap())?;
            std::fs::write(&plist, launchd_plist(&node_bin, &config, &log, &workdir))?;
            let domain = format!("gui/{}", uid());
            let _ = run(
                "launchctl",
                &["bootout", &format!("{}/{}", domain, LAUNCHD_LABEL)],
            );
            run_ok(
                "launchctl",
                &["bootstrap", &domain, &plist.to_string_lossy()],
            )?;
            println!(
                "{} launchd agent {} installed and started (restarts on failure, starts at login)",
                style(symbols::CHECK).green(),
                LAUNCHD_LABEL
            );
        }
    } else if cfg!(windows) {
        let exe = std::env::current_exe().context("cannot find this cyxcloud executable")?;
        let args = reg_add_args(&exe);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        run_ok("reg", &args)?;
        node::start(false).await?;
        println!(
            "{} The node starts when you log on (Run key \"{}\")",
            style(symbols::CHECK).green(),
            WINDOWS_RUN_VALUE
        );
        let port = std::fs::read_to_string(&config)
            .ok()
            .and_then(|t| toml::from_str::<toml::Value>(&t).ok())
            .and_then(|v| v.get("network")?.get("grpc_port")?.as_integer())
            .unwrap_or(node::DEFAULT_GRPC_PORT as i64);
        println!(
            "  If other machines cannot reach it, open the port as administrator:\n  netsh advfirewall firewall add rule name=\"CyxCloud Node\" dir=in action=allow protocol=TCP localport={}",
            port
        );
    } else {
        bail!("starting at boot is not supported on this OS yet; use `cyxcloud node start`");
    }
    Ok(())
}

/// `cyxcloud node service uninstall`
pub async fn uninstall() -> Result<()> {
    match installed() {
        None => {
            println!(
                "{} No start-at-boot service is installed",
                style("!").yellow()
            );
        }
        Some(Installed::Systemd) => {
            let _ = run("systemctl", &["--user", "disable", "--now", SYSTEMD_UNIT]);
            if let Some(unit) = systemd_unit_path() {
                std::fs::remove_file(&unit)?;
            }
            let _ = run("systemctl", &["--user", "daemon-reload"]);
            println!(
                "{} systemd user unit removed; the node is stopped",
                style(symbols::CHECK).green()
            );
        }
        Some(Installed::Launchd) => {
            #[cfg(unix)]
            {
                let _ = run(
                    "launchctl",
                    &["bootout", &format!("gui/{}/{}", uid(), LAUNCHD_LABEL)],
                );
            }
            if let Some(plist) = launchd_plist_path() {
                std::fs::remove_file(&plist)?;
            }
            println!(
                "{} launchd agent removed; the node is stopped",
                style(symbols::CHECK).green()
            );
        }
        Some(Installed::WindowsRunKey) => {
            run_ok(
                "reg",
                &["delete", WINDOWS_RUN_KEY, "/v", WINDOWS_RUN_VALUE, "/f"],
            )?;
            println!(
                "{} The node no longer starts at logon (the running node keeps running: `cyxcloud node stop`)",
                style(symbols::CHECK).green()
            );
        }
    }
    Ok(())
}

/// Start through the service manager when it owns the node (Linux, macOS).
/// Returns false when the caller should start the node itself.
pub fn start_via_service() -> Result<bool> {
    match installed() {
        Some(Installed::Systemd) => {
            run_ok("systemctl", &["--user", "start", SYSTEMD_UNIT])?;
            Ok(true)
        }
        Some(Installed::Launchd) => {
            #[cfg(unix)]
            {
                let domain = format!("gui/{}", uid());
                if let Some(plist) = launchd_plist_path() {
                    let _ = run(
                        "launchctl",
                        &["bootstrap", &domain, &plist.to_string_lossy()],
                    );
                }
                let _ = run(
                    "launchctl",
                    &["kickstart", &format!("{}/{}", domain, LAUNCHD_LABEL)],
                );
            }
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// Stop through the service manager when it owns the node (Linux, macOS).
/// Returns false when the caller should stop the node itself.
pub fn stop_via_service() -> Result<bool> {
    match installed() {
        Some(Installed::Systemd) => {
            run_ok("systemctl", &["--user", "stop", SYSTEMD_UNIT])?;
            Ok(true)
        }
        Some(Installed::Launchd) => {
            #[cfg(unix)]
            {
                let _ = run(
                    "launchctl",
                    &["bootout", &format!("gui/{}/{}", uid(), LAUNCHD_LABEL)],
                );
            }
            Ok(true)
        }
        _ => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_systemd_unit_runs_the_node_with_its_config_and_restarts_it() {
        let unit = systemd_unit(
            Path::new("/home/mrcj/.local/bin/cyxcloud-node"),
            Path::new("/home/mrcj/.cyxcloud/node/config.toml"),
            Path::new("/home/mrcj/.cyxcloud/node/node.log"),
            Path::new("/home/mrcj/.cyxcloud/node"),
        );
        assert!(unit.contains(
            "ExecStart=/home/mrcj/.local/bin/cyxcloud-node --config /home/mrcj/.cyxcloud/node/config.toml"
        ));
        assert!(unit.contains("Restart=on-failure"));
        assert!(unit.contains("StandardOutput=append:/home/mrcj/.cyxcloud/node/node.log"));
        assert!(unit.contains("WantedBy=default.target"));
        assert!(unit.contains("After=network-online.target"));
    }

    #[test]
    fn paths_with_spaces_are_quoted_for_systemd() {
        let unit = systemd_unit(
            Path::new("/opt/cyx cloud/cyxcloud-node"),
            Path::new("/home/a b/.cyxcloud/node/config.toml"),
            Path::new("/tmp/node.log"),
            Path::new("/home/a b/.cyxcloud/node"),
        );
        assert!(unit.contains(
            "ExecStart=\"/opt/cyx cloud/cyxcloud-node\" --config \"/home/a b/.cyxcloud/node/config.toml\""
        ));
    }

    #[test]
    fn the_launchd_plist_is_xml_with_the_arguments_and_keepalive() {
        let plist = launchd_plist(
            Path::new("/usr/local/bin/cyxcloud-node"),
            Path::new("/Users/mrcj/.cyxcloud/node/config.toml"),
            Path::new("/Users/mrcj/.cyxcloud/node/node.log"),
            Path::new("/Users/mrcj/.cyxcloud/node"),
        );
        assert!(plist.contains("<string>ai.cyxwiz.cyxcloud-node</string>"));
        assert!(plist.contains(
            "<string>/usr/local/bin/cyxcloud-node</string>\n    <string>--config</string>"
        ));
        assert!(plist.contains("<key>KeepAlive</key>"));
        assert!(plist.contains("<key>RunAtLoad</key>\n  <true/>"));
        assert_eq!(
            plist.matches("<dict>").count(),
            plist.matches("</dict>").count()
        );
    }

    #[test]
    fn xml_special_characters_are_escaped() {
        let plist = launchd_plist(
            Path::new("/Users/R&D/<bin>"),
            Path::new("/c"),
            Path::new("/l"),
            Path::new("/w"),
        );
        assert!(plist.contains("<string>/Users/R&amp;D/&lt;bin&gt;</string>"));
    }

    #[test]
    fn the_windows_run_key_runs_node_start_at_logon() {
        let exe = Path::new(r"C:\Program Files\CyxCloud\cyxcloud.exe");
        assert_eq!(
            run_key_command(exe),
            r#""C:\Program Files\CyxCloud\cyxcloud.exe" node start"#
        );
        let args = reg_add_args(exe);
        assert_eq!(args[0], "add");
        assert_eq!(args[1], WINDOWS_RUN_KEY);
        assert!(args
            .windows(2)
            .any(|w| w[0] == "/v" && w[1] == WINDOWS_RUN_VALUE));
        assert!(args
            .windows(2)
            .any(|w| w[0] == "/d" && w[1] == run_key_command(exe)));
        assert!(
            WINDOWS_RUN_KEY.starts_with("HKCU"),
            "per user, no administrator rights"
        );
    }

    #[test]
    fn programs_are_found_on_the_path() {
        let dir = tempfile::TempDir::new().unwrap();
        let name = if cfg!(windows) {
            "cyxcloud-node.exe"
        } else {
            "cyxcloud-node"
        };
        std::fs::write(dir.path().join(name), b"").unwrap();
        let path_var = std::env::join_paths([dir.path()]).unwrap();

        let found = resolve_program(Path::new("cyxcloud-node"), Some(&path_var)).unwrap();

        assert_eq!(found, dir.path().join(name));
        assert!(resolve_program(Path::new("not-there"), Some(&path_var)).is_none());
    }
}
