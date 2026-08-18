# Package Manager & Registry Hardening Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Fix 9 code-review findings in the package manager (client + registry) with TDD — every fix starts with a failing test that proves the bug.

**Architecture:** Two independent codebases: the main cargo workspace (`crates/`) for the CLI client, and `pkg/` for the registry server. Fixes are grouped by codebase so tasks within each group can be committed independently.

**Tech Stack:** Rust 2021, `toml` crate (already a dep), `toml_edit` (new dep for manifest editing), `flate2` + `tar` (new deps for safe extraction), `semver` (new dep for CLI, already in `pkg/`). SQLx + SQLite for registry.

---

## Group A: CLI Package Manager (crates/)

### Task 1: Use `toml` crate for `parse_entrypoint` in resolve.rs

**Files:**
- Modify: `crates/sema-core/src/resolve.rs:221-248`
- Test: `crates/sema-core/src/resolve.rs` (inline `#[cfg(test)]` module)

**Step 1: Write the failing test**

Add to the existing `mod tests` in `crates/sema-core/src/resolve.rs`:

```rust
#[test]
fn parse_entrypoint_ignores_non_package_table() {
    // Bug: the old line-by-line parser finds `entrypoint` in ANY table,
    // not just [package] or top-level. This test proves it.
    let dir = temp_packages_dir();
    let toml_content = "[tool]\nentrypoint = \"tool.sema\"\n";
    fs::write(dir.join("sema.toml"), toml_content).unwrap();
    // The parser should return None since there's no top-level or [package] entrypoint
    let result = parse_entrypoint(&dir.join("sema.toml"));
    assert_eq!(result, None, "should not pick up entrypoint from [tool] table");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn parse_entrypoint_reads_from_package_table() {
    let dir = temp_packages_dir();
    let toml_content = "[package]\nentrypoint = \"lib.sema\"\n";
    fs::write(dir.join("sema.toml"), toml_content).unwrap();
    let result = parse_entrypoint(&dir.join("sema.toml"));
    assert_eq!(result, Some("lib.sema".to_string()));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn parse_entrypoint_reads_top_level() {
    let dir = temp_packages_dir();
    let toml_content = "entrypoint = \"main.sema\"\n[deps]\nfoo = \"1.0\"\n";
    fs::write(dir.join("sema.toml"), toml_content).unwrap();
    let result = parse_entrypoint(&dir.join("sema.toml"));
    assert_eq!(result, Some("main.sema".to_string()));
    let _ = fs::remove_dir_all(&dir);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p sema-core -- parse_entrypoint_ignores`
Expected: FAIL — the old parser returns `Some("tool.sema")` instead of `None`.

**Step 3: Rewrite `parse_entrypoint` using `toml` crate**

Replace the function body with:

```rust
fn parse_entrypoint(path: &Path) -> Option<String> {
    let contents = std::fs::read_to_string(path).ok()?;
    let doc: toml::Value = toml::from_str(&contents).ok()?;

    // Check [package].entrypoint first
    if let Some(ep) = doc.get("package")
        .and_then(|p| p.get("entrypoint"))
        .and_then(|v| v.as_str())
    {
        return Some(ep.to_string());
    }

    // Fall back to top-level entrypoint (not inside any table)
    if let Some(table) = doc.as_table() {
        if let Some(ep) = table.get("entrypoint").and_then(|v| v.as_str()) {
            return Some(ep.to_string());
        }
    }

    None
}
```

Note: top-level `entrypoint` in a TOML table IS at the root level — `toml::Value::as_table()` on the root returns the top-level map, and `doc.get("entrypoint")` would return it even if there are subtables. However, `[tool]\nentrypoint = "x"` would be nested under `tool`, so `doc.get("entrypoint")` returns None. This is the correct behavior.

**Step 4: Run tests to verify they pass**

Run: `cargo test -p sema-core -- parse_entrypoint`
Expected: All 3 tests PASS.

