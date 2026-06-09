# Package Manager & JSON Consolidation Cleanup

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Eliminate 4× JSON conversion duplication by adding canonical functions to sema-core, then clean up pkg.rs naming/comments/duplication.

**Architecture:** sema-core already depends on serde_json (unused). Add a `json.rs` module there with canonical `value_to_json`/`json_to_value`. All consumer crates already depend on sema-core, so zero Cargo.toml changes needed. Then replace all 4 copies. Separately, clean up pkg.rs local issues.

**Tech Stack:** Rust, serde_json, sema-core Value/ValueView/SemaError

---

### Task 1: Add canonical JSON conversion to sema-core

**Files:**
- Create: `crates/sema-core/src/json.rs`
- Modify: `crates/sema-core/src/lib.rs`

**Context:** sema-core already has `serde_json` in Cargo.toml but doesn't use it. The canonical implementation lives in `sema-stdlib/src/json.rs` lines 35–105. We copy that logic here as the single source of truth. We also add a `value_to_json_lossy` variant for kv.rs which silently converts NaN→null and unsupported types→string instead of erroring.

**Step 1: Create `crates/sema-core/src/json.rs`**

```rust
//! Canonical conversions between `Value` and `serde_json::Value`.
//!
//! Two modes:
//! - **Strict** (`value_to_json`): errors on NaN/Infinity and unsupported types.
//! - **Lossy** (`value_to_json_lossy`): NaN/Infinity→null, unsupported→string.

use std::collections::BTreeMap;

use crate::{resolve, SemaError, Value, ValueView};

/// Convert a Sema Value to a JSON value, erroring on NaN/Infinity and unsupported types.
pub fn value_to_json(val: &Value) -> Result<serde_json::Value, SemaError> {
    match val.view() {
        ValueView::Nil => Ok(serde_json::Value::Null),
        ValueView::Bool(b) => Ok(serde_json::Value::Bool(b)),
        ValueView::Int(n) => Ok(serde_json::Value::Number(n.into())),
        ValueView::Float(f) => serde_json::Number::from_f64(f)
            .map(serde_json::Value::Number)
            .ok_or_else(|| SemaError::eval("cannot encode NaN/Infinity as JSON")),
        ValueView::String(s) => Ok(serde_json::Value::String(s.to_string())),
        ValueView::Keyword(s) => Ok(serde_json::Value::String(resolve(s))),
        ValueView::Symbol(s) => Ok(serde_json::Value::String(resolve(s))),
        ValueView::List(items) | ValueView::Vector(items) => {
            let arr: Result<Vec<_>, _> = items.iter().map(value_to_json).collect();
            Ok(serde_json::Value::Array(arr?))
        }
        ValueView::Map(map) => {
            let mut obj = serde_json::Map::new();
            for (k, v) in map.iter() {
                obj.insert(key_to_string(k), value_to_json(v)?);
            }
            Ok(serde_json::Value::Object(obj))
        }
        ValueView::HashMap(map) => {
            let mut obj = serde_json::Map::new();
            for (k, v) in map.iter() {
                obj.insert(key_to_string(k), value_to_json(v)?);
            }
            Ok(serde_json::Value::Object(obj))
        }
        _ => Err(SemaError::eval(format!(
            "cannot encode {} as JSON",
            val.type_name()
        ))),
    }
}

/// Convert a Sema Value to JSON without erroring. NaN/Infinity become null,
/// unsupported types become their string representation.
pub fn value_to_json_lossy(val: &Value) -> serde_json::Value {
    match value_to_json(val) {
        Ok(v) => v,
        Err(_) => {
            // For floats that failed (NaN/Inf), return null.
            // For unsupported types, return their string repr.
            if let ValueView::Float(_) = val.view() {
                serde_json::Value::Null
            } else {
                serde_json::Value::String(val.to_string())
            }
        }
    }
}

/// Convert a JSON value to a Sema Value.
pub fn json_to_value(json: &serde_json::Value) -> Value {
    match json {
        serde_json::Value::Null => Value::nil(),
        serde_json::Value::Bool(b) => Value::bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::int(i)
            } else if let Some(f) = n.as_f64() {
                Value::float(f)
            } else {
                Value::nil()
            }
        }
        serde_json::Value::String(s) => Value::string(s),
        serde_json::Value::Array(arr) => Value::list(arr.iter().map(json_to_value).collect()),
        serde_json::Value::Object(obj) => {
            let mut map = BTreeMap::new();
            for (k, v) in obj {
                map.insert(Value::keyword(k), json_to_value(v));
            }
            Value::map(map)
        }
    }
}

/// Extract a string key from a Value for use as a JSON/TOML map key.
pub fn key_to_string(k: &Value) -> String {
    match k.view() {
        ValueView::String(s) => s.to_string(),
        ValueView::Keyword(s) => resolve(s),
        ValueView::Symbol(s) => resolve(s),
        _ => k.to_string(),
    }
}
```

