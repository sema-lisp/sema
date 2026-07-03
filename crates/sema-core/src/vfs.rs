use std::collections::HashMap;
use std::sync::OnceLock;

static EMBEDDED_VFS: OnceLock<HashMap<String, Vec<u8>>> = OnceLock::new();

/// Initialize the VFS with embedded files. Called once at startup for bundled binaries.
/// Panics if called more than once.
pub fn init_vfs(files: HashMap<String, Vec<u8>>) {
    EMBEDDED_VFS
        .set(files)
        .expect("VFS already initialized — init_vfs must only be called once");
}

/// Read a file from the VFS. Returns None if VFS is inactive or file not found.
pub fn vfs_read(path: &str) -> Option<Vec<u8>> {
    EMBEDDED_VFS.get()?.get(path).cloned()
}

/// Check if a file exists in the VFS. Returns None if VFS is inactive.
pub fn vfs_exists(path: &str) -> Option<bool> {
    let map = EMBEDDED_VFS.get()?;
    Some(map.contains_key(path))
}

/// Check if the VFS is active (has been initialized).
pub fn is_vfs_active() -> bool {
    EMBEDDED_VFS.get().is_some()
}

/// Resolve `path` to the canonical VFS key it matches, or `None`. Tries the path
/// as-is, then lexically normalized (resolving "./"/".."/interior "." — this
/// covers entry imports like `"./util.sema"` that have no `base_dir`), then
/// relative to `base_dir`. The returned key is the one identity used for reads,
/// caching, and `current_file`, so every spelling of the same module dedups.
pub fn vfs_resolve_key(path: &str, base_dir: Option<&str>) -> Option<String> {
    if vfs_read(path).is_some() {
        return Some(path.to_string());
    }
    if let Some(norm) = normalize_path(std::path::Path::new(path)) {
        if norm != path && vfs_read(&norm).is_some() {
            return Some(norm);
        }
    }
    if let Some(base) = base_dir {
        if let Some(norm) = normalize_path(&std::path::Path::new(base).join(path)) {
            if vfs_read(&norm).is_some() {
                return Some(norm);
            }
        }
    }
    None
}

/// Try to resolve a path against the VFS (see [`vfs_resolve_key`]) and read it.
pub fn vfs_resolve_and_read(path: &str, base_dir: Option<&str>) -> Option<Vec<u8>> {
    vfs_resolve_key(path, base_dir).and_then(|k| vfs_read(&k))
}

/// Normalize a path by resolving `.` and `..` components without hitting the filesystem.
/// Returns `None` if the path would traverse above the starting point (e.g., `../secret`
/// or `a/../../secret`), preventing cross-package reads in the VFS. This is the canonical
/// key form for embedded/VFS module lookups, matching the keys the import tracer emits.
pub fn normalize_path(path: &std::path::Path) -> Option<String> {
    let mut components = Vec::new();
    for comp in path.components() {
        match comp {
            std::path::Component::CurDir => {} // skip "."
            std::path::Component::ParentDir => {
                components.pop()?; // traversal above root → None
            }
            other => components.push(other.as_os_str().to_string_lossy().to_string()),
        }
    }
    Some(components.join("/"))
}

