use std::path::{Path, PathBuf};

use crate::error::SemaError;
use crate::home::sema_home;

/// Returns the packages directory: `sema_home()/packages/`.
pub fn packages_dir() -> PathBuf {
    sema_home().join("packages")
}

/// Returns the immutable cache root for one registry tarball checksum.
///
/// A package lock records this checksum, so package imports can resolve the
/// exact package tree selected for the current project rather than whichever
/// version was installed most recently in the compatibility directory.
pub fn registry_cache_dir(checksum: &str) -> PathBuf {
    registry_cache_dir_in(&packages_dir(), checksum)
}

fn registry_cache_dir_in(base: &Path, checksum: &str) -> PathBuf {
    base.join(".store").join(format!("registry-{checksum}"))
}

/// Returns the immutable cache root for one git commit.
pub fn git_cache_dir(commit: &str) -> PathBuf {
    git_cache_dir_in(&packages_dir(), commit)
}

fn git_cache_dir_in(base: &Path, commit: &str) -> PathBuf {
    base.join(".store").join(format!("git-{commit}"))
}

/// Determines if an import spec is a package path vs a file path.
///
/// Package paths either:
/// - Contain `/` with a hostname-like first segment (e.g., `github.com/user/repo`)
/// - Are short names that exist in `~/.sema/packages/` (e.g., `http-helpers`)
///
/// Rejects relative paths (`./`, `../`), `.sema` extensions, absolute paths,
/// URLs with schemes (`://`), backslashes, and colons.
pub fn is_package_import(spec: &str) -> bool {
    if spec.starts_with("./")
        || spec.starts_with("../")
        || spec.ends_with(".sema")
        || spec.starts_with('/')
        || spec.contains("://")
        || spec.contains('\\')
        || spec.contains(':')
    {
        return false;
    }

    // Classic git-style: contains / (e.g., github.com/user/repo)
    if spec.contains('/') {
        return true;
    }

    // Registry-style short name: check if it exists in the packages directory
    packages_dir().join(spec).is_dir()
}

/// Validate that a package spec contains no path traversal or dangerous segments.
///
/// Rejects: `..` segments, empty segments, schemes, backslashes, colons, NUL bytes.
pub fn validate_package_spec(spec: &str) -> Result<(), SemaError> {
    if spec.contains("://") {
        return Err(SemaError::eval(format!(
            "invalid package spec: URL schemes not allowed: {spec}"
        ))
        .with_hint("Use bare host/path format, e.g.: github.com/user/repo"));
    }
    if spec.starts_with('/') {
        return Err(SemaError::eval(format!(
            "invalid package spec: absolute paths not allowed: {spec}"
        ))
        .with_hint("Use bare host/path format, e.g.: github.com/user/repo"));
    }
    if spec.contains('\\') {
        return Err(SemaError::eval(format!(
            "invalid package spec: backslashes not allowed: {spec}"
        )));
    }
    if spec.contains(':') {
        return Err(SemaError::eval(format!(
            "invalid package spec: colons not allowed: {spec}"
        )));
    }
    if spec.contains('\0') {
        return Err(SemaError::eval(
            "invalid package spec: NUL byte not allowed".to_string(),
        ));
    }
    for segment in spec.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err(SemaError::eval(format!(
                "invalid package spec: path traversal not allowed: {spec}"
            )));
        }
    }
    Ok(())
}

/// A validated package path (e.g., "github.com/user/repo").
///
/// Construction via `parse()` ensures the path has no traversal,
/// schemes, backslashes, colons, or empty segments.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PackagePath(String);