**Step 2: Register the module in `crates/sema-core/src/lib.rs`**

Add `pub mod json;` and re-export the public functions:

```rust
pub mod json;
```

Add to the existing pub use block:

```rust
pub use json::{json_to_value, key_to_string, value_to_json, value_to_json_lossy};
```

**Step 3: Run tests**

Run: `cargo test -p sema-core`
Expected: PASS (no existing tests break; new module has no tests yet — tests exist in consumers)

**Step 4: Commit**

```bash
git add crates/sema-core/src/json.rs crates/sema-core/src/lib.rs
git commit -m "refactor: add canonical JSON conversion to sema-core"
```

---

### Task 2: Update sema-stdlib/json.rs to use sema-core canonical functions

**Files:**
- Modify: `crates/sema-stdlib/src/json.rs`

**Context:** This file currently defines `value_to_json` and `json_to_value` (pub) plus registration. Replace the implementations with re-exports from sema-core and remove the duplicated conversion code. Keep the `register()` function and its `pub use` re-exports so downstream callers (`http.rs` uses `crate::json::value_to_json`) continue to work.

**Step 1: Replace `crates/sema-stdlib/src/json.rs`**

The file should become:

```rust
use sema_core::{check_arity, SemaError, Value};

// Re-export canonical conversions so existing `crate::json::value_to_json` callers still work.
pub use sema_core::json::{json_to_value, value_to_json};

use crate::register_fn;

pub fn register(env: &sema_core::Env) {
    register_fn(env, "json/encode", |args| {
        check_arity!(args, "json/encode", 1);
        let json = value_to_json(&args[0])
            .map_err(|e| SemaError::eval(format!("json/encode: {e}")))?;
        let s = serde_json::to_string(&json)
            .map_err(|e| SemaError::eval(format!("json/encode: {e}")))?;
        Ok(Value::string(&s))
    });

    register_fn(env, "json/encode-pretty", |args| {
        check_arity!(args, "json/encode-pretty", 1);
        let json = value_to_json(&args[0])
            .map_err(|e| SemaError::eval(format!("json/encode-pretty: {e}")))?;
        let s = serde_json::to_string_pretty(&json)
            .map_err(|e| SemaError::eval(format!("json/encode-pretty: {e}")))?;
        Ok(Value::string(&s))
    });

    register_fn(env, "json/decode", |args| {
        check_arity!(args, "json/decode", 1);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let json: serde_json::Value =
            serde_json::from_str(s).map_err(|e| SemaError::eval(format!("json/decode: {e}")))?;
        Ok(json_to_value(&json))
    });
}
```

**Step 2: Run tests**

Run: `cargo test -p sema-stdlib`
Expected: PASS

Also run integration tests to verify json/encode, json/decode still work:
Run: `cargo test -p sema --test integration_test -- json`
Expected: PASS

**Step 3: Commit**

```bash
git add crates/sema-stdlib/src/json.rs
git commit -m "refactor: sema-stdlib json.rs uses sema-core canonical conversion"
```

---

### Task 3: Update sema-stdlib/kv.rs to use sema-core lossy conversion

**Files:**
- Modify: `crates/sema-stdlib/src/kv.rs`

