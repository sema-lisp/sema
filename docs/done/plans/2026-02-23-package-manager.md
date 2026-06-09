# Package Manager Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add a Go-style package manager to Sema — packages are git repos, installed to `~/.sema/packages/{url}`, imported via `(import "github.com/user/repo")`, managed via `sema pkg` CLI.

**Architecture:** Rust-first implementation. Import path resolution lives in `sema-core` (shared by evaluator + build tracer). The `sema pkg` CLI commands live in the `sema` binary crate as a `pkg.rs` module using `std::process::Command` for git. TOML parsing added to `sema-stdlib` (mirroring `json.rs`) and used in Rust for manifest handling. No new crates needed.

**Tech Stack:** Rust, `toml` crate (new workspace dep), `std::process::Command` for git, `sema-core::sema_home()` for path conventions.

**Ref:** [GitHub Issue #10](https://github.com/HelgeSverre/sema/issues/10)

---

## On-Disk Conventions

```
~/.sema/
  packages/
    github.com/
      helgesverre/
        sema-http/
          sema.toml       ← manifest
          mod.sema        ← default entrypoint
          router.sema
          ...
```

- **Package dir:** `sema_home()/packages/<url>` (e.g., `~/.sema/packages/github.com/user/repo`)
- **Import spec:** any import path containing `/` and NOT starting with `./` or `../` and NOT ending in `.sema` is a package path
- **Entrypoint:** `mod.sema` by default, overridable via `sema.toml` `[package] entrypoint`
- **Manifest:** `sema.toml` with `[package]` (name, version, description, entrypoint) and `[deps]` sections

```toml
# sema.toml
[package]
name = "my-app"
version = "1.0.0"

[deps]
sema-http = "github.com/helgesverre/sema-http@v0.3.0"
sema-test = "github.com/helgesverre/sema-test@v1.0.0"
```

---

## Task 1: Import Path Resolver in `sema-core`

**Files:**
- Create: `crates/sema-core/src/resolve.rs`
- Modify: `crates/sema-core/src/lib.rs` (add `pub mod resolve;` + re-export)

**Context:** The resolver must be in `sema-core` because both the evaluator (`sema-eval`) and the build tracer (`sema` binary's `import_tracer.rs`) need it. Neither should depend on each other.

**Step 1: Create `resolve.rs` with the package path resolver**

```rust
// crates/sema-core/src/resolve.rs
use std::path::{Path, PathBuf};
use crate::SemaError;

/// Returns the packages directory: `sema_home()/packages/`
pub fn packages_dir() -> PathBuf {
    crate::sema_home().join("packages")
}

/// Determines if an import spec is a package path (vs a file path).
///
/// Package paths contain `/` but don't start with `./` or `../`
/// and don't end with `.sema`. Examples:
///   - "github.com/user/repo" → package
///   - "./utils.sema" → file
///   - "utils.sema" → file
///   - "/absolute/path.sema" → file
pub fn is_package_import(spec: &str) -> bool {
    if spec.starts_with("./") || spec.starts_with("../") {
        return false;
    }
    if spec.ends_with(".sema") {
        return false;
    }
    if Path::new(spec).is_absolute() {
        return false;
    }
    // Must contain at least one slash (domain/user/repo pattern)
    spec.contains('/')
}

/// Resolve a package import spec to a filesystem path.
///
/// Given "github.com/user/repo", looks for:
/// 1. `~/.sema/packages/github.com/user/repo/sema.toml` → read entrypoint
/// 2. `~/.sema/packages/github.com/user/repo/mod.sema` → default entrypoint
///
/// For sub-module imports like "github.com/user/repo/sub", also tries:
/// 3. `~/.sema/packages/github.com/user/repo/sub.sema`
pub fn resolve_package_import(spec: &str) -> Result<PathBuf, SemaError> {
    let pkg_dir = packages_dir();
    let full_path = pkg_dir.join(spec);

    // Try as a direct .sema file (sub-module import)
    let as_file = full_path.with_extension("sema");
    if as_file.is_file() {
        return Ok(as_file);
    }

    // Try as a package directory
    if full_path.is_dir() {
        // Check for sema.toml with custom entrypoint
        let manifest = full_path.join("sema.toml");
        if manifest.is_file() {
            if let Some(entry) = read_entrypoint_from_manifest(&manifest) {
                let entry_path = full_path.join(entry);
                if entry_path.is_file() {
                    return Ok(entry_path);
                }
            }
        }

        // Default entrypoint: mod.sema
        let mod_sema = full_path.join("mod.sema");
        if mod_sema.is_file() {
            return Ok(mod_sema);
        }
    }

    Err(SemaError::eval(format!(
        "package not found: {spec}\n  Run: sema pkg get {spec}"
    )))
}

/// Read the entrypoint field from a sema.toml manifest.
/// Returns None if the file can't be read or doesn't have an entrypoint.
/// Uses simple line parsing to avoid requiring the toml crate in sema-core.
fn read_entrypoint_from_manifest(path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    // Simple parser: look for `entrypoint = "..."` line
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("entrypoint") {
            if let Some(val) = trimmed.split('=').nth(1) {
                let val = val.trim().trim_matches('"').trim_matches('\'');
                if !val.is_empty() {
                    return Some(val.to_string());
                }
            }
        }
    }
    None
}
```

**Step 2: Wire into `sema-core/src/lib.rs`**

Add to the module declarations:
```rust
pub mod resolve;
```

**Step 3: Write unit tests in `resolve.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_package_import() {
        assert!(is_package_import("github.com/user/repo"));
        assert!(is_package_import("github.com/user/repo/sub"));
        assert!(!is_package_import("./utils.sema"));
        assert!(!is_package_import("../lib.sema"));
        assert!(!is_package_import("utils.sema"));
        assert!(!is_package_import("/absolute/path.sema"));
        assert!(!is_package_import("single-word"));
    }

    #[test]
    fn test_packages_dir() {
        let dir = packages_dir();
        assert!(dir.to_string_lossy().contains("packages"));
    }

    #[test]
    fn test_resolve_package_mod_sema() {
        let tmp = std::env::temp_dir().join("sema-test-pkg-resolve");
        let pkg = tmp.join("packages/github.com/test/pkg");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(pkg.join("mod.sema"), "(define x 1)").unwrap();

        std::env::set_var("SEMA_HOME", tmp.to_str().unwrap());
        let result = resolve_package_import("github.com/test/pkg");
        std::env::remove_var("SEMA_HOME");
        std::fs::remove_dir_all(&tmp).ok();

        assert!(result.is_ok());
        assert!(result.unwrap().ends_with("mod.sema"));
    }

    #[test]
    fn test_resolve_package_not_found() {
        std::env::set_var("SEMA_HOME", "/tmp/sema-nonexistent-home");
        let result = resolve_package_import("github.com/no/such-pkg");
        std::env::remove_var("SEMA_HOME");

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("package not found"));
        assert!(err.contains("sema pkg get"));
    }
}
```

**Step 4: Run tests**

Run: `cargo test -p sema-core -- resolve`
Expected: PASS

**Step 5: Commit**

```bash
git add crates/sema-core/src/resolve.rs crates/sema-core/src/lib.rs
git commit -m "feat(core): add package import path resolver"
```

---

## Task 2: Wire Package Resolution into `eval_import`

**Files:**
- Modify: `crates/sema-eval/src/special_forms.rs:1287-1303` (the `eval_import` path resolution block)

**Context:** Currently `eval_import` resolves paths as relative to current file or absolute. We need to check `is_package_import()` first and resolve via `resolve_package_import()` before falling through to the existing relative/absolute path logic.

**Step 1: Modify path resolution in `eval_import`**

In `eval_import`, replace the path resolution block (lines ~1296-1303):

```rust
// Current code:
let resolved = if std::path::Path::new(path_str).is_absolute() {
    std::path::PathBuf::from(path_str)
} else if let Some(dir) = ctx.current_file_dir() {
    dir.join(path_str)
} else {
    std::path::PathBuf::from(path_str)
};
```

With:

```rust
let resolved = if sema_core::resolve::is_package_import(path_str) {
    sema_core::resolve::resolve_package_import(path_str)?
} else if std::path::Path::new(path_str).is_absolute() {
    std::path::PathBuf::from(path_str)
} else if let Some(dir) = ctx.current_file_dir() {
    dir.join(path_str)
} else {
    std::path::PathBuf::from(path_str)
};
```

**Step 2: Add integration test in `crates/sema/tests/integration_test.rs`**

```rust
#[test]
fn test_package_import() {
    // Set up a fake package in a temp dir
    let tmp = std::env::temp_dir().join("sema-test-pkg-import");
    let pkg_dir = tmp.join("packages/github.com/test/mylib");
    std::fs::create_dir_all(&pkg_dir).unwrap();
    std::fs::write(
        pkg_dir.join("mod.sema"),
        "(module mylib (export greet)) (define (greet name) (string-append \"Hello, \" name))",
    )
    .unwrap();

    std::env::set_var("SEMA_HOME", tmp.to_str().unwrap());
    let result = eval(
        r#"(begin
            (import "github.com/test/mylib")
            (greet "World"))"#,
    );
    std::env::remove_var("SEMA_HOME");
    std::fs::remove_dir_all(&tmp).ok();

    assert_eq!(result, Value::string("Hello, World"));
}
```

**Step 3: Run tests**

Run: `cargo test -p sema --test integration_test -- test_package_import`
Expected: PASS

**Step 4: Commit**

```bash
git add crates/sema-eval/src/special_forms.rs crates/sema/tests/integration_test.rs
git commit -m "feat(eval): support package imports in (import)"
```

---

## Task 3: Wire Package Resolution into `import_tracer`

**Files:**
- Modify: `crates/sema/src/import_tracer.rs:80-140` (the `extract_imports` path resolution)

**Context:** `sema build` uses `import_tracer.rs` to statically discover all imported files for bundling. It must use the same resolver so that package imports are bundled correctly into standalone executables.

**Step 1: Update `extract_imports` in `import_tracer.rs`**

Find where it resolves the import path (after extracting the string from the `import`/`load` form). Add a check for package imports before the existing relative path resolution:

```rust
// After extracting `path_str` from the import form:
let target = if sema_core::resolve::is_package_import(path_str) {
    match sema_core::resolve::resolve_package_import(path_str) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("warning: cannot resolve package import {path_str}: {e}");
            return Ok(());
        }
    }
} else {
    // existing relative/absolute resolution logic
    ...
};
```

Also update the VFS key computation so bundled package imports use forward-slash package paths (not absolute filesystem paths) as their key.

**Step 2: Run existing tracer tests**

Run: `cargo test -p sema -- import_tracer`
Expected: PASS (existing tests still work)

**Step 3: Commit**

```bash
git add crates/sema/src/import_tracer.rs
git commit -m "feat(build): support package imports in import tracer"
```

---

## Task 4: `sema pkg` CLI Subcommand

**Files:**
- Create: `crates/sema/src/pkg.rs`
- Modify: `crates/sema/src/main.rs` (add `mod pkg;`, add `Pkg` variant to `Commands`, wire dispatch)

**Context:** The `sema` binary uses clap with a `Commands` enum. We add a `Pkg` variant that delegates to `pkg.rs`. Git operations use `std::process::Command`. All operations check that `git` is available.

**Step 1: Add `Pkg` subcommand to `Commands` enum in `main.rs`**

```rust
/// Package manager
Pkg {
    #[command(subcommand)]
    command: PkgCommands,
},
```

Add the `PkgCommands` enum (can live in `main.rs` or `pkg.rs`):

```rust
#[derive(Subcommand)]
enum PkgCommands {
    /// Install a package from a git URL
    Get {
        /// Package URL, optionally with @ref (e.g., github.com/user/repo@v1.0)
        url: String,
    },
    /// Install all dependencies from sema.toml
    Install,
    /// Update a package (or all packages)
    Update {
        /// Package name to update (updates all if omitted)
        name: Option<String>,
    },
    /// Remove an installed package
    Remove {
        /// Package URL or name
        name: String,
    },
    /// List installed packages
    List,
    /// Initialize a new sema.toml in the current directory
    Init,
}
```

**Step 2: Create `crates/sema/src/pkg.rs` with implementations**

```rust
// crates/sema/src/pkg.rs
use std::path::{Path, PathBuf};
use std::process::Command;

use sema_core::resolve::packages_dir;

/// Parse a URL spec like "github.com/user/repo@v1.0" into (url, ref)
fn parse_url_spec(spec: &str) -> (&str, &str) {
    match spec.split_once('@') {
        Some((url, git_ref)) => (url, git_ref),
        None => (spec, "main"),
    }
}

pub fn cmd_get(spec: &str) -> Result<(), String> {
    let (url, git_ref) = parse_url_spec(spec);
    let dest = packages_dir().join(url);

    if dest.exists() {
        // Already cloned — fetch and checkout
        run_git(&dest, &["fetch", "--tags"])?;
        run_git(&dest, &["checkout", git_ref])?;
        eprintln!("✓ Updated {url} → {git_ref}");
    } else {
        // Fresh clone
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create directory: {e}"))?;
        }
        let clone_url = format!("https://{url}.git");
        run_git_global(&["clone", &clone_url, &dest.to_string_lossy()])?;
        run_git(&dest, &["checkout", git_ref])?;
        eprintln!("✓ Installed {url} → {git_ref}");
    }
    Ok(())
}

pub fn cmd_install() -> Result<(), String> {
    let manifest_path = Path::new("sema.toml");
    if !manifest_path.exists() {
        return Err("no sema.toml found in current directory".to_string());
    }
    let content = std::fs::read_to_string(manifest_path)
        .map_err(|e| format!("cannot read sema.toml: {e}"))?;
    let manifest: toml::Value = content
        .parse()
        .map_err(|e| format!("invalid sema.toml: {e}"))?;

    let deps = match manifest.get("deps").and_then(|d| d.as_table()) {
        Some(t) => t,
        None => {
            eprintln!("No [deps] section in sema.toml");
            return Ok(());
        }
    };

    for (name, url_val) in deps {
        let url = url_val
            .as_str()
            .ok_or_else(|| format!("dep {name}: expected string URL"))?;
        eprintln!("Installing {name}...");
        cmd_get(url)?;
    }
    Ok(())
}

pub fn cmd_update(name: Option<&str>) -> Result<(), String> {
    let pkg_dir = packages_dir();
    if let Some(name) = name {
        // Find the package by name — search for it in the packages dir
        let target = find_package_dir(&pkg_dir, name)?;
        run_git(&target, &["pull"])?;
        eprintln!("✓ Updated {name}");
    } else {
        // Update all packages
        for entry in find_all_packages(&pkg_dir)? {
            let rel = entry.strip_prefix(&pkg_dir).unwrap_or(&entry);
            eprintln!("Updating {}...", rel.display());
            if let Err(e) = run_git(&entry, &["pull"]) {
                eprintln!("  ✗ Failed: {e}");
            }
        }
    }
    Ok(())
}

pub fn cmd_remove(name: &str) -> Result<(), String> {
    let pkg_dir = packages_dir();
    let target = find_package_dir(&pkg_dir, name)?;
    std::fs::remove_dir_all(&target)
        .map_err(|e| format!("cannot remove {}: {e}", target.display()))?;

    // Clean up empty parent directories
    let mut parent = target.parent();
    while let Some(p) = parent {
        if p == pkg_dir {
            break;
        }
        if p.read_dir().map(|mut d| d.next().is_none()).unwrap_or(true) {
            std::fs::remove_dir(p).ok();
            parent = p.parent();
        } else {
            break;
        }
    }

    eprintln!("✓ Removed {name}");
    Ok(())
}

pub fn cmd_list() -> Result<(), String> {
    let pkg_dir = packages_dir();
    if !pkg_dir.exists() {
        println!("No packages installed.");
        return Ok(());
    }
    let packages = find_all_packages(&pkg_dir)?;
    if packages.is_empty() {
        println!("No packages installed.");
        return Ok(());
    }
    for pkg in packages {
        let rel = pkg.strip_prefix(&pkg_dir).unwrap_or(&pkg);
        let git_ref = get_current_ref(&pkg).unwrap_or_else(|| "unknown".to_string());
        println!("  {} @ {}", rel.display(), git_ref);
    }
    Ok(())
}

pub fn cmd_init() -> Result<(), String> {
    let manifest_path = Path::new("sema.toml");
    if manifest_path.exists() {
        return Err("sema.toml already exists".to_string());
    }
    // Infer project name from current directory
    let name = std::env::current_dir()
        .ok()
        .and_then(|d| d.file_name().map(|n| n.to_string_lossy().to_string()))
        .unwrap_or_else(|| "my-project".to_string());

    let content = format!(
        r#"[package]
name = "{name}"
version = "0.1.0"
description = ""
entrypoint = "mod.sema"

[deps]
"#
    );
    std::fs::write(manifest_path, content)
        .map_err(|e| format!("cannot write sema.toml: {e}"))?;
    eprintln!("✓ Created sema.toml");
    Ok(())
}

// --- Helpers ---

fn run_git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .map_err(|e| format!("failed to run git: {e} (is git installed?)"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git {} failed: {}", args.join(" "), stderr.trim()));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn run_git_global(args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .output()
        .map_err(|e| format!("failed to run git: {e} (is git installed?)"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git {} failed: {}", args.join(" "), stderr.trim()));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn get_current_ref(dir: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["describe", "--tags", "--exact-match"])
        .current_dir(dir)
        .output()
        .ok()?;
    if output.status.success() {
        return Some(String::from_utf8_lossy(&output.stdout).trim().to_string());
    }
    // Fall back to branch name
    let output = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(dir)
        .output()
        .ok()?;
    if output.status.success() {
        return Some(String::from_utf8_lossy(&output.stdout).trim().to_string());
    }
    None
}

/// Find a package directory by name or full URL path.
fn find_package_dir(pkg_dir: &Path, name: &str) -> Result<PathBuf, String> {
    // Try as full path first
    let full = pkg_dir.join(name);
    if full.is_dir() {
        return Ok(full);
    }
    // Search by directory name
    for entry in find_all_packages(pkg_dir)? {
        if entry.file_name().map(|n| n.to_string_lossy().to_string()) == Some(name.to_string()) {
            return Ok(entry);
        }
    }
    Err(format!("package not found: {name}"))
}

/// Find all installed package directories (directories containing sema.toml or mod.sema).
fn find_all_packages(pkg_dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut result = Vec::new();
    if !pkg_dir.exists() {
        return Ok(result);
    }
    walk_packages(pkg_dir, &mut result);
    result.sort();
    Ok(result)
}

fn walk_packages(dir: &Path, result: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // Is this a package root?
            if path.join("sema.toml").exists() || path.join("mod.sema").exists() {
                result.push(path);
            } else {
                walk_packages(&path, result);
            }
        }
    }
}
```

**Step 3: Wire dispatch in `main.rs`**

In the `match command` block, add:

```rust
Commands::Pkg { command } => {
    let result = match command {
        PkgCommands::Get { url } => pkg::cmd_get(&url),
        PkgCommands::Install => pkg::cmd_install(),
        PkgCommands::Update { name } => pkg::cmd_update(name.as_deref()),
        PkgCommands::Remove { name } => pkg::cmd_remove(&name),
        PkgCommands::List => pkg::cmd_list(),
        PkgCommands::Init => pkg::cmd_init(),
    };
    if let Err(e) = result {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}
```

Add `mod pkg;` near the top of `main.rs`.

**Step 4: Add `toml` dep to the `sema` binary crate**

In workspace `Cargo.toml`, add to `[workspace.dependencies]`:
```toml
toml = "0.8"
```

In `crates/sema/Cargo.toml`, add:
```toml
toml = { workspace = true }
```

**Step 5: Manual test**

```bash
cargo build
./target/debug/sema pkg init
cat sema.toml
./target/debug/sema pkg get github.com/helgesverre/sema-http@main
./target/debug/sema pkg list
./target/debug/sema pkg remove sema-http
```

**Step 6: Commit**

```bash
git add crates/sema/src/pkg.rs crates/sema/src/main.rs Cargo.toml crates/sema/Cargo.toml
git commit -m "feat(cli): add sema pkg subcommand (get/install/update/remove/list/init)"
```

---

## Task 5: `toml/decode` and `toml/encode` in Stdlib

**Files:**
- Create: `crates/sema-stdlib/src/toml_ops.rs`
- Modify: `crates/sema-stdlib/src/lib.rs` (add module + register call)
- Modify: `crates/sema-stdlib/Cargo.toml` (add `toml` dep)

**Context:** Mirror `json.rs` exactly. TOML tables → Sema maps with keyword keys. Arrays → lists. Strings/ints/floats/bools → native values. Datetimes → strings. Not gated (no capability needed for pure data conversion).

**Step 1: Create `toml_ops.rs`**

```rust
// crates/sema-stdlib/src/toml_ops.rs
use std::collections::BTreeMap;
use sema_core::{check_arity, intern, SemaError, Value, ValueView};
use crate::register_fn;

pub fn register(env: &sema_core::Env) {
    register_fn(env, "toml/decode", |args| {
        check_arity!(args, "toml/decode", 1);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let table: toml::Table =
            s.parse().map_err(|e| SemaError::eval(format!("toml/decode: {e}")))?;
        Ok(toml_table_to_value(&table))
    });

    register_fn(env, "toml/encode", |args| {
        check_arity!(args, "toml/encode", 1);
        let toml_val = value_to_toml(&args[0])?;
        match toml_val {
            toml::Value::Table(t) => {
                let s = toml::to_string(&t)
                    .map_err(|e| SemaError::eval(format!("toml/encode: {e}")))?;
                Ok(Value::string(&s))
            }
            _ => Err(SemaError::eval("toml/encode: top-level value must be a map")),
        }
    });
}

fn toml_to_value(val: &toml::Value) -> Value {
    match val {
        toml::Value::String(s) => Value::string(s),
        toml::Value::Integer(n) => Value::int(*n),
        toml::Value::Float(f) => Value::float(*f),
        toml::Value::Boolean(b) => Value::bool(*b),
        toml::Value::Datetime(dt) => Value::string(&dt.to_string()),
        toml::Value::Array(arr) => {
            Value::list(arr.iter().map(toml_to_value).collect())
        }
        toml::Value::Table(table) => toml_table_to_value(table),
    }
}

fn toml_table_to_value(table: &toml::map::Map<String, toml::Value>) -> Value {
    let mut map = BTreeMap::new();
    for (k, v) in table {
        map.insert(Value::keyword(intern(k)), toml_to_value(v));
    }
    Value::map(map)
}

fn value_to_toml(val: &Value) -> Result<toml::Value, SemaError> {
    match val.view() {
        ValueView::Nil => Ok(toml::Value::String("nil".to_string())),
        ValueView::Bool(b) => Ok(toml::Value::Boolean(b)),
        ValueView::Int(n) => Ok(toml::Value::Integer(n)),
        ValueView::Float(f) => Ok(toml::Value::Float(f)),
        ValueView::String(s) => Ok(toml::Value::String(s.to_string())),
        ValueView::Keyword(s) => Ok(toml::Value::String(sema_core::resolve(s))),
        ValueView::Symbol(s) => Ok(toml::Value::String(sema_core::resolve(s))),
        ValueView::List(items) | ValueView::Vector(items) => {
            let arr: Result<Vec<_>, _> = items.iter().map(value_to_toml).collect();
            Ok(toml::Value::Array(arr?))
        }
        ValueView::Map(map) => {
            let mut table = toml::map::Map::new();
            for (k, v) in map.iter() {
                let key = match k.view() {
                    ValueView::String(s) => s.to_string(),
                    ValueView::Keyword(s) => sema_core::resolve(s),
                    ValueView::Symbol(s) => sema_core::resolve(s),
                    _ => k.to_string(),
                };
                table.insert(key, value_to_toml(v)?);
            }
            Ok(toml::Value::Table(table))
        }
        _ => Err(SemaError::eval(format!(
            "toml/encode: cannot encode {}",
            val.type_name()
        ))),
    }
}
```

**Step 2: Register in `lib.rs`**

Add to module declarations (near `json`):
```rust
mod toml_ops;
```

Add to `register_stdlib`:
```rust
toml_ops::register(env);
```

**Step 3: Add `toml` dep to `crates/sema-stdlib/Cargo.toml`**

```toml
toml = { workspace = true }
```

**Step 4: Add dual-eval tests in `crates/sema/tests/dual_eval_test.rs`**

```rust
dual_eval_tests! {
    toml_decode_basic: r#"
        (let [t (toml/decode "[package]\nname = \"test\"\nversion = \"1.0\"")]
          (:name (:package t)))
    "# => Value::string("test"),

    toml_decode_deps: r#"
        (let [t (toml/decode "[deps]\nhttp = \"github.com/user/http\"")]
          (:http (:deps t)))
    "# => Value::string("github.com/user/http"),

    toml_decode_array: r#"
        (let [t (toml/decode "tags = [\"a\", \"b\"]")]
          (length (:tags t)))
    "# => Value::int(2),

    toml_decode_nested: r#"
        (let [t (toml/decode "[package]\nname = \"x\"\n\n[deps]\nfoo = \"bar\"")]
          (list (:name (:package t)) (:foo (:deps t))))
    "# => Value::list(vec![Value::string("x"), Value::string("bar")]),
}
```

**Step 5: Run tests**

Run: `cargo test -p sema --test dual_eval_test -- toml`
Expected: PASS

**Step 6: Commit**

```bash
git add crates/sema-stdlib/src/toml_ops.rs crates/sema-stdlib/src/lib.rs \
    crates/sema-stdlib/Cargo.toml crates/sema/tests/dual_eval_test.rs
git commit -m "feat(stdlib): add toml/decode and toml/encode"
```

---

## Task 6: Documentation

**Files:**
- Create: `website/docs/stdlib/toml.md` (stdlib docs for toml/decode, toml/encode)
- Create: `website/docs/guides/packages.md` (package manager guide)

**Step 1: Create stdlib docs for TOML**

Document `toml/decode` and `toml/encode` following the pattern in `website/docs/stdlib/json.md`.

**Step 2: Create package manager guide**

Document the full workflow: `sema pkg init`, `sema.toml` format, `sema pkg get`, `sema pkg install`, how `(import "github.com/...")` works, and the on-disk layout.

**Step 3: Commit**

```bash
git add website/docs/stdlib/toml.md website/docs/guides/packages.md
git commit -m "docs: add toml stdlib reference and package manager guide"
```

---

## Task Summary

| Task | Scope | Files | Effort |
|------|-------|-------|--------|
| 1. Import resolver in `sema-core` | Core resolution logic | `resolve.rs` (new), `lib.rs` | S (30 min) |
| 2. Wire into `eval_import` | Runtime package imports | `special_forms.rs` | S (20 min) |
| 3. Wire into `import_tracer` | Build-time package bundling | `import_tracer.rs` | S (20 min) |
| 4. `sema pkg` CLI | Full CLI subcommand | `pkg.rs` (new), `main.rs`, `Cargo.toml` | M (1-2h) |
| 5. `toml/decode`+`toml/encode` | Stdlib TOML support | `toml_ops.rs` (new), `lib.rs` | S (30 min) |
| 6. Documentation | Guides + stdlib ref | 2 new .md files | S (30 min) |

**Total estimated effort: 3-5 hours**

## Next Iteration (v2)

These are the priority follow-ups after the initial implementation ships:

### `sema.lock` — Lock File for Reproducible Builds

**Purpose:** Record exact resolved versions (commit SHAs for git packages, checksums for registry packages) so that `sema pkg install` produces identical `~/.sema/packages/` contents across machines and time. Committed to version control.

**Format:** TOML file in the project root alongside `sema.toml`. Uses quoted keys with real package identifiers (no sanitization) to avoid collisions.

```toml
# sema.lock — auto-generated, do not edit manually
lock_version = 1

[packages."github.com/user/repo"]
source = "git"
ref = "main"
commit = "a1b2c3d4e5f6789012345678901234567890abcd"

[packages."http-helpers"]
source = "registry"
version = "1.2.0"
registry = "https://pkg.sema-lang.com"
checksum = "abc123def456789..."
```

**Design decisions:**
- **Quoted TOML keys** — `[packages."github.com/user/repo"]` uses the real package identifier. Sanitizing `/` and `.` to `_` would cause collisions (e.g., `github.com/a_b/c` vs `github.com/a/b_c`).
- **No redundant fields** — Git `url` is always derivable via `clone_url()` (`https://{path}.git`). Registry `name` is the key itself. Only store what's needed for pinning.
- **`lock_version = 1`** — Top-level field for future format evolution.
- **Checksums are raw hex** — Consistent with `registry_install()` which uses `format!("{:x}", sha2::Sha256::digest(...))` and `.sema-pkg.json`. Implicitly SHA256.

**Behavior by command:**

| Command | Lock file behavior |
|---------|-------------------|
| `sema pkg add <spec>` | Installs package, **writes/updates** lock entry with resolved commit/checksum |
| `sema pkg install` | If `sema.lock` exists, install from locked versions. If a dep is in `sema.toml` but not in lock, resolve and **append** to lock with a warning. Warn on orphaned lock entries but don't prune automatically. |
| `sema pkg install --locked` | Install from lock only. **Fail** if lock is missing, or if any dep in `sema.toml` is not in lock (or vice versa). Never resolves fresh. For CI. |
| `sema pkg update [name]` | Re-resolves to latest (per sema.toml ref/version), **rewrites** lock entries |
| `sema pkg remove <name>` | Removes package, **prunes** lock entry |

**Integrity verification (when installing from an existing lock):**
- **Git packages:** After `git checkout <commit>`, run `git rev-parse HEAD` and compare against `commit` in lock. Error on mismatch.
- **Registry packages:** After download, compute SHA256 of tarball and compare against `checksum` in lock. Error on mismatch.
- Verification only applies when a lock entry exists. First-time `add` writes the lock, doesn't verify against it.

**Stale lock detection (on `sema pkg install` without `--locked`):**
- Dep in `sema.toml` but not in `sema.lock` → warn `"{name} not in sema.lock, resolving..."`, resolve, append to lock.
- Dep in `sema.lock` but not in `sema.toml` → warn `"{name} in sema.lock but not in sema.toml"`. Do not auto-prune (avoids churn when switching branches).
- With `--locked` → both cases are hard errors instead of warnings.

**Git fetch robustness (for locked installs):**
- Use `git fetch origin` (not just `--tags`) to ensure branch heads and commits are available locally.
- Use `git checkout --detach <commit>` for pinned-commit installs.
- On `--locked`, fail if working tree is dirty rather than silently resetting.

**Required refactor:** `cmd_install()` currently calls `cmd_add()` which edits `sema.toml`. Lock-aware install must not modify the manifest. Extract internal functions:
- `install_git(path, ref_or_commit) → (ref, commit)` — pure install, returns resolved data.
- `install_registry(name, version, registry) → (version, checksum)` — pure install, returns resolved data.
- `cmd_add` = `install_*` + update `sema.toml` + update `sema.lock`.
- `cmd_install` = read lock (if present) + `install_*` + maybe update lock. Never touches `sema.toml`.

**Implementation scope:**

| Work Item | Effort |
|-----------|--------|
| Refactor `cmd_install`/`cmd_add` to separate install from manifest mutation | ~45 min |
| Lock file struct + read/write with `toml_edit` | ~30 min |
| Wire lock into `cmd_add` (write after install) | ~30 min |
| Wire lock into `cmd_install` (read lock, verify, stale detection) | ~45 min |
| Wire lock into `cmd_update` and `cmd_remove` (rewrite/prune entries) | ~30 min |
| `--locked` flag on install | ~20 min |
| Improve git fetch for locked installs (`fetch origin`, detached checkout, dirty check) | ~30 min |
| Tests (unit: read/write/round-trip; integration: install-from-lock, --locked failure) | ~1 hour |

**Total estimated effort: 4–6 hours**

### Pre/post-install hooks

`[hooks]` section in `sema.toml` supporting `pre-install` and `post-install` scripts. Scripts can be `.sema` files (run via the interpreter using shebang `#!/usr/bin/env sema`) or shell scripts. Executed after `sema pkg add` and `sema pkg install`.

## Future Work (YAGNI for now)

- **`sema pkg search`** — search a static JSON registry at `sema-lang.com/packages.json`
- **Project-local packages** — `<project>/.sema/packages/` for vendoring
- **Custom package sources** — GitLab, private servers, local directories

## Example: Self-Hosted Package Manager in Sema

As a showcase of Sema's capabilities, an example `pkg.sema` script demonstrating that the package manager *could* be written in Sema itself lives at `examples/pkg.sema`. This is for demonstration purposes — the real implementation is in Rust for reliability and performance.