impl PackagePath {
    pub fn parse(s: &str) -> Result<Self, SemaError> {
        validate_package_spec(s)?;
        Ok(Self(s.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for PackagePath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A parsed package spec: validated path + git ref (e.g., "github.com/user/repo@v1.0").
///
/// The git ref defaults to "main" when no `@ref` suffix is present.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageSpec {
    pub path: PackagePath,
    pub git_ref: String,
}

impl PackageSpec {
    pub fn parse(spec: &str) -> Result<Self, SemaError> {
        let (path_str, git_ref) = if let Some((p, r)) = spec.rsplit_once('@') {
            (p, r)
        } else {
            (spec, "main")
        };

        let path = PackagePath::parse(path_str)?;

        if git_ref.is_empty() {
            return Err(
                SemaError::eval(format!("invalid package spec: empty git ref: {spec}"))
                    .with_hint("Provide a ref after @, e.g.: github.com/user/repo@v1.0"),
            );
        }
        if git_ref.contains('\0') {
            return Err(SemaError::eval(
                "invalid package spec: NUL byte in git ref".to_string(),
            ));
        }
        // BIN-2: a ref starting with '-' would be parsed by `git checkout` as a
        // flag (e.g. `-f`). Reject it here; `git checkout` has no safe `--`
        // separator for refs (that turns the ref into a pathspec).
        if git_ref.starts_with('-') {
            return Err(SemaError::eval(format!(
                "invalid package spec: git ref cannot start with '-': {git_ref}"
            )));
        }

        Ok(Self {
            path,
            git_ref: git_ref.to_string(),
        })
    }

    pub fn clone_url(&self) -> String {
        format!("https://{}.git", self.path.as_str())
    }

    pub fn dest_dir(&self, packages_dir: &Path) -> PathBuf {
        packages_dir.join(self.path.as_str())
    }
}

/// Resolves a package spec to a filesystem path.
///
/// Resolution order:
/// 1. `~/.sema/packages/<spec>.sema` (sub-module import)
/// 2. `~/.sema/packages/<spec>/sema.toml` → custom entrypoint
/// 3. `~/.sema/packages/<spec>/package.sema` (default entrypoint)
pub fn resolve_package_import(spec: &str) -> Result<PathBuf, SemaError> {
    resolve_package_import_in(spec, &packages_dir())
}

/// Resolve a package import using the nearest project lock when one exists.
///
/// The lock selects an immutable content-addressed package cache. This keeps
/// one project's imports independent from later installs in another project.
pub fn resolve_package_import_for_project(
    spec: &str,
    project_file: Option<&Path>,
) -> Result<PathBuf, SemaError> {
    resolve_package_import_for_project_in(spec, project_file, &packages_dir())
}

fn resolve_package_import_for_project_in(
    spec: &str,
    project_file: Option<&Path>,
    package_root: &Path,
) -> Result<PathBuf, SemaError> {
    validate_package_spec(spec)?;
    let Some(lock_path) = project_file.and_then(find_project_lock) else {
        return resolve_package_import_in(spec, package_root);
    };

    let content = std::fs::read_to_string(&lock_path).map_err(|error| {
        SemaError::Io(format!("Failed to read {}: {error}", lock_path.display()))
    })?;
    let lock: toml::Value = toml::from_str(&content).map_err(|error| {
        SemaError::Io(format!("Failed to parse {}: {error}", lock_path.display()))
    })?;
    let entry = lock
        .get("packages")
        .and_then(|packages| packages.get(spec))
        .and_then(toml::Value::as_table)
        .ok_or_else(|| {
            SemaError::eval(format!(
                "package '{spec}' is not recorded in {}",
                lock_path.display()
            ))
            .with_hint("Run `sema pkg install` to update the project lock.")
        })?;
    let source = entry
        .get("source")
        .and_then(toml::Value::as_str)
        .ok_or_else(|| SemaError::eval(format!("invalid lock entry for package '{spec}'")))?;
    let cache_root = match source {
        "registry" => {
            let checksum = entry
                .get("checksum")
                .and_then(toml::Value::as_str)
                .ok_or_else(|| {
                    SemaError::eval(format!("invalid lock entry for package '{spec}'"))
                })?;
            validate_lock_cache_id(checksum, "checksum", spec)?;
            registry_cache_dir_in(package_root, checksum)
        }
        "git" => {
            let commit = entry
                .get("commit")
                .and_then(toml::Value::as_str)
                .ok_or_else(|| {
                    SemaError::eval(format!("invalid lock entry for package '{spec}'"))
                })?;
            validate_lock_cache_id(commit, "commit", spec)?;
            git_cache_dir_in(package_root, commit)
        }
        other => {
            return Err(SemaError::eval(format!(
                "invalid lock source {other:?} for package '{spec}'"
            )));
        }
    };
    if !cache_root.join(spec).is_dir() {
        return Err(SemaError::eval(format!(
            "locked package '{spec}' is not installed in the package cache"
        ))
        .with_hint("Run `sema pkg install --locked` to install the locked package."));
    }
    resolve_package_import_in(spec, &cache_root)
}

fn validate_lock_cache_id(value: &str, field: &str, package: &str) -> Result<(), SemaError> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(SemaError::eval(format!(
            "invalid {field} in lock entry for package '{package}'"
        )));
    }
    Ok(())
}

fn find_project_lock(project_file: &Path) -> Option<PathBuf> {
    let mut directory = if project_file.is_dir() {
        project_file.to_path_buf()
    } else {
        project_file.parent()?.to_path_buf()
    };
    loop {
        let lock = directory.join("sema.lock");
        if lock.is_file() {
            return Some(lock);
        }
        if !directory.pop() {
            return None;
        }
    }
}

/// Resolves a package spec against a given packages directory.
pub fn resolve_package_import_in(spec: &str, base: &Path) -> Result<PathBuf, SemaError> {
    validate_package_spec(spec)?;

    // 1. Direct file: <packages>/<spec>.sema
    let direct = base.join(format!("{spec}.sema"));
    if direct.is_file() {
        verify_path_within(base, &direct)?;
        return Ok(direct);
    }

    let pkg_dir = base.join(spec);

    // 2. sema.toml with custom entrypoint
    let toml_path = pkg_dir.join("sema.toml");
    if toml_path.is_file() {
        if let Some(entrypoint) = parse_entrypoint(&toml_path)? {
            // Validate the entrypoint itself doesn't escape the package dir
            if entrypoint.contains("..") || entrypoint.starts_with('/') {
                return Err(SemaError::eval(format!(
                    "invalid entrypoint in {}: {entrypoint}",
                    toml_path.display()
                )));
            }
            let entry = pkg_dir.join(&entrypoint);
            if entry.is_file() {
                verify_path_within(base, &entry)?;
                return Ok(entry);
            }
        }
    }

    // 3. Default entrypoint: package.sema
    let mod_file = pkg_dir.join("package.sema");
    if mod_file.is_file() {
        verify_path_within(base, &mod_file)?;
        return Ok(mod_file);
    }

    Err(SemaError::eval(format!("package not found: {spec}"))
        .with_hint(format!("Run: sema pkg add {spec}")))
}

/// Verify that a resolved path stays within the expected base directory.
fn verify_path_within(base: &Path, resolved: &Path) -> Result<(), SemaError> {
    // Use canonicalize if both paths exist, otherwise check lexically
    if let (Ok(canon_base), Ok(canon_resolved)) = (base.canonicalize(), resolved.canonicalize()) {
        if !canon_resolved.starts_with(&canon_base) {
            return Err(SemaError::eval(
                "package path escapes packages directory".to_string(),
            ));
        }
    }
    Ok(())
}

/// Parse `entrypoint = "..."` from a sema.toml file.
///
/// Checks `[package].entrypoint` first, then falls back to a top-level `entrypoint` key.
/// Ignores `entrypoint` keys in any other table (e.g. `[tool]`).
fn parse_entrypoint(path: &Path) -> Result<Option<String>, SemaError> {
    let contents = std::fs::read_to_string(path)
        .map_err(|error| SemaError::Io(format!("Failed to read {}: {error}", path.display())))?;
    let doc: toml::Value = toml::from_str(&contents)
        .map_err(|error| SemaError::Io(format!("Failed to parse {}: {error}", path.display())))?;

    // Check [package].entrypoint first
    if let Some(ep) = doc
        .get("package")
        .and_then(|p| p.get("entrypoint"))
        .and_then(|v| v.as_str())
    {
        return Ok(Some(ep.to_string()));
    }

    // Fall back to top-level entrypoint
    if let Some(ep) = doc.get("entrypoint").and_then(|v| v.as_str()) {
        return Ok(Some(ep.to_string()));
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    /// Create a unique temp packages directory for testing.
    fn temp_packages_dir() -> PathBuf {
        let id = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("sema-resolve-test-{}-{}", std::process::id(), id));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    // --- is_package_import tests ---

    #[test]
    fn test_is_package_import_valid() {
        assert!(is_package_import("github.com/user/repo"));
        assert!(is_package_import("github.com/user/repo/sub"));
        assert!(is_package_import("gitlab.com/org/project"));
    }

    #[test]
    fn test_is_package_import_file_paths() {
        assert!(!is_package_import("./utils.sema"));
        assert!(!is_package_import("../lib/utils.sema"));
        assert!(!is_package_import("utils.sema"));
        assert!(!is_package_import("/absolute/path.sema"));
        assert!(!is_package_import("single-word"));
        assert!(!is_package_import("github.com/user/repo.sema"));
    }

    #[test]
    fn test_is_package_import_rejects_schemes() {
        assert!(!is_package_import("https://github.com/user/repo"));
        assert!(!is_package_import("http://example.com/pkg"));
        assert!(!is_package_import("ssh://git@github.com/user/repo"));
    }

    #[test]
    fn test_is_package_import_rejects_dangerous() {
        assert!(!is_package_import("github.com\\user\\repo")); // backslash
        assert!(!is_package_import("git@github.com:user/repo")); // colon (scp-style)
        assert!(!is_package_import("C:/Users/path")); // Windows drive
    }

    // --- validate_package_spec tests ---

    #[test]
    fn test_validate_spec_valid() {
        assert!(validate_package_spec("github.com/user/repo").is_ok());
        assert!(validate_package_spec("gitlab.com/org/project/sub").is_ok());
    }

    #[test]
    fn test_validate_spec_traversal() {
        assert!(validate_package_spec("github.com/../../etc/passwd").is_err());
        assert!(validate_package_spec("github.com/user/../../../etc").is_err());
        assert!(validate_package_spec("../escape").is_err());
        assert!(validate_package_spec("github.com/./user/repo").is_err());
    }

    #[test]
    fn test_validate_spec_empty_segments() {
        assert!(validate_package_spec("github.com//user/repo").is_err());
        assert!(validate_package_spec("/github.com/user").is_err());
    }

    #[test]
    fn test_validate_spec_schemes() {
        assert!(validate_package_spec("https://github.com/user/repo").is_err());
        assert!(validate_package_spec("ssh://git@host/repo").is_err());
    }

    #[test]
    fn test_validate_spec_dangerous_chars() {
        assert!(validate_package_spec("github.com\\user").is_err());
        assert!(validate_package_spec("git@github.com:user/repo").is_err());
    }

    // --- resolve_package_import_in tests ---

    #[test]
    fn test_resolve_direct_file() {
        let base = temp_packages_dir();
        let pkg_path = base.join("github.com/user");
        fs::create_dir_all(&pkg_path).unwrap();
        fs::write(pkg_path.join("repo.sema"), "(define x 1)").unwrap();

        let result = resolve_package_import_in("github.com/user/repo", &base).unwrap();
        assert_eq!(result, pkg_path.join("repo.sema"));
    }

    #[test]
    fn test_resolve_package_sema() {
        let base = temp_packages_dir();
        let pkg_dir = base.join("github.com/user/repo");
        fs::create_dir_all(&pkg_dir).unwrap();
        fs::write(pkg_dir.join("package.sema"), "(define x 1)").unwrap();

        let result = resolve_package_import_in("github.com/user/repo", &base).unwrap();
        assert_eq!(result, pkg_dir.join("package.sema"));
    }

    #[test]
    fn test_resolve_custom_entrypoint() {
        let base = temp_packages_dir();
        let pkg_dir = base.join("github.com/user/repo");
        fs::create_dir_all(&pkg_dir).unwrap();
        fs::write(pkg_dir.join("sema.toml"), "entrypoint = \"lib.sema\"\n").unwrap();
        fs::write(pkg_dir.join("lib.sema"), "(define x 1)").unwrap();

        let result = resolve_package_import_in("github.com/user/repo", &base).unwrap();
        assert_eq!(result, pkg_dir.join("lib.sema"));
    }

    #[test]
    fn test_resolve_custom_entrypoint_single_quotes() {
        let base = temp_packages_dir();
        let pkg_dir = base.join("github.com/user/repo");
        fs::create_dir_all(&pkg_dir).unwrap();
        fs::write(pkg_dir.join("sema.toml"), "entrypoint = 'main.sema'\n").unwrap();
        fs::write(pkg_dir.join("main.sema"), "(define x 1)").unwrap();

        let result = resolve_package_import_in("github.com/user/repo", &base).unwrap();
        assert_eq!(result, pkg_dir.join("main.sema"));
    }

    #[test]
    fn test_resolve_entrypoint_with_inline_comment() {
        let base = temp_packages_dir();
        let pkg_dir = base.join("github.com/user/repo");
        fs::create_dir_all(&pkg_dir).unwrap();
        fs::write(
            pkg_dir.join("sema.toml"),
            "entrypoint = \"lib.sema\" # the main entry\n",
        )
        .unwrap();
        fs::write(pkg_dir.join("lib.sema"), "(define x 1)").unwrap();

        let result = resolve_package_import_in("github.com/user/repo", &base).unwrap();
        assert_eq!(result, pkg_dir.join("lib.sema"));
    }

    #[test]
    fn test_resolve_entrypoint_traversal_rejected() {
        let base = temp_packages_dir();
        let pkg_dir = base.join("github.com/user/repo");
        fs::create_dir_all(&pkg_dir).unwrap();
        fs::write(
            pkg_dir.join("sema.toml"),
            "entrypoint = \"../../etc/passwd\"\n",
        )
        .unwrap();

        let err = resolve_package_import_in("github.com/user/repo", &base).unwrap_err();
        assert!(err.to_string().contains("invalid entrypoint"));
    }

    #[test]
    fn test_resolve_not_found() {
        let base = temp_packages_dir();
        let err = resolve_package_import_in("github.com/user/repo", &base).unwrap_err();
        assert!(err.to_string().contains("package not found"));
        assert_eq!(err.hint(), Some("Run: sema pkg add github.com/user/repo"));
    }

    #[test]
    fn test_resolve_traversal_rejected() {
        let base = temp_packages_dir();
        let err = resolve_package_import_in("github.com/../../etc/passwd", &base).unwrap_err();
        assert!(err.to_string().contains("path traversal"));
    }

    #[test]
    fn test_resolve_priority_direct_over_mod() {
        let base = temp_packages_dir();
        let parent = base.join("github.com/user");
        fs::create_dir_all(&parent).unwrap();
        fs::write(parent.join("repo.sema"), "direct").unwrap();

        let pkg_dir = parent.join("repo");
        fs::create_dir_all(&pkg_dir).unwrap();
        fs::write(pkg_dir.join("package.sema"), "pkg").unwrap();

        let result = resolve_package_import_in("github.com/user/repo", &base).unwrap();
        assert_eq!(result, parent.join("repo.sema"));
    }

    #[test]
    fn test_resolve_entrypoint_fallback_to_package_sema() {
        let base = temp_packages_dir();
        let pkg_dir = base.join("github.com/user/repo");
        fs::create_dir_all(&pkg_dir).unwrap();
        // sema.toml exists but entrypoint file doesn't
        fs::write(
            pkg_dir.join("sema.toml"),
            "entrypoint = \"nonexistent.sema\"\n",
        )
        .unwrap();
        fs::write(pkg_dir.join("package.sema"), "(define x 1)").unwrap();

        let result = resolve_package_import_in("github.com/user/repo", &base).unwrap();
        assert_eq!(result, pkg_dir.join("package.sema"));
    }

    #[test]
    fn test_resolve_rejects_malformed_package_manifest() {
        let base = temp_packages_dir();
        let pkg_dir = base.join("repo");
        fs::create_dir_all(&pkg_dir).unwrap();
        fs::write(pkg_dir.join("sema.toml"), "[package\nentrypoint = ").unwrap();
        fs::write(pkg_dir.join("package.sema"), "fallback").unwrap();

        let error = resolve_package_import_in("repo", &base).unwrap_err();
        assert!(
            error.to_string().contains("Failed to parse"),
            "got: {error}"
        );

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn test_project_lock_selects_its_immutable_registry_cache() {
        let root = temp_packages_dir();
        let project = root.join("project");
        fs::create_dir_all(project.join("src")).unwrap();
        fs::write(project.join("src/main.sema"), "(import \"foo\")").unwrap();
        fs::write(
            project.join("sema.lock"),
            "lock_version = 1\n\n[packages.foo]\nsource = \"registry\"\nversion = \"1.0.0\"\nregistry = \"https://registry.example\"\nchecksum = \"abc\"\n",
        )
        .unwrap();

        let locked = registry_cache_dir_in(&root, "abc").join("foo");
        fs::create_dir_all(&locked).unwrap();
        fs::write(locked.join("package.sema"), "locked").unwrap();
        let global = root.join("foo");
        fs::create_dir_all(&global).unwrap();
        fs::write(global.join("package.sema"), "newer-global").unwrap();

        let resolved = resolve_package_import_for_project_in(
            "foo",
            Some(&project.join("src/main.sema")),
            &root,
        )
        .unwrap();
        assert_eq!(resolved, locked.join("package.sema"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_project_lock_rejects_non_hex_cache_ids() {
        let root = temp_packages_dir();
        let project = root.join("project");
        fs::create_dir_all(project.join("src")).unwrap();
        let source_file = project.join("src/main.sema");
        fs::write(&source_file, "(import \"foo\")").unwrap();

        for (source, field) in [("registry", "checksum"), ("git", "commit")] {
            fs::write(
                project.join("sema.lock"),
                format!(
                    "lock_version = 1\n\n[packages.foo]\nsource = \"{source}\"\n{field} = \"../escape\"\n"
                ),
            )
            .unwrap();
            let error = resolve_package_import_for_project_in("foo", Some(&source_file), &root)
                .unwrap_err();
            assert!(error.to_string().contains(&format!("invalid {field}")));
        }

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_resolve_sema_toml_without_entrypoint_uses_package_sema() {
        let base = temp_packages_dir();
        let pkg_dir = base.join("github.com/user/repo");
        fs::create_dir_all(&pkg_dir).unwrap();
        // sema.toml exists but has no entrypoint key
        fs::write(
            pkg_dir.join("sema.toml"),
            "[package]\nname = \"repo\"\nversion = \"1.0\"\n",
        )
        .unwrap();
        fs::write(pkg_dir.join("package.sema"), "(define x 1)").unwrap();

        let result = resolve_package_import_in("github.com/user/repo", &base).unwrap();
        assert_eq!(result, pkg_dir.join("package.sema"));
    }

    // --- verify_path_within tests (symlink escape) ---

    #[cfg(unix)]
    #[test]
    fn test_resolve_package_sema_symlink_escape_rejected() {
        let base = temp_packages_dir();
        // Create a target file outside the packages directory
        let outside = base.parent().unwrap().join(format!(
            "sema-escape-target-{}",
            TEST_COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("package.sema"), "pwned").unwrap();

        // Create a symlink inside packages that points outside
        let pkg_dir = base.join("github.com/user/evil");
        fs::create_dir_all(pkg_dir.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&outside, &pkg_dir).unwrap();

        let err = resolve_package_import_in("github.com/user/evil", &base).unwrap_err();
        assert!(
            err.to_string().contains("escapes"),
            "expected escape error, got: {err}"
        );

        let _ = fs::remove_dir_all(&outside);
    }

    #[cfg(unix)]
    #[test]
    fn test_resolve_entrypoint_symlink_escape_rejected() {
        let base = temp_packages_dir();
        // Create a target file outside the packages directory
        let outside_file = base.parent().unwrap().join(format!(
            "sema-escape-entry-{}.sema",
            TEST_COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        fs::write(&outside_file, "pwned").unwrap();

        // Create a package with a sema.toml pointing to a symlinked file
        let pkg_dir = base.join("github.com/user/tricky");
        fs::create_dir_all(&pkg_dir).unwrap();
        fs::write(pkg_dir.join("sema.toml"), "entrypoint = \"entry.sema\"\n").unwrap();
        std::os::unix::fs::symlink(&outside_file, pkg_dir.join("entry.sema")).unwrap();

        let err = resolve_package_import_in("github.com/user/tricky", &base).unwrap_err();
        assert!(
            err.to_string().contains("escapes"),
            "expected escape error, got: {err}"
        );

        let _ = fs::remove_file(&outside_file);
    }

    // --- PackagePath tests ---

    #[test]
    fn test_package_path_valid() {
        let p = PackagePath::parse("github.com/user/repo").unwrap();
        assert_eq!(p.as_str(), "github.com/user/repo");
    }

    #[test]
    fn test_package_path_rejects_traversal() {
        assert!(PackagePath::parse("github.com/../../etc/passwd").is_err());
    }

    #[test]
    fn test_package_path_display() {
        let p = PackagePath::parse("github.com/user/repo").unwrap();
        assert_eq!(format!("{p}"), "github.com/user/repo");
    }

    // --- PackageSpec tests ---

    #[test]
    fn test_package_spec_with_ref() {
        let s = PackageSpec::parse("github.com/user/repo@v1.0").unwrap();
        assert_eq!(s.path.as_str(), "github.com/user/repo");
        assert_eq!(s.git_ref, "v1.0");
    }

    #[test]
    fn test_package_spec_no_ref_defaults_main() {
        let s = PackageSpec::parse("github.com/user/repo").unwrap();
        assert_eq!(s.git_ref, "main");
    }

    #[test]
    fn test_package_spec_clone_url() {
        let s = PackageSpec::parse("github.com/user/repo@v1.0").unwrap();
        assert_eq!(s.clone_url(), "https://github.com/user/repo.git");
    }

    #[test]
    fn test_package_spec_dest_dir() {
        let s = PackageSpec::parse("github.com/user/repo").unwrap();
        let base = PathBuf::from("/home/user/.sema/packages");
        assert_eq!(
            s.dest_dir(&base),
            PathBuf::from("/home/user/.sema/packages/github.com/user/repo")
        );
    }

    #[test]
    fn test_package_spec_rejects_empty_ref() {
        assert!(PackageSpec::parse("github.com/user/repo@").is_err());
    }

    #[test]
    fn test_package_spec_rejects_traversal_in_path() {
        assert!(PackageSpec::parse("github.com/../../etc/passwd@main").is_err());
    }

    #[test]
    fn parse_entrypoint_ignores_non_package_table() {
        let dir = temp_packages_dir();
        let toml_content = "[tool]\nentrypoint = \"tool.sema\"\n";
        fs::write(dir.join("sema.toml"), toml_content).unwrap();
        let result = parse_entrypoint(&dir.join("sema.toml")).unwrap();
        assert_eq!(
            result, None,
            "should not pick up entrypoint from [tool] table"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_entrypoint_reads_from_package_table() {
        let dir = temp_packages_dir();
        let toml_content = "[package]\nentrypoint = \"lib.sema\"\n";
        fs::write(dir.join("sema.toml"), toml_content).unwrap();
        let result = parse_entrypoint(&dir.join("sema.toml")).unwrap();
        assert_eq!(result, Some("lib.sema".to_string()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_entrypoint_reads_top_level() {
        let dir = temp_packages_dir();
        let toml_content = "entrypoint = \"main.sema\"\n[deps]\nfoo = \"1.0\"\n";
        fs::write(dir.join("sema.toml"), toml_content).unwrap();
        let result = parse_entrypoint(&dir.join("sema.toml")).unwrap();
        assert_eq!(result, Some("main.sema".to_string()));
        let _ = fs::remove_dir_all(&dir);
    }
}