**Context:** kv.rs has private `sema_to_json_val` / `json_val_to_sema` (lines 166–224) that duplicate JSON conversion with slightly different semantics (NaN→null silently). Replace with `sema_core::value_to_json_lossy` and `sema_core::json_to_value`.

**Step 1: Delete `sema_to_json_val` and `json_val_to_sema` functions (lines 166–224)**

**Step 2: Replace all call sites**

- `sema_to_json_val(...)` → `sema_core::value_to_json_lossy(...)`
- `json_val_to_sema(...)` → `sema_core::json_to_value(...)`

Search for call sites with: `grep -n "sema_to_json_val\|json_val_to_sema" crates/sema-stdlib/src/kv.rs`

**Step 3: Run tests**

Run: `cargo test -p sema-stdlib`
Run: `cargo test -p sema --test integration_test -- kv`
Expected: PASS

**Step 4: Commit**

```bash
git add crates/sema-stdlib/src/kv.rs
git commit -m "refactor: kv.rs uses sema-core json conversion"
```

---

### Task 4: Update sema-llm to use sema-core canonical conversion

**Files:**
- Modify: `crates/sema-llm/src/builtins.rs`

**Context:** builtins.rs has `sema_value_to_json` (line 4409) and `json_to_sema_value` (line 4455) that duplicate the stdlib's JSON conversion. Replace with `sema_core::value_to_json` and `sema_core::json_to_value`. Note: `sema_value_to_json_schema` (line 4045) is a *different* function that converts Value to JSON Schema — keep it.

**Step 1: Delete the `sema_value_to_json` function (lines 4409–4452)**

**Step 2: Delete the `json_to_sema_value` function (lines 4455–4477)**

**Step 3: Remove the now-unused `BTreeMap` import if it was only used by `json_to_sema_value`**

Check: `grep -n "BTreeMap" crates/sema-llm/src/builtins.rs` — if other code uses it, keep.

**Step 4: Replace all call sites**

Find with: `grep -n "sema_value_to_json\|json_to_sema_value" crates/sema-llm/src/builtins.rs`

Replace:
- `sema_value_to_json(...)` → `sema_core::value_to_json(...)` 
- `json_to_sema_value(...)` → `sema_core::json_to_value(...)`
- `crate::builtins::sema_value_to_json(...)` → `sema_core::value_to_json(...)`

**Important:** The call at line 4280 does `.unwrap_or(serde_json::Value::Null)` — this stays the same since `value_to_json` returns `Result`.

**Step 5: Run tests**

Run: `cargo test -p sema-llm`
Expected: PASS

**Step 6: Commit**

```bash
git add crates/sema-llm/src/builtins.rs
git commit -m "refactor: sema-llm uses sema-core json conversion"
```

---

### Task 5: Update sema-wasm to use sema-core canonical conversion

**Files:**
- Modify: `crates/sema-wasm/src/lib.rs`

**Context:** sema-wasm has `value_to_json_for_body` (line 218–265) and `json_to_value` (line 2337–2360) that duplicate the conversion. Replace with `sema_core::value_to_json` and `sema_core::json_to_value`.

**Step 1: Delete `value_to_json_for_body` function (lines 218–265)**

**Step 2: Delete `json_to_value` function (lines 2337–2360)**

**Step 3: Replace all call sites**

Find with: `grep -n "value_to_json_for_body\|json_to_value" crates/sema-wasm/src/lib.rs`

Replace:
- `value_to_json_for_body(...)` → `sema_core::value_to_json(...)`
- `json_to_value(...)` → `sema_core::json_to_value(...)`

**Step 4: Remove now-unused `BTreeMap` import if only used by the deleted function**

**Step 5: Verify build (no test suite for wasm)**

Run: `cargo check -p sema-wasm` (may need wasm target, if not available just do `cargo check` for the whole workspace)
Expected: No errors

**Step 6: Commit**

```bash
git add crates/sema-wasm/src/lib.rs
git commit -m "refactor: sema-wasm uses sema-core json conversion"
```

---

### Task 6: Use `key_to_string` in toml_ops.rs

**Files:**
- Modify: `crates/sema-stdlib/src/toml_ops.rs`