/// Validate a VFS path at build time. Rejects unsafe paths.
pub fn validate_vfs_path(path: &str) -> Result<(), String> {
    if path.is_empty() {
        return Err("empty VFS path".to_string());
    }
    if path.starts_with('/') || path.starts_with('\\') {
        return Err(format!("absolute path not allowed in VFS: {path}"));
    }
    if path.contains('\0') {
        return Err(format!("NUL byte in VFS path: {path}"));
    }
    if path.split('/').any(|seg| seg == "..") {
        return Err(format!("path traversal not allowed in VFS: {path}"));
    }
    // Reject Windows device names
    let stem = path
        .split('/')
        .next_back()
        .unwrap_or(path)
        .split('.')
        .next()
        .unwrap_or("");
    let upper = stem.to_uppercase();
    if matches!(
        upper.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "COM1" | "COM2" | "COM3" | "LPT1" | "LPT2" | "LPT3"
    ) {
        return Err(format!("reserved device name in VFS path: {path}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- validate_vfs_path --

    #[test]
    fn test_validate_empty_path() {
        assert!(validate_vfs_path("").is_err());
    }

    #[test]
    fn test_validate_absolute_unix() {
        assert!(validate_vfs_path("/etc/passwd").is_err());
    }

    #[test]
    fn test_validate_absolute_windows() {
        assert!(validate_vfs_path("\\windows\\system32").is_err());
    }

    #[test]
    fn test_validate_nul_byte() {
        assert!(validate_vfs_path("foo\0bar").is_err());
    }

    #[test]
    fn test_validate_dotdot() {
        assert!(validate_vfs_path("../etc/passwd").is_err());
        assert!(validate_vfs_path("foo/../bar").is_err());
        // Filenames that contain ".." as a substring but aren't traversal
        assert!(validate_vfs_path("foo..bar.sema").is_ok());
        assert!(validate_vfs_path("a..b/c.sema").is_ok());
    }

    #[test]
    fn test_validate_reserved_device_names() {
        for name in &[
            "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "LPT1", "LPT2", "LPT3",
        ] {
            assert!(
                validate_vfs_path(name).is_err(),
                "{name} should be rejected"
            );
            let with_ext = format!("{name}.txt");
            assert!(
                validate_vfs_path(&with_ext).is_err(),
                "{with_ext} should be rejected"
            );
        }
    }

    #[test]
    fn test_validate_ok_paths() {
        assert!(validate_vfs_path("lib/utils.sema").is_ok());
        assert!(validate_vfs_path("main.sema").is_ok());
        assert!(validate_vfs_path("data/config.json").is_ok());
    }

    // -- normalize_path --

    #[test]
    fn test_normalize_removes_cur_dir() {
        let p = std::path::Path::new("./foo/./bar");
        assert_eq!(normalize_path(p), Some("foo/bar".to_string()));
    }

    #[test]
    fn test_normalize_resolves_parent_dir() {
        let p = std::path::Path::new("foo/baz/../bar");
        assert_eq!(normalize_path(p), Some("foo/bar".to_string()));
    }

    #[test]
    fn test_normalize_simple_path() {
        let p = std::path::Path::new("lib/utils.sema");
        assert_eq!(normalize_path(p), Some("lib/utils.sema".to_string()));
    }

    #[test]
    fn test_normalize_rejects_traversal_past_root() {
        // "pkg/../../secret" should NOT normalize to "secret" — that would
        // allow cross-package reads in the VFS. normalize_path should return
        // None when traversal escapes above the starting point.
        let p = std::path::Path::new("pkg/../../secret");
        assert_eq!(
            normalize_path(p),
            None,
            "traversal past root must return None, not alias an unrelated key"
        );
    }

    #[test]
    fn test_normalize_rejects_leading_dotdot() {
        let p = std::path::Path::new("../etc/passwd");
        assert_eq!(normalize_path(p), None, "leading .. must return None");
    }

    #[test]
    fn test_normalize_allows_safe_dotdot() {
        // "a/b/../c" is fine — stays within the base
        let p = std::path::Path::new("a/b/../c");
        assert_eq!(normalize_path(p), Some("a/c".to_string()));
    }

    #[test]
    fn test_normalize_empty_path() {
        let p = std::path::Path::new("");
        assert_eq!(normalize_path(p), Some("".to_string()));
    }

    #[test]
    fn test_normalize_multi_component_traversal() {
        // With a multi-component base, .. can escape a "subroots" —
        // this is intentionally allowed by normalize_path (it only blocks
        // escaping above the *entire* joined path's starting point).
        let p = std::path::Path::new("github.com/a/lib/../../b/util");
        assert_eq!(normalize_path(p), Some("github.com/b/util".to_string()));
    }

    // -- VFS lifecycle --
    // VFS uses OnceLock (write-once per process), so all VFS state tests
    // must live in a single test to guarantee ordering: check inactive
    // state first, then initialize, then verify active state.

    #[test]
    fn test_vfs_lifecycle() {
        // Phase 1: Before init, VFS is inactive
        assert!(
            !is_vfs_active(),
            "VFS should be inactive before init_vfs is called"
        );
        assert_eq!(vfs_read("hello.sema"), None);
        assert_eq!(vfs_exists("hello.sema"), None);
        assert_eq!(vfs_resolve_and_read("hello.sema", None), None);

        // Phase 2: Initialize
        let mut files = HashMap::new();
        files.insert("hello.sema".to_string(), b"(+ 1 2)".to_vec());
        files.insert("lib/foo.sema".to_string(), b"data".to_vec());
        files.insert("exists.txt".to_string(), vec![]);

        init_vfs(files);

        // Phase 3: After init, VFS is active
        assert!(is_vfs_active());

        // Read existing key
        assert_eq!(vfs_read("hello.sema"), Some(b"(+ 1 2)".to_vec()));

        // Read missing key
        assert_eq!(vfs_read("missing.sema"), None);

        // Exists
        assert_eq!(vfs_exists("exists.txt"), Some(true));
        assert_eq!(vfs_exists("ghost.txt"), Some(false));

        // Resolve direct
        assert_eq!(
            vfs_resolve_and_read("lib/foo.sema", None),
            Some(b"data".to_vec())
        );

        // Resolve with base_dir
        assert_eq!(
            vfs_resolve_and_read("foo.sema", Some("lib")),
            Some(b"data".to_vec())
        );

        // Resolve miss
        assert_eq!(vfs_resolve_and_read("bar.sema", Some("other")), None);
        assert_eq!(vfs_resolve_and_read("missing.sema", None), None);

        // VFS is visible from child threads (process-global)
        let handle = std::thread::spawn(|| {
            assert!(is_vfs_active(), "VFS should be visible from child thread");
            assert_eq!(vfs_read("hello.sema"), Some(b"(+ 1 2)".to_vec()));
        });
        handle.join().unwrap();
    }
}
