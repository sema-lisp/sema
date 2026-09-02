//! Durable file replacement helpers for host applications.

use std::io::{self, Write};
use std::path::Path;

use crate::path::PathExt as _;

/// A staged replacement that is discarded unless explicitly committed.
pub struct AtomicFile {
    file: PlatformAtomicFile,
}

impl AtomicFile {
    /// Create an ordinary staged replacement beside `destination`.
    pub fn new(destination: &Path) -> io::Result<Self> {
        Self::create(destination, false)
    }

    /// Create a staged replacement with owner-only permissions on Unix.
    pub fn new_private(destination: &Path) -> io::Result<Self> {
        Self::create(destination, true)
    }

    fn create(destination: &Path, private: bool) -> io::Result<Self> {
        let destination = stable_destination(destination)?;
        let parent = destination_parent(&destination);
        std::fs::create_dir_all(parent)?;
        let file = open_file(&destination, parent, private)?;
        Ok(Self { file })
    }

    pub fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.file.write_all(bytes)
    }

    /// Flush the file, atomically replace the destination, and sync its parent.
    pub fn commit(self) -> io::Result<()> {
        self.file.commit()
    }

    /// Atomically replace `destination` with `bytes`.
    pub fn write(destination: &Path, bytes: &[u8]) -> io::Result<()> {
        let mut staged = Self::new(destination)?;
        staged.write_all(bytes)?;
        staged.commit()
    }

    /// Atomically replace the target of `destination` after resolving a final
    /// symlink and any symlinks in its existing prefix.
    ///
    /// Use this for ordinary file edits that must retain the caller's directory
    /// entry. Use [`Self::write`] when replacing that entry is intentional.
    pub fn write_through(destination: &Path, bytes: &[u8]) -> io::Result<()> {
        Self::write(&destination.resolve_allow_missing()?, bytes)
    }

    /// Atomically replace a sensitive file with owner-only permissions on Unix.
    pub fn write_private(destination: &Path, bytes: &[u8]) -> io::Result<()> {
        let mut staged = Self::new_private(destination)?;
        staged.write_all(bytes)?;
        staged.commit()
    }
}

fn stable_destination(destination: &Path) -> io::Result<std::path::PathBuf> {
    if destination.is_absolute() {
        Ok(destination.to_path_buf())
    } else {
        destination.absolute_from(&std::env::current_dir()?)
    }
}

fn destination_parent(destination: &Path) -> &Path {
    destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

#[cfg(not(windows))]
type PlatformAtomicFile = atomic_write_file::AtomicWriteFile;

#[cfg(not(windows))]
fn open_file(destination: &Path, _parent: &Path, private: bool) -> io::Result<PlatformAtomicFile> {
    open_options(private).open(destination)
}

#[cfg(unix)]
fn open_options(private: bool) -> atomic_write_file::OpenOptions {
    use atomic_write_file::unix::OpenOptionsExt as _;
    use std::os::unix::fs::OpenOptionsExt as _;

    let mut options = atomic_write_file::OpenOptions::new();
    if private {
        options
            .preserve_mode(false)
            .try_preserve_owner(false)
            .mode(0o600);
    }
    options
}

#[cfg(all(not(unix), not(windows)))]
fn open_options(_private: bool) -> atomic_write_file::OpenOptions {
    atomic_write_file::OpenOptions::new()
}

#[cfg(windows)]
struct PlatformAtomicFile {
    destination: std::path::PathBuf,
    staged: tempfile::NamedTempFile,
}

#[cfg(windows)]
fn open_file(destination: &Path, parent: &Path, private: bool) -> io::Result<PlatformAtomicFile> {
    let staged = tempfile::NamedTempFile::new_in(parent)?;
    if !private {
        if let Ok(metadata) = std::fs::metadata(destination) {
            staged.as_file().set_permissions(metadata.permissions())?;
        }
    }
    Ok(PlatformAtomicFile {
        destination: destination.to_path_buf(),
        staged,
    })
}

#[cfg(windows)]
impl Write for PlatformAtomicFile {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.staged.write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.staged.flush()
    }
}