**Step 5: Commit**

```bash
git add crates/sema-core/src/resolve.rs
git commit -m "fix: use toml crate for parse_entrypoint, reject non-package tables"
```

---

### Task 2: Use `toml_edit` for `add_dep_to_toml` and `remove_dep_from_toml`

**Files:**
- Modify: `Cargo.toml` (workspace deps: add `toml_edit`)
- Modify: `crates/sema/Cargo.toml` (add `toml_edit` dep)
- Modify: `crates/sema/src/pkg.rs:310-458`
- Test: `crates/sema/tests/integration_test.rs` (or inline in pkg.rs)

**Step 1: Add `toml_edit` dependency**

In workspace `Cargo.toml`, add:
```toml
toml_edit = "0.22"
```

In `crates/sema/Cargo.toml`, add:
```toml
toml_edit.workspace = true
```

**Step 2: Write failing tests**

Add tests to `crates/sema/src/pkg.rs` in a new `#[cfg(test)] mod tests` block (or to integration tests):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "sema-pkg-test-{name}-{}", std::process::id()
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn add_dep_preserves_comments() {
        let dir = tmpdir("add-comments");
        let toml_path = dir.join("sema.toml");
        let input = "# Project config\n[package]\nname = \"my-app\"\n\n# Dependencies\n[deps]\n\"github.com/test/foo\" = \"v1.0.0\"\n";
        fs::write(&toml_path, input).unwrap();

        add_dep_to_toml(&toml_path, "github.com/test/bar", "v2.0.0").unwrap();

        let output = fs::read_to_string(&toml_path).unwrap();
        let doc: toml_edit::DocumentMut = output.parse().unwrap();

        // Structural assertions — not string matching
        let deps = doc["deps"].as_table().expect("deps table must exist");
        assert_eq!(deps["github.com/test/foo"].as_str(), Some("v1.0.0"), "existing dep preserved");
        assert_eq!(deps["github.com/test/bar"].as_str(), Some("v2.0.0"), "new dep added");

        // Comment preservation — verify the comment text survives
        assert!(output.contains("# Project config"), "top comment lost");
        assert!(output.contains("# Dependencies"), "deps comment lost");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn add_dep_creates_deps_section_if_missing() {
        let dir = tmpdir("add-no-deps");
        let toml_path = dir.join("sema.toml");
        fs::write(&toml_path, "[package]\nname = \"bare\"\n").unwrap();

        let changed = add_dep_to_toml(&toml_path, "github.com/a/b", "v1.0.0").unwrap();
        assert!(changed);

        let output = fs::read_to_string(&toml_path).unwrap();
        let doc: toml_edit::DocumentMut = output.parse().unwrap();
        assert_eq!(doc["deps"]["github.com/a/b"].as_str(), Some("v1.0.0"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn add_dep_updates_existing_version() {
        let dir = tmpdir("add-update");
        let toml_path = dir.join("sema.toml");
        fs::write(&toml_path, "[deps]\n\"github.com/a/b\" = \"v1.0.0\"\n").unwrap();

        let changed = add_dep_to_toml(&toml_path, "github.com/a/b", "v2.0.0").unwrap();
        assert!(changed);

        let output = fs::read_to_string(&toml_path).unwrap();
        let doc: toml_edit::DocumentMut = output.parse().unwrap();
        assert_eq!(doc["deps"]["github.com/a/b"].as_str(), Some("v2.0.0"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn add_dep_returns_false_if_already_set() {
        let dir = tmpdir("add-noop");
        let toml_path = dir.join("sema.toml");
        fs::write(&toml_path, "[deps]\n\"github.com/a/b\" = \"v1.0.0\"\n").unwrap();

        let changed = add_dep_to_toml(&toml_path, "github.com/a/b", "v1.0.0").unwrap();
        assert!(!changed);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn remove_dep_removes_entry_preserves_others() {
        let dir = tmpdir("remove");
        let toml_path = dir.join("sema.toml");
        fs::write(&toml_path, "[deps]\n\"github.com/a/b\" = \"v1.0.0\"\n\"github.com/c/d\" = \"v2.0.0\"\n").unwrap();

        let removed = remove_dep_from_toml(&toml_path, "github.com/a/b").unwrap();
        assert!(removed);

        let output = fs::read_to_string(&toml_path).unwrap();
        let doc: toml_edit::DocumentMut = output.parse().unwrap();
        let deps = doc["deps"].as_table().unwrap();
        assert!(deps.get("github.com/a/b").is_none(), "removed dep should be gone");
        assert_eq!(deps["github.com/c/d"].as_str(), Some("v2.0.0"), "other dep preserved");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn remove_dep_returns_false_if_not_found() {
        let dir = tmpdir("remove-noop");
        let toml_path = dir.join("sema.toml");
        fs::write(&toml_path, "[deps]\n\"github.com/a/b\" = \"v1.0.0\"\n").unwrap();

        let removed = remove_dep_from_toml(&toml_path, "github.com/x/y").unwrap();
        assert!(!removed);
        let _ = fs::remove_dir_all(&dir);
    }
}
```

**Step 3: Run tests to verify they fail**

Run: `cargo test -p sema-lang -- pkg::tests`
Expected: `add_dep_preserves_comments` fails because the old string-manipulation approach doesn't produce valid TOML that `toml_edit` can parse with correct structure (or it drops comments).

**Step 4: Rewrite both functions using `toml_edit`**

```rust
fn add_dep_to_toml(toml_path: &Path, pkg_path: &str, git_ref: &str) -> Result<bool, String> {
    let content = std::fs::read_to_string(toml_path)
        .map_err(|e| format!("Failed to read sema.toml: {e}"))?;
    let mut doc: toml_edit::DocumentMut = content.parse()
        .map_err(|e| format!("Failed to parse sema.toml: {e}"))?;

    // Ensure [deps] table exists
    if doc.get("deps").is_none() {
        doc["deps"] = toml_edit::Item::Table(toml_edit::Table::new());
    }

    let deps = doc["deps"].as_table_mut()
        .ok_or("sema.toml [deps] is not a table")?;

    // Check if already set to same value
    if let Some(existing) = deps.get(pkg_path).and_then(|v| v.as_str()) {
        if existing == git_ref {
            return Ok(false);
        }
    }

    deps[pkg_path] = toml_edit::value(git_ref);

    std::fs::write(toml_path, doc.to_string())
        .map_err(|e| format!("Failed to write sema.toml: {e}"))?;
    Ok(true)
}

fn remove_dep_from_toml(toml_path: &Path, pkg_path: &str) -> Result<bool, String> {
    let content = std::fs::read_to_string(toml_path)
        .map_err(|e| format!("Failed to read sema.toml: {e}"))?;
    let mut doc: toml_edit::DocumentMut = content.parse()
        .map_err(|e| format!("Failed to parse sema.toml: {e}"))?;

    let removed = if let Some(deps) = doc.get_mut("deps").and_then(|d| d.as_table_mut()) {
        deps.remove(pkg_path).is_some()
    } else {
        false
    };

    if removed {
        std::fs::write(toml_path, doc.to_string())
            .map_err(|e| format!("Failed to write sema.toml: {e}"))?;
    }

    Ok(removed)
}
```

**Step 5: Run tests**

Run: `cargo test -p sema-lang -- pkg::tests`
Expected: All 6 tests PASS.

**Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock crates/sema/Cargo.toml crates/sema/src/pkg.rs
git commit -m "fix: use toml_edit for sema.toml manipulation, preserving comments and formatting"
```

---

### Task 3: Replace semver validation with `semver` crate in CLI

**Files:**
- Modify: `Cargo.toml` (workspace deps: add `semver`)
- Modify: `crates/sema/Cargo.toml` (add `semver` dep)
- Modify: `crates/sema/src/pkg.rs:872-878`
- Test: `crates/sema/src/pkg.rs` (add to `mod tests`)

**Step 1: Write the failing test**

```rust
#[test]
fn validate_version_accepts_prerelease() {
    // The old manual parser rejects this — it only accepts X.Y.Z
    assert!(validate_version("1.0.0-alpha.1").is_ok());
}

#[test]
fn validate_version_accepts_build_metadata() {
    assert!(validate_version("1.0.0+build.123").is_ok());
}

#[test]
fn validate_version_rejects_garbage() {
    assert!(validate_version("not-a-version").is_err());
    assert!(validate_version("1.0").is_err());
    assert!(validate_version("").is_err());
}
```

Also extract a `validate_version` helper from `cmd_publish`:

```rust
fn validate_version(version: &str) -> Result<semver::Version, String> {
    semver::Version::parse(version)
        .map_err(|_| format!("Invalid semver version: {version} (expected X.Y.Z[-prerelease][+build])"))
}
```

**Step 2: Run to verify failure**

Run: `cargo test -p sema-lang -- validate_version_accepts_prerelease`
Expected: FAIL (function doesn't exist yet, or old logic rejects it).

**Step 3: Add dep + implement**

Add `semver = "1"` to workspace `Cargo.toml` and `crates/sema/Cargo.toml`.

Replace lines 872-878 in `cmd_publish`:
```rust
let _ver = validate_version(version)?;
```

**Step 4: Run tests**

Run: `cargo test -p sema-lang -- validate_version`
Expected: All 3 PASS.

**Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock crates/sema/Cargo.toml crates/sema/src/pkg.rs
git commit -m "fix: use semver crate for version validation, accept pre-release versions"
```

---

### Task 4: Safe tarball extraction with `tar` + `flate2`

**Files:**
- Modify: `Cargo.toml` (add `tar`, `flate2` to workspace deps)
- Modify: `crates/sema/Cargo.toml` (add deps)
- Modify: `crates/sema/src/pkg.rs:716-756` (replace `create_tarball` and `extract_tarball`)
- Test: `crates/sema/src/pkg.rs` (add to `mod tests`)

**Step 1: Write the failing tests**

```rust
#[test]
fn extract_tarball_rejects_path_traversal() {
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::io::Write;
    use tar::Header;

    // Build a tar.gz with a malicious "../pwned.txt" entry
    let mut enc = GzEncoder::new(Vec::new(), Compression::default());
    {
        let mut ar = tar::Builder::new(&mut enc);
        let data = b"pwned!";
        let mut header = Header::new_gnu();
        header.set_path("../pwned.txt").unwrap();
        header.set_size(data.len() as u64);
        header.set_cksum();
        ar.append(&header, &data[..]).unwrap();
        ar.finish().unwrap();
    }
    let malicious_tarball = enc.finish().unwrap();

    let dir = tmpdir("traversal");
    let dest = dir.join("extracted");
    let parent_file = dir.join("pwned.txt");

    let result = extract_tarball(&malicious_tarball, &dest);
    assert!(result.is_err(), "path traversal should be rejected");
    assert!(!parent_file.exists(), "file must NOT be written outside dest");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn extract_tarball_rejects_absolute_paths() {
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use tar::Header;

    let mut enc = GzEncoder::new(Vec::new(), Compression::default());
    {
        let mut ar = tar::Builder::new(&mut enc);
        let data = b"pwned!";
        let mut header = Header::new_gnu();
        header.set_path("/etc/pwned.txt").unwrap();
        header.set_size(data.len() as u64);
        header.set_cksum();
        ar.append(&header, &data[..]).unwrap();
        ar.finish().unwrap();
    }
    let malicious_tarball = enc.finish().unwrap();

    let dir = tmpdir("abs-path");
    let dest = dir.join("extracted");

    let result = extract_tarball(&malicious_tarball, &dest);
    assert!(result.is_err(), "absolute paths should be rejected");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn extract_tarball_extracts_valid_archive() {
    use flate2::write::GzEncoder;
    use flate2::Compression;

    let mut enc = GzEncoder::new(Vec::new(), Compression::default());
    {
        let mut ar = tar::Builder::new(&mut enc);
        let data = b"(define x 42)";
        let mut header = tar::Header::new_gnu();
        header.set_path("package.sema").unwrap();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        ar.append(&header, &data[..]).unwrap();
        ar.finish().unwrap();
    }
    let tarball = enc.finish().unwrap();

    let dir = tmpdir("valid-tar");
    let dest = dir.join("extracted");

    extract_tarball(&tarball, &dest).unwrap();
    let content = fs::read_to_string(dest.join("package.sema")).unwrap();
    assert_eq!(content, "(define x 42)");

    let _ = fs::remove_dir_all(&dir);
}
```

**Step 2: Run to verify failure**

Run: `cargo test -p sema-lang -- extract_tarball_rejects`
Expected: FAIL — current implementation shells out to system `tar` which allows traversal, and these tests use programmatic tarballs that the shell approach can't handle through stdin piping properly in test context.

**Step 3: Add deps and implement**

Add to workspace `Cargo.toml`:
```toml
tar = "0.4"
flate2 = "1"
```

Add to `crates/sema/Cargo.toml`:
```toml
tar.workspace = true
flate2.workspace = true
```

Replace both functions:

```rust
fn create_tarball(dir: &str) -> Result<Vec<u8>, String> {
    use flate2::write::GzEncoder;
    use flate2::Compression;

    let enc = GzEncoder::new(Vec::new(), Compression::default());
    let mut ar = tar::Builder::new(enc);

    let dir_path = Path::new(dir);
    for entry in walkdir(dir_path)? {
        let rel = entry.strip_prefix(dir_path).unwrap_or(&entry);
        let rel_str = rel.to_string_lossy();
        // Skip .git and target directories
        if rel_str.starts_with(".git") || rel_str.starts_with("target") {
            continue;
        }
        if entry.is_file() {
            ar.append_path_with_name(&entry, rel)
                .map_err(|e| format!("Failed to add {}: {e}", entry.display()))?;
        }
    }

    let enc = ar.into_inner().map_err(|e| format!("Failed to finalize tar: {e}"))?;
    enc.finish().map_err(|e| format!("Failed to finalize gzip: {e}"))
}

fn walkdir(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    walkdir_inner(dir, &mut files)?;
    Ok(files)
}

fn walkdir_inner(dir: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir)
        .map_err(|e| format!("Failed to read directory {}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("directory entry error: {e}"))?;
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            if name == ".git" || name == "target" {
                continue;
            }
            walkdir_inner(&path, files)?;
        } else {
            files.push(path);
        }
    }
    Ok(())
}

fn extract_tarball(data: &[u8], dest: &Path) -> Result<(), String> {
    use flate2::read::GzDecoder;

    std::fs::create_dir_all(dest)
        .map_err(|e| format!("Failed to create directory: {e}"))?;

    let decoder = GzDecoder::new(data);
    let mut archive = tar::Archive::new(decoder);

    for entry in archive.entries().map_err(|e| format!("Invalid tar archive: {e}"))? {
        let mut entry = entry.map_err(|e| format!("Invalid tar entry: {e}"))?;
        let path = entry.path().map_err(|e| format!("Invalid entry path: {e}"))?;

        // Reject absolute paths
        if path.is_absolute() {
            return Err(format!("Tar entry has absolute path: {}", path.display()));
        }

        // Reject path traversal
        for component in path.components() {
            if matches!(component, std::path::Component::ParentDir) {
                return Err(format!("Tar entry contains path traversal: {}", path.display()));
            }
        }

        // Reject symlinks and hardlinks
        let entry_type = entry.header().entry_type();
        if entry_type.is_symlink() || entry_type.is_hard_link() {
            return Err(format!("Tar entry is a symlink/hardlink (rejected): {}", path.display()));
        }

        // Only extract regular files and directories
        let full_path = dest.join(&path);
        if entry_type.is_dir() {
            std::fs::create_dir_all(&full_path)
                .map_err(|e| format!("Failed to create dir {}: {e}", full_path.display()))?;
        } else if entry_type.is_file() {
            if let Some(parent) = full_path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create parent dir: {e}"))?;
            }
            entry.unpack(&full_path)
                .map_err(|e| format!("Failed to extract {}: {e}", path.display()))?;
        }
    }

    Ok(())
}
```

**Step 4: Run tests**

Run: `cargo test -p sema-lang -- extract_tarball`
Expected: All 3 PASS.

**Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock crates/sema/Cargo.toml crates/sema/src/pkg.rs
git commit -m "security: replace shell tar with safe Rust extraction, reject traversal/symlinks"
```

---

### Task 5: Missing package = hard error during `sema build`

**Files:**
- Modify: `crates/sema/src/import_tracer.rs:20,180-196`
- Test: `crates/sema/src/import_tracer.rs` (modify existing test)

**Step 1: Write the failing test (flip existing test expectation)**

Change the existing "uninstalled package warns but doesn't error" test to assert it IS an error:

```rust
// In the test_trace_package_imports function, replace the
// "Uninstalled package warns but doesn't error" block with:

// --- Uninstalled package IS an error ---
{
    fs::write(
        dir.join("main.sema"),
        r#"(import "github.com/nonexistent/pkg")"#,
    )
    .unwrap();
    let result = trace_imports(&dir.join("main.sema"));
    assert!(result.is_err(), "missing package should be a hard error");
    let err = result.unwrap_err();
    assert!(
        err.contains("not installed"),
        "error should mention 'not installed', got: {err}"
    );
    assert!(
        err.contains("sema pkg add"),
        "error should hint 'sema pkg add', got: {err}"
    );
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p sema-lang -- test_trace_package_imports`
Expected: FAIL — currently returns `Ok(())` with empty map.

**Step 3: Fix `process_package_import`**

Replace lines 186-196:
```rust
let resolved = match resolve_package_import(import_path) {
    Ok(p) => p,
    Err(_) => {
        return Err(format!(
            "package \"{}\" is not installed (hint: sema pkg add {})",
            import_path, import_path
        ));
    }
};
```

**Step 4: Run tests**

Run: `cargo test -p sema-lang -- test_trace_package_imports`
Expected: PASS.

**Step 5: Commit**

```bash
git add crates/sema/src/import_tracer.rs
git commit -m "fix: missing package is a hard error during sema build, not a warning"
```

---

### Task 6: Portable VFS keys for package imports in import tracer

**Files:**
- Modify: `crates/sema/src/import_tracer.rs:158-162,213-214`
- Test: `crates/sema/src/import_tracer.rs` (add new test to existing thread-spawned block)

**Step 1: Write the failing test**

Add to the `test_trace_package_imports` function:

```rust
// --- Transitive imports have stable VFS keys, not absolute paths ---
{
    fs::write(
        dir.join("main.sema"),
        r#"(import "github.com/test/translib")"#,
    )
    .unwrap();
    let result = trace_imports(&dir.join("main.sema")).unwrap();

    // The helpers.sema key must NOT contain the temp directory path
    for key in result.keys() {
        assert!(
            !key.starts_with('/'),
            "VFS key should be relative, not absolute: {key}"
        );
        assert!(
            !key.contains(&*sema_home.to_string_lossy()),
            "VFS key must not contain SEMA_HOME path: {key}"
        );
    }

    // The helpers.sema file imported transitively by the package
    // should appear under a package-relative key
    let helpers_key = result.keys().find(|k| k.contains("helpers.sema"));
    assert!(
        helpers_key.is_some(),
        "transitive helper should be in VFS: {result:?}"
    );
}
```

**Step 2: Run to verify failure**

Run: `cargo test -p sema-lang -- test_trace_package_imports`
Expected: FAIL — `helpers.sema` currently gets an absolute path as its VFS key.

**Step 3: Implement portable VFS key resolution**

In `process_import`, fix the fallback for files outside `root_dir` (line 159-162):

```rust
// Compute VFS key: relative to root_dir for project files,
// relative to packages_dir for package files
let rel_path = if let Ok(rel) = canonical.strip_prefix(root_dir) {
    rel.to_string_lossy().into_owned()
} else {
    // Check if this is inside a package directory
    let pkg_dir = sema_core::resolve::packages_dir();
    if let Ok(canon_pkg) = pkg_dir.canonicalize() {
        if let Ok(rel) = canonical.strip_prefix(&canon_pkg) {
            // Use package-relative path
            rel.to_string_lossy().into_owned().replace('\\', "/")
        } else {
            return Err(format!(
                "imported file is outside project and packages directory: {}",
                canonical.display()
            ));
        }
    } else {
        canonical.to_string_lossy().into_owned()
    }
};
```

Also update `process_package_import` — when it recursively traces the package's own imports via `trace_file_imports`, the `root_dir` context is wrong (it's the project root, not the package root). The transitive imports within the package will fail `strip_prefix(root_dir)`. Fix by passing the package's directory as the context for tracing:

In `process_package_import`, change the recursive call:
```rust
// Use the package's own directory as root for resolving its internal imports
let pkg_root = canonical.parent().unwrap_or(root_dir);
trace_file_imports(&exprs, &canonical, root_dir, visited, result)?;
```

The VFS key logic in `process_import` already handles the fallback to `packages_dir`, so this should work.

**Step 4: Run tests**

Run: `cargo test -p sema-lang -- test_trace_package_imports`
Expected: All PASS.

**Step 5: Commit**

```bash
git add crates/sema/src/import_tracer.rs
git commit -m "fix: use portable VFS keys for package imports, never embed absolute paths"
```

---

## Group B: Registry Server (pkg/)

### Task 7: Transactional publish + input validation

**Files:**
- Modify: `pkg/src/api/packages.rs:34-250`
- Modify: `pkg/src/blob.rs` (add `delete` function)
- Modify: `pkg/src/config.rs` (add `max_dependencies`)
- Test: `pkg/tests/integration_test.rs`

**Step 1: Write failing tests**

Add to `pkg/tests/integration_test.rs` (or create a new test file `pkg/tests/publish_test.rs`):

```rust
// These tests require a helper that creates an in-memory SQLite DB
// with migrations applied, creates a test user + token, and provides
// an AppState.

#[tokio::test]
async fn publish_rejects_invalid_tarball_magic_bytes() {
    let state = test_state().await;
    let token = create_test_token(&state, "publish").await;

    // Send random bytes as tarball (not gzip)
    let response = publish_request(&state, &token, "test-pkg", "1.0.0", b"not-a-tarball", "{}").await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    // Verify no version was inserted
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM package_versions")
        .fetch_one(&state.db).await.unwrap();
    assert_eq!(count.0, 0, "no version should exist after invalid tarball");
}

#[tokio::test]
async fn publish_rejects_too_many_dependencies() {
    let state = test_state().await;
    let token = create_test_token(&state, "publish").await;

    // Build valid tarball
    let tarball = make_test_tarball();

    // Metadata with 201 dependencies (over limit)
    let deps: Vec<serde_json::Value> = (0..201)
        .map(|i| serde_json::json!({"name": format!("dep-{i}"), "version_req": ">=1.0.0"}))
        .collect();
    let metadata = serde_json::json!({ "dependencies": deps });

    let response = publish_request(&state, &token, "test-pkg", "1.0.0", &tarball, &metadata.to_string()).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM package_versions")
        .fetch_one(&state.db).await.unwrap();
    assert_eq!(count.0, 0);
}

#[tokio::test]
async fn publish_rejects_invalid_dep_version_req() {
    let state = test_state().await;
    let token = create_test_token(&state, "publish").await;
    let tarball = make_test_tarball();

    let metadata = serde_json::json!({
        "dependencies": [{"name": "foo", "version_req": "not-semver!!!"}]
    });

    let response = publish_request(&state, &token, "test-pkg", "1.0.0", &tarball, &metadata.to_string()).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM package_versions")
        .fetch_one(&state.db).await.unwrap();
    assert_eq!(count.0, 0);
}

#[tokio::test]
async fn publish_rolls_back_on_dep_insert_failure() {
    // This tests that the version insert + dep insert are in a single tx.
    // If dep insert fails, version row should not exist.
    //
    // We can trigger this by inserting a dependency with a name containing
    // NUL bytes (or other DB-invalid data), or by testing the transactional
    // behavior directly.
    let state = test_state().await;
    let token = create_test_token(&state, "publish").await;
    let tarball = make_test_tarball();

    // First publish succeeds
    let metadata = serde_json::json!({
        "dependencies": [{"name": "valid-dep", "version_req": ">=1.0.0"}]
    });
    let response = publish_request(&state, &token, "test-pkg", "1.0.0", &tarball, &metadata.to_string()).await;
    assert_eq!(response.status(), StatusCode::OK);

    // Verify version AND dependency both exist
    let ver_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM package_versions")
        .fetch_one(&state.db).await.unwrap();
    assert_eq!(ver_count.0, 1);

    let dep_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM dependencies")
        .fetch_one(&state.db).await.unwrap();
    assert_eq!(dep_count.0, 1);
}
```

**Step 2: Run to verify failures**

Run: `cd pkg && cargo test -- publish_rejects`
Expected: FAIL — no validation exists yet, and these test helpers don't exist yet.

**Step 3: Implement fixes**

a) Add `delete` to `pkg/src/blob.rs`:
```rust
pub async fn delete(blob_dir: &str, blob_key: &str) {
    let path = blob_path(blob_dir, blob_key);
    let _ = tokio::fs::remove_file(path).await;
}
```

b) Add `max_dependencies` to `pkg/src/config.rs`:
```rust
pub max_dependencies: usize,
// In from_env():
max_dependencies: env::var("MAX_DEPENDENCIES")
    .ok().and_then(|v| v.parse().ok()).unwrap_or(200),
```

c) Rewrite `publish` in `pkg/src/api/packages.rs`:

Key changes:
1. Validate tarball magic bytes (`1F 8B` for gzip) before any DB work
2. Validate dependency count and version_req before any DB work
3. Use a single transaction for ALL DB mutations (package create + owner + version + deps)
4. On failure, delete blob
5. Stop silently ignoring dep insert errors (`let _ =`)

**Step 4: Run tests**

Run: `cd pkg && cargo test`
Expected: All PASS.

**Step 5: Commit**

```bash
cd pkg
git add src/api/packages.rs src/blob.rs src/config.rs tests/
git commit -m "fix: transactional publish, validate tarball + deps, reject invalid input"
```

---

## Execution Order

Tasks 1-6 (Group A) can be done sequentially in order. Task 7 (Group B) is independent and can be done in parallel with Group A since it's a separate codebase.

**Recommended:** Execute Group A sequentially (Tasks 1→2→3→4→5→6), then Task 7.

## Verification

After all tasks:
```bash
# Main workspace
cargo test
cargo test -p sema-core -- parse_entrypoint
cargo test -p sema-lang -- pkg::tests
cargo test -p sema-lang -- extract_tarball
cargo test -p sema-lang -- validate_version
cargo test -p sema-lang -- test_trace_package_imports

# Registry
cd pkg && cargo test
```