**Context:** `value_to_toml` (lines 55–98) has duplicated map key extraction logic (String/Keyword/Symbol match) in both the `Map` and `HashMap` arms. Replace with `sema_core::key_to_string` from the new json.rs module.

**Step 1: Replace the key extraction in `value_to_toml`**

In both the `ValueView::Map` and `ValueView::HashMap` arms, replace the `match k.view() { ... }` block with `sema_core::key_to_string(k)`.

**Step 2: Run tests**

Run: `cargo test -p sema-stdlib`
Run: `cargo test -p sema --test integration_test -- toml`
Expected: PASS

**Step 3: Commit**

```bash
git add crates/sema-stdlib/src/toml_ops.rs
git commit -m "refactor: toml_ops uses key_to_string from sema-core"
```

---

### Task 7: Merge run_git/run_git_global in pkg.rs

**Files:**
- Modify: `crates/sema/src/pkg.rs`

**Context:** `run_git` (line 6) and `run_git_global` (line 20) are identical except `run_git` calls `.current_dir(dir)`. Merge into one function with `Option<&Path>`.

**Step 1: Replace both functions with:**

```rust
fn run_git(dir: Option<&Path>, args: &[&str]) -> Result<String, String> {
    let mut cmd = Command::new("git");
    cmd.args(args);
    if let Some(dir) = dir {
        cmd.current_dir(dir);
    }
    let output = cmd
        .output()
        .map_err(|e| format!("Failed to run git: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(format!("git {} failed: {stderr}", args.join(" ")))
    }
}
```

**Step 2: Update all call sites**

- `run_git(&dest, ...)` → `run_git(Some(&dest), ...)`
- `run_git(&dir, ...)` → `run_git(Some(&dir), ...)`  
- `run_git(dir, ...)` → `run_git(Some(dir), ...)`
- `run_git_global(...)` → `run_git(None, ...)`

**Step 3: Run tests**

Run: `cargo test -p sema -- pkg`
Expected: PASS

**Step 4: Commit**

```bash
git add crates/sema/src/pkg.rs
git commit -m "refactor: merge run_git/run_git_global into single function"
```

---

### Task 8: Introduce PackageSpec struct in sema-core

**Files:**
- Modify: `crates/sema-core/src/resolve.rs`

**Context:** `parse_url_spec` in pkg.rs returns a bare `(&str, &str)` tuple. The path portion is validated separately via `validate_package_spec`. We introduce a `PackagePath` newtype (validated, no `@ref`) and `PackageSpec` struct (`PackagePath` + `git_ref`) so that "parsed implies validated". This lives in `sema_core::resolve` alongside the existing validation. Construction is fail-fast. Derived values (`clone_url`, `dest_dir`) are methods, not stored fields.

**Step 1: Add `PackagePath` and `PackageSpec` to `crates/sema-core/src/resolve.rs`**

Add after the existing `validate_package_spec` function:

```rust
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
            return Err(SemaError::eval(format!(
                "invalid package spec: empty git ref: {spec}"
            )));
        }
        if git_ref.contains('\0') {
            return Err(SemaError::eval(
                "invalid package spec: NUL byte in git ref".to_string(),
            ));
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
```

**Step 2: Update the existing `resolve_package_import_in` to accept `&str` still**

No change needed — `resolve_package_import_in` deals with import paths (no `@ref`), not get specs. It already calls `validate_package_spec` internally. Optionally, it could accept `&PackagePath` in the future, but that's a larger ripple — skip for now.

**Step 3: Add tests for PackageSpec and PackagePath**

Add to the existing `#[cfg(test)] mod tests` block in `resolve.rs`:

```rust
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
    assert_eq!(s.dest_dir(&base), PathBuf::from("/home/user/.sema/packages/github.com/user/repo"));
}

#[test]
fn test_package_spec_rejects_empty_ref() {
    assert!(PackageSpec::parse("github.com/user/repo@").is_err());
}

#[test]
fn test_package_spec_rejects_traversal_in_path() {
    assert!(PackageSpec::parse("github.com/../../etc/passwd@main").is_err());
}
```

**Step 4: Run tests**

Run: `cargo test -p sema-core`
Expected: PASS

