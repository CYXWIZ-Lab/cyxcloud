//! Files that hold secrets (node key, access secret, sign-in tokens) are
//! readable by their owner only, on every OS (TOFIX177 principle: "owner-only
//! on every OS, Windows ACL too").

use std::path::Path;

/// Write `contents` to `path` (creating parent folders) and restrict it to the
/// current user.
pub fn write_owner_only(path: &Path, contents: impl AsRef<[u8]>) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, contents)?;
    restrict_to_owner(path)
}

/// Create `path` with `contents`, owner-only, only if it does not exist yet.
/// Returns false (and writes nothing) when the file already exists.
///
/// Atomic against a second process or thread doing the same: the contents are
/// written and restricted in a private temporary file first, which is then
/// hard-linked into place (the link fails if `path` exists), so nobody ever
/// reads a half-written or world-readable file.
pub fn create_owner_only_new(path: &Path, contents: impl AsRef<[u8]>) -> std::io::Result<bool> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if path.exists() {
        return Ok(false);
    }
    let mut nonce = [0u8; 8];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut nonce);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let tmp = path.with_file_name(format!(
        ".{}.{}.{}.tmp",
        name,
        std::process::id(),
        u64::from_le_bytes(nonce)
    ));
    let result = (|| {
        write_owner_only(&tmp, contents)?;
        match std::fs::hard_link(&tmp, path) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
            Err(e) => Err(e),
        }
    })();
    let _ = std::fs::remove_file(&tmp);
    result
}

/// Restrict an existing file to the current user.
///
/// Unix: mode 0600. Windows: inheritance removed and full control granted to
/// the current user only (`icacls`), so other accounts on the machine cannot
/// read it.
pub fn restrict_to_owner(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
    }
    #[cfg(windows)]
    {
        let user = std::env::var("USERNAME").map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "USERNAME is not set; cannot restrict the file to its owner",
            )
        })?;
        let status = std::process::Command::new("icacls")
            .arg(path)
            .arg("/inheritance:r")
            .arg("/grant:r")
            .arg(format!("{}:F", user))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(std::io::Error::other(format!(
                "icacls could not restrict {} to {}",
                path.display(),
                user
            )))
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_owner_only_file_is_written_and_readable_by_us() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("sub").join("secret");

        write_owner_only(&path, "abc").unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "abc");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[cfg(windows)]
    #[test]
    fn on_windows_only_the_current_user_keeps_access() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("secret");
        write_owner_only(&path, "abc").unwrap();

        let out = std::process::Command::new("icacls")
            .arg(&path)
            .output()
            .unwrap();
        let acl = String::from_utf8_lossy(&out.stdout).to_lowercase();
        let user = std::env::var("USERNAME").unwrap().to_lowercase();

        assert!(acl.contains(&user), "{acl}");
        assert!(!acl.contains("builtin\\users"), "{acl}");
        assert!(!acl.contains("everyone"), "{acl}");
    }
}
