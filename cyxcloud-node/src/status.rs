//! Filling the status report (cyxcloud_core::node_status) from this machine:
//! the OS, the CPU, memory and the disk that holds the data directory.

pub use cyxcloud_core::node_status::{MachineFacts, NodeStatusReport, ServerFacts, StorageFacts};

/// Read the machine facts from the OS.
pub fn detect_machine() -> MachineFacts {
    use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};
    let sys = System::new_with_specifics(
        RefreshKind::new()
            .with_cpu(CpuRefreshKind::everything())
            .with_memory(MemoryRefreshKind::everything()),
    );
    let threads = sys.cpus().len() as u32;
    let os = match (System::name(), System::os_version()) {
        (Some(name), Some(version)) => format!("{} {}", name.trim(), version.trim()),
        (Some(name), None) => name.trim().to_string(),
        _ => std::env::consts::OS.to_string(),
    };
    MachineFacts {
        hostname: hostname::get()
            .map(|h| h.to_string_lossy().to_string())
            .unwrap_or_default(),
        os,
        cpu_model: sys
            .cpus()
            .first()
            .map(|c| c.brand().trim().to_string())
            .unwrap_or_default(),
        cpu_cores: sys
            .physical_core_count()
            .map(|c| c as u32)
            .unwrap_or(threads),
        cpu_threads: threads,
        ram_bytes: sys.total_memory(),
    }
}

/// Free and total bytes on the disk holding `path` (0, 0 when unknown).
pub fn disk_space_for(path: &std::path::Path) -> (u64, u64) {
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let target = strip_verbatim(&target);
    let disks = sysinfo::Disks::new_with_refreshed_list();
    disks
        .list()
        .iter()
        .filter(|d| target.starts_with(d.mount_point()))
        .max_by_key(|d| d.mount_point().as_os_str().len())
        .map(|d| (d.available_space(), d.total_space()))
        .unwrap_or((0, 0))
}

/// `canonicalize` on Windows returns `\\?\D:\...`; mount points are `D:\`.
fn strip_verbatim(path: &std::path::Path) -> std::path::PathBuf {
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) => std::path::PathBuf::from(rest),
        None => path.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_machine_has_a_name_and_a_cpu() {
        let m = detect_machine();
        assert!(!m.hostname.is_empty());
        assert!(m.cpu_threads >= 1);
        assert!(m.cpu_cores >= 1);
        assert!(m.ram_bytes > 0);
    }

    #[test]
    fn the_disk_holding_the_temp_dir_has_space() {
        let (free, total) = disk_space_for(&std::env::temp_dir());
        assert!(total > 0, "no disk found for the temp dir");
        assert!(free <= total);
    }

    #[test]
    fn verbatim_windows_paths_are_plain_again() {
        assert_eq!(
            strip_verbatim(std::path::Path::new(r"\\?\D:\data")),
            std::path::PathBuf::from(r"D:\data")
        );
        assert_eq!(
            strip_verbatim(std::path::Path::new("/home/x")),
            std::path::PathBuf::from("/home/x")
        );
    }
}
