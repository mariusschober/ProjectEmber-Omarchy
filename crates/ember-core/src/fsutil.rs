use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Creates a directory owned by this user with mode 0700. Refuses symlinked
/// directories so a hostile home layout cannot redirect state writes.
pub fn ensure_private_dir(path: &Path) -> io::Result<()> {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("refusing symlinked state directory: {}", path.display()),
            ));
        }
        if !metadata.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                format!("state path is not a directory: {}", path.display()),
            ));
        }
    } else {
        fs::create_dir_all(path)?;
    }
    let metadata = fs::metadata(path)?;
    let mut permissions = metadata.permissions();
    if permissions.mode() & 0o777 != 0o700 {
        permissions.set_mode(0o700);
        fs::set_permissions(path, permissions)?;
    }
    Ok(())
}

/// Reads a regular file without following symlinks and with a hard size cap.
/// The cap is enforced from metadata and again while reading so a growing file
/// cannot exhaust memory.
pub fn read_limited(path: &Path, max_bytes: u64) -> io::Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("refusing to follow symlink: {}", path.display()),
        ));
    }
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("not a regular file: {}", path.display()),
        ));
    }
    if metadata.len() > max_bytes {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "file exceeds the permitted size",
        ));
    }
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    let mut buffer = Vec::with_capacity(metadata.len() as usize);
    std::io::Read::by_ref(&mut file)
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut buffer)?;
    if buffer.len() as u64 > max_bytes {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "file grew beyond the permitted size",
        ));
    }
    Ok(buffer)
}

/// Atomically writes `data` to `path` with mode 0600, fsyncing both file and
/// directory. The temporary file is created with `O_EXCL|O_NOFOLLOW` in the
/// destination directory, so a symlink planted at the destination is replaced
/// rather than followed.
pub fn atomic_write_private(path: &Path, data: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no parent"))?;
    ensure_private_dir(parent)?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?;
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temp = parent.join(format!(".{file_name}.tmp.{}.{counter}", std::process::id()));

    let result = (|| -> io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&temp)?;
        file.write_all(data)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, path)?;
        if let Ok(directory) = File::open(parent) {
            let _ = directory.sync_all();
        }
        Ok(())
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

/// Moves an unreadable file aside instead of deleting it. Returns the new path.
pub fn quarantine(path: &Path, label: &str) -> io::Result<PathBuf> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no parent"))?;
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("recovery");
    let stamp = chrono::Utc::now().timestamp();
    let mut candidate = parent.join(format!("{stem}.{label}-{stamp}.json"));
    let mut suffix = 1;
    while fs::symlink_metadata(&candidate).is_ok() {
        candidate = parent.join(format!("{stem}.{label}-{stamp}-{suffix}.json"));
        suffix += 1;
    }
    fs::rename(path, &candidate)?;
    if let Ok(directory) = File::open(parent) {
        let _ = directory.sync_all();
    }
    Ok(candidate)
}

/// Removes a file without following symlinks; a symlink itself is removed.
pub fn remove_file(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => fs::remove_file(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

pub fn file_exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

/// Reads the first `max` bytes of a file for backup comparisons.
pub fn read_prefix(path: &Path, max: usize) -> io::Result<Vec<u8>> {
    let mut file = File::open(path)?;
    let mut buffer = vec![0u8; max];
    let read = file.read(&mut buffer)?;
    buffer.truncate(read);
    Ok(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn atomic_write_creates_private_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state").join("file.json");
        atomic_write_private(&path, b"hello").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"hello");
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let dir_mode = fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(dir_mode, 0o700);
    }

    #[test]
    fn symlinked_file_is_replaced_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("victim.txt");
        fs::write(&target, b"do not touch").unwrap();
        let link = dir.path().join("state.json");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        atomic_write_private(&link, b"safe").unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"do not touch");
        assert_eq!(fs::read(&link).unwrap(), b"safe");
        assert!(!fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[test]
    fn read_limited_rejects_symlink_and_oversize() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        fs::write(&target, b"secret").unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(read_limited(&link, 1024).is_err());

        let big = dir.path().join("big");
        fs::write(&big, vec![b'x'; 2048]).unwrap();
        assert!(read_limited(&big, 1024).is_err());
        assert_eq!(read_limited(&big, 4096).unwrap().len(), 2048);
    }

    #[test]
    fn quarantine_preserves_contents() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal.json");
        let mut file = File::create(&path).unwrap();
        file.write_all(b"broken").unwrap();
        drop(file);
        let moved = quarantine(&path, "corrupt").unwrap();
        assert!(!path.exists());
        assert_eq!(fs::read(moved).unwrap(), b"broken");
    }
}