#[cfg(windows)]
impl PlatformAtomicFile {
    fn commit(self) -> io::Result<()> {
        self.staged.as_file().sync_all()?;
        let (file, temporary) = self.staged.keep().map_err(io::Error::from)?;
        drop(file);
        let result = replace_file(&temporary, &self.destination);
        if result.is_err() {
            let _ = std::fs::remove_file(temporary);
        }
        result
    }
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    // SAFETY: Both buffers are NUL-terminated and remain alive for the call.
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_existing_file() {
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("output");
        std::fs::write(&output, b"old").unwrap();
        AtomicFile::write(&output, b"new").unwrap();
        assert_eq!(std::fs::read(output).unwrap(), b"new");
    }

    #[test]
    fn bare_destination_uses_current_directory() {
        assert_eq!(destination_parent(Path::new("sema.lock")), Path::new("."));
        let destination = stable_destination(Path::new("sema.lock")).unwrap();
        assert!(destination.is_absolute());
        assert_eq!(destination.file_name().unwrap(), "sema.lock");
    }

    #[test]
    fn failed_write_keeps_existing_file() {
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("output");
        std::fs::write(&output, b"old").unwrap();
        let mut staged = AtomicFile::new(&output).unwrap();
        staged.write_all(b"new").unwrap();
        drop(staged);
        assert_eq!(std::fs::read(output).unwrap(), b"old");
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
    }

    #[test]
    fn creates_missing_parent_directories() {
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("missing/parent/output");

        AtomicFile::write(&output, b"new").unwrap();

        assert_eq!(std::fs::read(output).unwrap(), b"new");
    }

    #[cfg(unix)]
    #[test]
    fn private_write_uses_owner_only_permissions() {
        use std::os::unix::fs::PermissionsExt as _;

        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("secret");
        AtomicFile::write_private(&output, b"secret").unwrap();
        assert_eq!(
            std::fs::metadata(output).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[cfg(unix)]
    #[test]
    fn private_write_replaces_permissive_mode() {
        use std::os::unix::fs::PermissionsExt as _;

        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("secret");
        std::fs::write(&output, b"old").unwrap();
        std::fs::set_permissions(&output, std::fs::Permissions::from_mode(0o644)).unwrap();

        AtomicFile::write_private(&output, b"secret").unwrap();

        assert_eq!(
            std::fs::metadata(output).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[cfg(unix)]
    #[test]
    fn ordinary_write_preserves_existing_mode() {
        use std::os::unix::fs::PermissionsExt as _;

        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("executable");
        std::fs::write(&output, b"old").unwrap();
        std::fs::set_permissions(&output, std::fs::Permissions::from_mode(0o755)).unwrap();

        AtomicFile::write(&output, b"new").unwrap();

        assert_eq!(
            std::fs::metadata(output).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }

    #[cfg(unix)]
    #[test]
    fn write_through_preserves_final_symlink() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target");
        let link = temp.path().join("link");
        std::fs::write(&target, b"old").unwrap();
        symlink(&target, &link).unwrap();

        AtomicFile::write_through(&link, b"new").unwrap();

        assert!(std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(std::fs::read(target).unwrap(), b"new");
    }

    #[cfg(unix)]
    #[test]
    fn write_through_creates_dangling_symlink_target() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target");
        let link = temp.path().join("link");
        symlink(&target, &link).unwrap();

        AtomicFile::write_through(&link, b"new").unwrap();

        assert!(std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(std::fs::read(target).unwrap(), b"new");
    }

    #[cfg(unix)]
    #[test]
    fn write_replaces_final_symlink_entry() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target");
        let link = temp.path().join("link");
        std::fs::write(&target, b"old").unwrap();
        symlink(&target, &link).unwrap();

        AtomicFile::write(&link, b"new").unwrap();

        assert!(!std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(std::fs::read(target).unwrap(), b"old");
        assert_eq!(std::fs::read(link).unwrap(), b"new");
    }
}
