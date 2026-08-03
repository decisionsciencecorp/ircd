//! Secret / history path permissions (H-13).

use std::fs;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

use anyhow::{bail, Context, Result};

/// True if mode bits allow group or other read/write/execute.
pub fn is_world_or_group_accessible(mode: u32) -> bool {
    mode & 0o077 != 0
}

/// Ensure `path` exists as a file with mode `0600`.
/// If it already exists and is group/world-accessible, fail closed.
pub fn ensure_private_file(path: &Path) -> Result<()> {
    if path.exists() {
        let meta = fs::metadata(path).with_context(|| format!("stat {}", path.display()))?;
        let mode = meta.permissions().mode() & 0o777;
        if is_world_or_group_accessible(mode) {
            bail!(
                "{} is group/world-accessible (mode {:o}); refuse to use as private key/db",
                path.display(),
                mode
            );
        }
        let mut perms = meta.permissions();
        perms.set_mode(0o600);
        fs::set_permissions(path, perms)
            .with_context(|| format!("chmod 0600 {}", path.display()))?;
    } else if let Some(parent) = path.parent() {
        ensure_private_dir(parent)?;
    }
    Ok(())
}

/// Ensure directory exists with mode `0700`.
pub fn ensure_private_dir(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty() {
        return Ok(());
    }
    if !path.exists() {
        fs::create_dir_all(path).with_context(|| format!("mkdir {}", path.display()))?;
    }
    let meta = fs::metadata(path).with_context(|| format!("stat {}", path.display()))?;
    let mut perms = meta.permissions();
    perms.set_mode(0o700);
    fs::set_permissions(path, perms).with_context(|| format!("chmod 0700 {}", path.display()))?;
    Ok(())
}

/// Create a new private file (0600) for writing secrets.
pub fn create_private_file(path: &Path) -> Result<fs::File> {
    if let Some(parent) = path.parent() {
        ensure_private_dir(parent)?;
    }
    fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("create {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn mode_bits() {
        assert!(!is_world_or_group_accessible(0o600));
        assert!(is_world_or_group_accessible(0o640));
        assert!(is_world_or_group_accessible(0o604));
    }

    #[test]
    fn refuse_group_readable_key() {
        let dir = tempdir().unwrap();
        let key = dir.path().join("key.pem");
        {
            let mut f = fs::File::create(&key).unwrap();
            f.write_all(b"secret").unwrap();
        }
        let mut perms = fs::metadata(&key).unwrap().permissions();
        perms.set_mode(0o644);
        fs::set_permissions(&key, perms).unwrap();
        assert!(ensure_private_file(&key).is_err());
    }

    #[test]
    fn tighten_private_file_and_dir() {
        let dir = tempdir().unwrap();
        let nested = dir.path().join("data");
        let db = nested.join("h.sqlite3");
        ensure_private_dir(&nested).unwrap();
        {
            let mut f = create_private_file(&db).unwrap();
            f.write_all(b"x").unwrap();
        }
        ensure_private_file(&db).unwrap();
        let mode = fs::metadata(&db).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let dmode = fs::metadata(&nested).unwrap().permissions().mode() & 0o777;
        assert_eq!(dmode, 0o700);
    }

    #[test]
    fn ensure_private_missing_file_makes_parent_and_empty_dir_ok() {
        assert!(ensure_private_dir(Path::new("")).is_ok());
        let dir = tempdir().unwrap();
        let nested = dir.path().join("nest").join("leaf.db");
        // File does not exist yet — should ensure parent dir only.
        ensure_private_file(&nested).unwrap();
        assert!(nested.parent().unwrap().is_dir());
        assert!(!nested.exists());
    }
}
