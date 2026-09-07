//! Path resolution helpers shared by security and publication code.
//!
//! [`PathExt`] provides operations that resolve symlinks in the deepest
//! existing prefix. This is important for paths that do not exist yet: lexical
//! cleanup alone can miss a symlink in an existing parent directory.

use std::io;
use std::path::{Component, Path, PathBuf};

/// Remove `.` and resolve `..` without reading the filesystem.
fn normalize_lexical(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            other => result.push(other.as_os_str()),
        }
    }
    result
}

/// Filesystem-aware operations for [`Path`].
pub trait PathExt {
    /// Make this path absolute relative to the explicit `base` and normalize it.
    fn absolute_from(&self, base: &Path) -> io::Result<PathBuf>;

    /// Resolve every symlink in the deepest existing prefix and append a
    /// missing suffix lexically.
    fn resolve_allow_missing(&self) -> io::Result<PathBuf>;

    /// Return whether this path and `other` name the same destination.
    ///
    /// Existing files use filesystem identity, so hard links compare equal.
    /// Missing paths compare their resolved prefixes and lexical suffixes.
    fn is_same_destination_as(&self, other: &Path) -> io::Result<bool>;
}

impl PathExt for Path {
    fn absolute_from(&self, base: &Path) -> io::Result<PathBuf> {
        if !base.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("base path is not absolute: {}", base.display()),
            ));
        }
        let absolute = if self.is_absolute() {
            self.to_path_buf()
        } else {
            base.join(self)
        };
        Ok(normalize_lexical(&absolute))
    }

    fn resolve_allow_missing(&self) -> io::Result<PathBuf> {
        soft_canonicalize::soft_canonicalize(self)
    }

    fn is_same_destination_as(&self, other: &Path) -> io::Result<bool> {
        match (std::fs::metadata(self), std::fs::metadata(other)) {
            (Ok(_), Ok(_)) => same_file::is_same_file(self, other),
            (Err(error), _) if error.kind() != io::ErrorKind::NotFound => Err(error),
            (_, Err(error)) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(self.resolve_allow_missing()? == other.resolve_allow_missing()?),
        }
    }
}

/// A resolved root used to validate filesystem containment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathBoundary {
    root: PathBuf,
}

impl PathBoundary {
    /// Resolve a boundary root, including roots whose final components are missing.
    pub fn new(root: &Path) -> io::Result<Self> {
        Ok(Self {
            root: root.resolve_allow_missing()?,
        })
    }

    /// Return the resolved boundary root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolve `candidate` and return whether it remains below this boundary.
    /// Filesystem resolution failures remain errors rather than denials.
    pub fn contains(&self, candidate: &Path) -> io::Result<bool> {
        Ok(candidate.resolve_allow_missing()?.starts_with(&self.root))
    }

    /// Resolve `candidate` and verify that it remains below this boundary.
    pub fn resolve(&self, candidate: &Path) -> io::Result<PathBuf> {
        let candidate = candidate.resolve_allow_missing()?;
        if candidate.starts_with(&self.root) {
            Ok(candidate)
        } else {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "path {} is outside {}",
                    candidate.display(),
                    self.root.display()
                ),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_dot_dot_through_missing_directory() {
        let temp = tempfile::tempdir().unwrap();
        let resolved = temp
            .path()
            .join("missing/../file")
            .resolve_allow_missing()
            .unwrap();
        assert_eq!(resolved, temp.path().canonicalize().unwrap().join("file"));
    }

    #[test]
    fn detects_missing_destination_alias() {
        let temp = tempfile::tempdir().unwrap();
        assert!(temp
            .path()
            .join("new/../file")
            .is_same_destination_as(&temp.path().join("file"))
            .unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn detects_hard_links() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        std::fs::write(&first, b"data").unwrap();
        std::fs::hard_link(&first, &second).unwrap();
        assert!(first.is_same_destination_as(&second).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn containment_resolves_symlink_before_missing_suffix() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let allowed = temp.path().join("allowed");
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&allowed).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        symlink(&outside, allowed.join("link")).unwrap();
        let candidate = allowed.join("link/new/deep/file");
        let boundary = PathBoundary::new(&allowed).unwrap();
        let error = boundary.resolve(&candidate).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    }

    #[cfg(unix)]
    #[test]
    fn containment_resolves_parent_after_symlink_target() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let allowed = temp.path().join("allowed");
        let outside_parent = temp.path().join("outside");
        let outside_child = outside_parent.join("child");
        std::fs::create_dir_all(&allowed).unwrap();
        std::fs::create_dir_all(&outside_child).unwrap();
        symlink(&outside_child, allowed.join("link")).unwrap();
        let boundary = PathBoundary::new(&allowed).unwrap();

        let error = boundary
            .resolve(&allowed.join("link/../secret"))
            .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    }

    #[cfg(unix)]
    #[test]
    fn contains_preserves_candidate_resolution_errors() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let allowed = temp.path().join("allowed");
        let loop_path = allowed.join("loop");
        std::fs::create_dir_all(&allowed).unwrap();
        symlink(&loop_path, &loop_path).unwrap();
        let boundary = PathBoundary::new(&allowed).unwrap();

        assert_ne!(
            boundary.contains(&loop_path).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn preserves_indirect_proc_root_namespace() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let link = temp.path().join("process-root");
        symlink("/proc/self/root", &link).unwrap();

        assert_eq!(
            link.resolve_allow_missing().unwrap(),
            Path::new("/proc/self/root")
        );
    }

    #[test]
    fn boundary_accepts_missing_root_and_child() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("future-root");
        let boundary = PathBoundary::new(&root).unwrap();
        let expected_root = root.resolve_allow_missing().unwrap();
        assert_eq!(boundary.root(), expected_root);
        assert_eq!(
            boundary.resolve(&root.join("nested/file")).unwrap(),
            expected_root.join("nested/file")
        );
    }
}