**Step 5: Commit**

```bash
git add crates/sema-core/src/resolve.rs
git commit -m "feat: add PackagePath/PackageSpec types for validated package specs"
```

---

### Task 9: Update pkg.rs to use PackageSpec and clean up

**Files:**
- Modify: `crates/sema/src/pkg.rs`

**Context:** Now that `PackageSpec` exists in sema-core, replace `parse_url_spec` and the manual `validate_package_spec` call in `cmd_get` with a single `PackageSpec::parse()`. Also rename `get_current_ref` → `current_git_ref`, rename `walk_packages` → `collect_packages`, and clean up obvious comments.

**Step 1: Delete `parse_url_spec` function (lines 40–46)**

**Step 2: Update `cmd_get` to use `PackageSpec`**

```rust
pub fn cmd_get(spec: &str) -> Result<(), String> {
    let spec = sema_core::resolve::PackageSpec::parse(spec).map_err(|e| e.to_string())?;
    let pkg_dir = packages_dir();
    let dest = spec.dest_dir(&pkg_dir);

    if dest.exists() {
        run_git(Some(&dest), &["fetch", "--tags"])?;
        run_git(Some(&dest), &["checkout", "--", &spec.git_ref])?;
        let current = current_git_ref(&dest);
        println!("✓ Updated {} → {current}", spec.path);
    } else {
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create directory: {e}"))?;
        }
        run_git(None, &["clone", &spec.clone_url(), &dest.to_string_lossy()])?;
        run_git(Some(&dest), &["checkout", "--", &spec.git_ref])?;
        let current = current_git_ref(&dest);
        println!("✓ Installed {} → {current}", spec.path);
    }

    Ok(())
}
```

Note: `git checkout -- <ref>` uses `--` to prevent ref names starting with `-` from being interpreted as options.

**Step 3: Rename `get_current_ref` → `current_git_ref`**

Update all call sites in `cmd_get` (already done above), `cmd_update`, and `cmd_list`.

**Step 4: Rename `walk_packages` → `collect_packages`**

Keep `find_all_packages` as the public-facing function, rename the recursive helper:

```rust
fn find_all_packages(pkg_dir: &Path) -> Vec<PathBuf> {
    let mut packages = Vec::new();
    collect_packages(pkg_dir, &mut packages);
    packages
}

fn collect_packages(dir: &Path, packages: &mut Vec<PathBuf>) {
    // ... existing body unchanged ...
}
```

**Step 5: Clean up comments**

Remove these "what" comments:
- `// Try exact path match first (e.g., github.com/user/repo)`
- `// Search by directory name`
- `// A package root has sema.toml or mod.sema`
- `// Update existing package`
- `// Clone new package`

Keep these "why" comments:
- `// Skip symlinks to avoid loops and escaping the packages directory`

**Step 6: Update tests**

The existing `test_parse_url_spec_*` tests in pkg.rs should be removed (parsing is now tested via `PackageSpec` in sema-core). The `test_cmd_get_rejects_traversal` test still works since `PackageSpec::parse` calls `validate_package_spec`.

**Step 7: Run tests**

Run: `cargo test -p sema -- pkg`
Expected: PASS

**Step 8: Commit**

```bash
git add crates/sema/src/pkg.rs
git commit -m "refactor: use PackageSpec, rename functions, clean up comments in pkg.rs"
```

---

### Task 10: Remove unused serde dep

**Files:**
- Potentially modify: `crates/sema-core/Cargo.toml` (check after Task 1)

**Context:** After Task 1, sema-core now uses serde_json. Check if `serde` (without `_json`) is also used. If not, remove it.

**Step 1: Check if serde is used**

Run: `grep -rn "use serde\|serde::" crates/sema-core/src/`

If no results (only serde_json is used), remove `serde.workspace = true` from `crates/sema-core/Cargo.toml`.

**Step 2: Verify**

Run: `cargo check -p sema-core`
Expected: PASS

**Step 3: Commit (if change was made)**

```bash
git add crates/sema-core/Cargo.toml
git commit -m "chore: remove unused serde dep from sema-core"
```
