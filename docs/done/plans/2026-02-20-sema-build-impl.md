# `sema build` Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement `sema build` — a command that compiles sema programs into standalone executables by appending a VFS archive to the runtime binary.

**Architecture:** Trailer-based binary format (archive appended to sema binary). Thread-local VFS in sema-stdlib for file interception. Import tracing at build time. Archive format with versioned header and extensible metadata.

**Tech Stack:** Rust, existing sema crates (sema-core, sema-stdlib, sema-vm, sema-eval, sema-reader)

**Design Doc:** `docs/plans/2026-02-20-sema-build-design.md`

---

### Task 1: Add `sema_home()` utility to sema-core

**Files:**
- Create: `crates/sema-core/src/home.rs`
- Modify: `crates/sema-core/src/lib.rs`

**Step 1: Create `crates/sema-core/src/home.rs`**

```rust
use std::path::PathBuf;

/// Returns the sema home directory.
/// Resolution: $SEMA_HOME > $HOME/.sema > %USERPROFILE%\.sema > .sema
pub fn sema_home() -> PathBuf {
    if let Ok(p) = std::env::var("SEMA_HOME") {
        return PathBuf::from(p);
    }
    if let Ok(home) = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")) {
        return PathBuf::from(home).join(".sema");
    }
    PathBuf::from(".sema")
}
```

**Step 2: Add `pub mod home;` and re-export in `crates/sema-core/src/lib.rs`**

Add after the existing module declarations:

```rust
pub mod home;
pub use home::sema_home;
```

**Step 3: Update `dirs_path()` in `crates/sema/src/main.rs` to use the new utility**

Replace the `dirs_path()` and `dirs_home()` functions (lines 1170-1178) with:

```rust
fn dirs_path() -> std::path::PathBuf {
    sema_core::sema_home()
}
```

Remove the `dirs_home()` function entirely.

**Step 4: Add `sys/sema-home` builtin in `crates/sema-stdlib/src/system.rs`**

After the `sys/home-dir` registration (around line 138), add:

```rust
register_fn(env, "sys/sema-home", |args| {
    check_arity!(args, "sys/sema-home", 0);
    Ok(Value::string(&sema_core::sema_home().to_string_lossy()))
});
```

**Step 5: Verify it compiles**

Run: `cargo build -p sema-core -p sema-stdlib -p sema-lang 2>&1 | tail -5`
Expected: Compiles without errors.

**Step 6: Commit**

```bash
git add crates/sema-core/src/home.rs crates/sema-core/src/lib.rs crates/sema-stdlib/src/system.rs crates/sema/src/main.rs
git commit -m "feat: add sema_home() utility and sys/sema-home builtin"
```

---

### Task 2: Create VFS module in sema-core

> **Changed from original plan:** VFS lives in sema-core (not sema-stdlib) so both sema-eval (import/load interception) and sema-stdlib (file/read interception) can access it without circular dependencies.

**Files:**
- Create: `crates/sema-core/src/vfs.rs`
- Modify: `crates/sema-core/src/lib.rs`

**Step 1: Create `crates/sema-core/src/vfs.rs`**

```rust
use std::cell::RefCell;
use std::collections::HashMap;

thread_local! {
    static EMBEDDED_VFS: RefCell<Option<HashMap<String, Vec<u8>>>> = const { RefCell::new(None) };
}

/// Initialize the VFS with embedded files. Called once at startup for bundled binaries.
pub fn init_vfs(files: HashMap<String, Vec<u8>>) {
    EMBEDDED_VFS.with(|vfs| {
        *vfs.borrow_mut() = Some(files);
    });
}

/// Read a file from the VFS. Returns None if VFS is inactive or file not found.
pub fn vfs_read(path: &str) -> Option<Vec<u8>> {
    EMBEDDED_VFS.with(|vfs| {
        let vfs = vfs.borrow();
        vfs.as_ref()?.get(path).cloned()
    })
}

/// Check if a file exists in the VFS. Returns None if VFS is inactive.
pub fn vfs_exists(path: &str) -> Option<bool> {
    EMBEDDED_VFS.with(|vfs| {
        let vfs = vfs.borrow();
        let map = vfs.as_ref()?;
        Some(map.contains_key(path))
    })
}

/// Check if the VFS is active (has been initialized).
pub fn is_vfs_active() -> bool {
    EMBEDDED_VFS.with(|vfs| vfs.borrow().is_some())
}

/// Try to resolve a path against the VFS, normalizing it.
/// The `base_dir` is used to resolve relative paths.
/// Returns the VFS content if found.
pub fn vfs_resolve_and_read(path: &str, base_dir: Option<&str>) -> Option<Vec<u8>> {
    // Try the path as-is first
    if let Some(data) = vfs_read(path) {
        return Some(data);
    }

    // Try resolving relative to base_dir
    if let Some(base) = base_dir {
        let resolved = std::path::Path::new(base).join(path);
        let normalized = normalize_path(&resolved);
        if let Some(data) = vfs_read(&normalized) {
            return Some(data);
        }
    }

    None
}

/// Normalize a path by resolving `.` and `..` components without hitting the filesystem.
fn normalize_path(path: &std::path::Path) -> String {
    let mut components = Vec::new();
    for comp in path.components() {
        match comp {
            std::path::Component::CurDir => {} // skip "."
            std::path::Component::ParentDir => {
                components.pop(); // handle ".."
            }
            other => components.push(other.as_os_str().to_string_lossy().to_string()),
        }
    }
    components.join("/")
}
```

**Step 1b: Add VFS path validation helper at the end of `vfs.rs`**

```rust
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
    if path.contains("..") {
        return Err(format!("path traversal not allowed in VFS: {path}"));
    }
    // Reject Windows device names
    let stem = path.split('/').last().unwrap_or(path).split('.').next().unwrap_or("");
    let upper = stem.to_uppercase();
    if matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "COM1" | "COM2" | "COM3" | "LPT1" | "LPT2" | "LPT3") {
        return Err(format!("reserved device name in VFS path: {path}"));
    }
    Ok(())
}
```

**Step 2: Add the module to `crates/sema-core/src/lib.rs`**

After the existing module declarations, add:

```rust
pub mod vfs;
```

**Step 3: Verify it compiles**

Run: `cargo build -p sema-core 2>&1 | tail -5`
Expected: Compiles without errors.

**Step 4: Commit**

```bash
git add crates/sema-core/src/vfs.rs crates/sema-core/src/lib.rs
git commit -m "feat: add thread-local VFS module in sema-core for embedded file access"
```

---

### Task 3: Intercept `file/read` and `file/exists?` with VFS

**Files:**
- Modify: `crates/sema-stdlib/src/io.rs`

**Step 1: Modify `file/read` (line 59-67) to check VFS first**

Replace the `file/read` registration with:

```rust
crate::register_fn_path_gated(env, sandbox, Caps::FS_READ, "file/read", &[0], |args| {
    check_arity!(args, "file/read", 1);
    let path = args[0]
        .as_str()
        .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
    // Check VFS first
    if let Some(data) = sema_core::vfs::vfs_read(path) {
        return String::from_utf8(data)
            .map(|s| Value::string(&s))
            .map_err(|e| SemaError::Io(format!("file/read {path}: invalid UTF-8 in VFS: {e}")));
    }
    let content = std::fs::read_to_string(path)
        .map_err(|e| SemaError::Io(format!("file/read {path}: {e}")))?;
    Ok(Value::string(&content))
});
```

**Step 2: Modify `file/read-bytes` (line 82-97) to check VFS first**

Replace with:

```rust
crate::register_fn_path_gated(
    env,
    sandbox,
    Caps::FS_READ,
    "file/read-bytes",
    &[0],
    |args| {
        check_arity!(args, "file/read-bytes", 1);
        let path = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        // Check VFS first
        if let Some(data) = sema_core::vfs::vfs_read(path) {
            return Ok(Value::bytevector(data));
        }
        let bytes = std::fs::read(path)
            .map_err(|e| SemaError::Io(format!("file/read-bytes {path}: {e}")))?;
        Ok(Value::bytevector(bytes))
    },
);
```

**Step 3: Modify `file/exists?` (line 119-125) to check VFS first**

Replace with:

```rust
crate::register_fn_path_gated(env, sandbox, Caps::FS_READ, "file/exists?", &[0], |args| {
    check_arity!(args, "file/exists?", 1);
    let path = args[0]
        .as_str()
        .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
    // Check VFS first
    if let Some(exists) = sema_core::vfs::vfs_exists(path) {
        if exists {
            return Ok(Value::bool(true));
        }
    }
    Ok(Value::bool(std::path::Path::new(path).exists()))
});
```

**Step 4: Modify `file/read-lines` (line 431-447) to check VFS first**

Replace with:

```rust
crate::register_fn_path_gated(
    env,
    sandbox,
    Caps::FS_READ,
    "file/read-lines",
    &[0],
    |args| {
        check_arity!(args, "file/read-lines", 1);
        let path = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        // Check VFS first
        let content = if let Some(data) = sema_core::vfs::vfs_read(path) {
            String::from_utf8(data)
                .map_err(|e| SemaError::Io(format!("file/read-lines {path}: invalid UTF-8 in VFS: {e}")))?
        } else {
            std::fs::read_to_string(path)
                .map_err(|e| SemaError::Io(format!("file/read-lines {path}: {e}")))?
        };
        let lines: Vec<Value> = content.split('\n').map(Value::string).collect();
        Ok(Value::list(lines))
    },
);
```

**Step 5: Verify it compiles**

Run: `cargo build -p sema-stdlib 2>&1 | tail -5`
Expected: Compiles without errors.

**Step 6: Commit**

```bash
git add crates/sema-stdlib/src/io.rs
git commit -m "feat: intercept file/read, file/read-bytes, file/exists?, file/read-lines with VFS"
```

---

### Task 4: Intercept `import` and `load` with VFS

> **Simplified from original plan:** VFS is already in sema-core (Task 2), so sema-eval can access it directly. No dependency gymnastics needed.

**Files:**
- Modify: `crates/sema-eval/src/special_forms.rs`

**Step 2: Modify `eval_import` in `crates/sema-eval/src/special_forms.rs` (line 1131-1215)**

In the `eval_import` function, after path resolution (line 1148) and before the canonicalize call (line 1166), add a VFS check:

```rust
// Check VFS before hitting the filesystem
if sema_core::vfs::is_vfs_active() {
    let vfs_path = if std::path::Path::new(path_str).is_absolute() {
        path_str.to_string()
    } else {
        resolved.to_string_lossy().to_string()
    };

    // Check cache first (using resolved as key since we can't canonicalize VFS paths)
    if let Some(cached) = ctx.get_cached_module(&resolved) {
        copy_exports_to_env(&cached, &selective, env)?;
        return Ok(Trampoline::Value(Value::nil()));
    }

    if let Some(content_bytes) = sema_core::vfs::vfs_read(&vfs_path) {
        let content = String::from_utf8(content_bytes)
            .map_err(|e| SemaError::Io(format!("import {path_str}: invalid UTF-8 in VFS: {e}")))?;

        ctx.begin_module_load(&resolved)?;

        let load_result: Result<std::collections::BTreeMap<String, Value>, SemaError> = (|| {
            let (exprs, spans) = sema_reader::read_many_with_spans(&content)?;
            ctx.merge_span_table(spans);

            let module_env = eval::create_module_env(env);
            ctx.push_file_path(resolved.clone());
            ctx.clear_module_exports();

            let eval_result = (|| {
                for expr in &exprs {
                    eval::eval_value(ctx, expr, &module_env)?;
                }
                Ok(())
            })();

            ctx.pop_file_path();
            let declared = ctx.take_module_exports();
            eval_result?;

            Ok(collect_module_exports(&module_env, declared.as_deref()))
        })();

        ctx.end_module_load(&resolved);
        let exports = load_result?;

        ctx.cache_module(resolved, exports.clone());
        copy_exports_to_env(&exports, &selective, env)?;

        return Ok(Trampoline::Value(Value::nil()));
    }
}
```

Insert this block after line 1163 (after the first cache check on the `resolved` path) and before line 1166 (the `canonicalize` call). This way, if the file is in the VFS, we skip the filesystem entirely.

**Step 2b: Modify `eval_load` in `crates/sema-eval/src/special_forms.rs`**

Apply the same VFS-first pattern to `eval_load`. Before the `std::fs::read_to_string` call, add:

```rust
// Check VFS before hitting the filesystem
if sema_core::vfs::is_vfs_active() {
    if let Some(content_bytes) = sema_core::vfs::vfs_read(path_str) {
        let content = String::from_utf8(content_bytes)
            .map_err(|e| SemaError::Io(format!("load {path_str}: invalid UTF-8 in VFS: {e}")))?;
        let (exprs, spans) = sema_reader::read_many_with_spans(&content)?;
        ctx.merge_span_table(spans);
        for expr in &exprs {
            eval::eval_value(ctx, expr, env)?;
        }
        return Ok(Trampoline::Value(Value::nil()));
    }
}
```

**Step 3: Verify it compiles**

Run: `cargo build -p sema-eval 2>&1 | tail -5`
Expected: Compiles without errors.

**Step 4: Commit**

```bash
git add crates/sema-eval/src/special_forms.rs
git commit -m "feat: VFS interception for import and load"
```

---

### Task 5: Implement archive serialization/deserialization

**Files:**
- Create: `crates/sema/src/archive.rs`

This module handles reading and writing the VFS archive format (header + metadata + TOC + file data + trailer).

**Step 1: Create `crates/sema/src/archive.rs`**

```rust
use std::collections::HashMap;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

const MAGIC: &[u8; 8] = b"SEMAEXEC";
const TRAILER_SIZE: u64 = 16; // archive_size(u64) + magic(8)
const FORMAT_VERSION: u16 = 1;

/// Simple CRC32 (IEEE) implementation — no external dependency needed.
fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFFFFFF;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB88320 } else { crc >> 1 };
        }
    }
    !crc
}

/// Represents a deserialized VFS archive.
pub struct Archive {
    pub format_version: u16,
    pub flags: u16,
    pub metadata: HashMap<String, Vec<u8>>,
    pub files: HashMap<String, Vec<u8>>,
}

/// Check if a file has an appended SEMAEXEC archive by reading its trailer.
pub fn has_embedded_archive(path: &Path) -> io::Result<bool> {
    let mut file = std::fs::File::open(path)?;
    let file_len = file.metadata()?.len();
    if file_len < TRAILER_SIZE {
        return Ok(false);
    }
    file.seek(SeekFrom::End(-(TRAILER_SIZE as i64)))?;
    let mut trailer = [0u8; 16];
    file.read_exact(&mut trailer)?;
    Ok(&trailer[8..16] == MAGIC)
}

/// Extract the embedded archive from a bundled binary.
pub fn extract_archive(path: &Path) -> io::Result<Archive> {
    let data = std::fs::read(path)?;
    let len = data.len();

    if len < TRAILER_SIZE as usize {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "file too small"));
    }

    // Read trailer
    let trailer = &data[len - TRAILER_SIZE as usize..];
    if &trailer[8..16] != MAGIC {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "missing SEMAEXEC magic"));
    }
    let archive_size = u64::from_le_bytes(trailer[0..8].try_into().unwrap()) as usize;

    if archive_size + TRAILER_SIZE as usize > len {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "archive_size exceeds file size"));
    }

    let archive_start = len - TRAILER_SIZE as usize - archive_size;
    let archive_data = &data[archive_start..archive_start + archive_size];

    deserialize_archive(archive_data)
}

fn deserialize_archive(data: &[u8]) -> io::Result<Archive> {
    let mut pos = 0;

    // Header
    if data.len() < 12 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "archive header too short"));
    }
    let format_version = u16::from_le_bytes([data[pos], data[pos + 1]]);
    pos += 2;
    let flags = u16::from_le_bytes([data[pos], data[pos + 1]]);
    pos += 2;
    let stored_checksum = u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);
    pos += 4;

    // Validate CRC32 over everything after the checksum field
    let computed_checksum = crc32(&data[pos..]);
    if stored_checksum != computed_checksum {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("archive checksum mismatch: expected {stored_checksum:08x}, got {computed_checksum:08x}"),
        ));
    }

    let metadata_count = u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]) as usize;
    pos += 4;

    if format_version > FORMAT_VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unsupported archive format version {format_version} (max supported: {FORMAT_VERSION})"),
        ));
    }

    // Metadata
    let mut metadata = HashMap::new();
    for _ in 0..metadata_count {
        let key_len = u16::from_le_bytes([data[pos], data[pos + 1]]) as usize;
        pos += 2;
        let key = std::str::from_utf8(&data[pos..pos + key_len])
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("invalid metadata key: {e}")))?
            .to_string();
        pos += key_len;
        let val_len = u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]) as usize;
        pos += 4;
        let val = data[pos..pos + val_len].to_vec();
        pos += val_len;
        metadata.insert(key, val);
    }

    // TOC
    let entry_count = u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]) as usize;
    pos += 4;

    struct TocEntry {
        path: String,
        offset: usize,
        size: usize,
    }

    let mut toc = Vec::with_capacity(entry_count);
    for _ in 0..entry_count {
        let path_len = u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]) as usize;
        pos += 4;
        let path = std::str::from_utf8(&data[pos..pos + path_len])
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("invalid TOC path: {e}")))?
            .to_string();
        pos += path_len;
        let offset = u64::from_le_bytes(data[pos..pos + 8].try_into().unwrap()) as usize;
        pos += 8;
        let size = u64::from_le_bytes(data[pos..pos + 8].try_into().unwrap()) as usize;
        pos += 8;
        toc.push(TocEntry { path, offset, size });
    }

    // File data starts at current pos
    let file_data_start = pos;
    let mut files = HashMap::new();
    for entry in &toc {
        let start = file_data_start + entry.offset;
        let end = start + entry.size;
        if end > data.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("TOC entry '{}' extends beyond archive data", entry.path),
            ));
        }
        files.insert(entry.path.clone(), data[start..end].to_vec());
    }

    Ok(Archive {
        format_version,
        flags,
        metadata,
        files,
    })
}

/// Serialize a VFS archive to bytes.
pub fn serialize_archive(
    metadata: &HashMap<String, Vec<u8>>,
    files: &HashMap<String, Vec<u8>>,
) -> Vec<u8> {
    let mut buf = Vec::new();

    // Header (checksum placeholder — filled in at the end)
    buf.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes()); // flags
    let checksum_offset = buf.len();
    buf.extend_from_slice(&0u32.to_le_bytes()); // CRC32 placeholder
    buf.extend_from_slice(&(metadata.len() as u32).to_le_bytes());

    // Metadata (sorted for determinism)
    let mut meta_keys: Vec<&String> = metadata.keys().collect();
    meta_keys.sort();
    for key in meta_keys {
        let val = &metadata[key];
        buf.extend_from_slice(&(key.len() as u16).to_le_bytes());
        buf.extend_from_slice(key.as_bytes());
        buf.extend_from_slice(&(val.len() as u32).to_le_bytes());
        buf.extend_from_slice(val);
    }

    // Build TOC and file data
    let mut file_data = Vec::new();
    let mut toc_entries: Vec<(&String, usize, usize)> = Vec::new();

    // Sort file keys for determinism
    let mut file_keys: Vec<&String> = files.keys().collect();
    file_keys.sort();

    for key in &file_keys {
        let data = &files[*key];
        let offset = file_data.len();
        file_data.extend_from_slice(data);
        toc_entries.push((key, offset, data.len()));
    }

    // TOC
    buf.extend_from_slice(&(toc_entries.len() as u32).to_le_bytes());
    for (path, offset, size) in &toc_entries {
        buf.extend_from_slice(&(path.len() as u32).to_le_bytes());
        buf.extend_from_slice(path.as_bytes());
        buf.extend_from_slice(&(*offset as u64).to_le_bytes());
        buf.extend_from_slice(&(*size as u64).to_le_bytes());
    }

    // File data
    buf.extend_from_slice(&file_data);

    // Compute CRC32 over everything after the checksum field and backfill
    let body_start = checksum_offset + 4;
    let checksum = crc32(&buf[body_start..]);
    buf[checksum_offset..checksum_offset + 4].copy_from_slice(&checksum.to_le_bytes());

    buf
}

/// Write a bundled executable: runtime binary + archive + trailer.
pub fn write_bundled_executable(
    runtime_path: &Path,
    output_path: &Path,
    archive_bytes: &[u8],
) -> io::Result<()> {
    let runtime = std::fs::read(runtime_path)?;

    let mut out = std::fs::File::create(output_path)?;
    out.write_all(&runtime)?;
    out.write_all(archive_bytes)?;
    // Trailer
    out.write_all(&(archive_bytes.len() as u64).to_le_bytes())?;
    out.write_all(MAGIC)?;
    out.flush()?;

    // chmod +x on unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(output_path)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(output_path, perms)?;
    }

    Ok(())
}
```

**Step 2: Add `mod archive;` to `crates/sema/src/main.rs` (near top, after use declarations)**

Add after the `use` declarations (around line 10):

```rust
mod archive;
```

**Step 3: Verify it compiles**

Run: `cargo build -p sema-lang 2>&1 | tail -5`
Expected: Compiles without errors.

**Step 4: Write a unit test for archive round-trip**

Add at the bottom of `crates/sema/src/archive.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_archive_roundtrip() {
        let mut metadata = HashMap::new();
        metadata.insert("sema-version".to_string(), b"1.10.0".to_vec());
        metadata.insert("entry-point".to_string(), b"__main__.semac".to_vec());

        let mut files = HashMap::new();
        files.insert("__main__.semac".to_string(), vec![0x00, 0x53, 0x45, 0x4D, 1, 2, 3, 4]);
        files.insert("lib/utils.sema".to_string(), b"(define (square x) (* x x))".to_vec());

        let archive_bytes = serialize_archive(&metadata, &files);
        let archive = deserialize_archive(&archive_bytes).unwrap();

        assert_eq!(archive.format_version, FORMAT_VERSION);
        assert_eq!(archive.metadata.get("sema-version").unwrap(), b"1.10.0");
        assert_eq!(archive.metadata.get("entry-point").unwrap(), b"__main__.semac");
        assert_eq!(archive.files.len(), 2);
        assert_eq!(archive.files.get("__main__.semac").unwrap(), &vec![0x00, 0x53, 0x45, 0x4D, 1, 2, 3, 4]);
        assert_eq!(archive.files.get("lib/utils.sema").unwrap(), b"(define (square x) (* x x))");
    }

    #[test]
    fn test_archive_empty() {
        let metadata = HashMap::new();
        let files = HashMap::new();

        let archive_bytes = serialize_archive(&metadata, &files);
        let archive = deserialize_archive(&archive_bytes).unwrap();

        assert_eq!(archive.format_version, FORMAT_VERSION);
        assert_eq!(archive.metadata.len(), 0);
        assert_eq!(archive.files.len(), 0);
    }
}
```

**Step 5: Run the tests**

Run: `cargo test -p sema-lang -- archive 2>&1`
Expected: Both tests pass.

**Step 6: Commit**

```bash
git add crates/sema/src/archive.rs crates/sema/src/main.rs
git commit -m "feat: implement VFS archive serialization and deserialization"
```

---

### Task 6: Implement import and load tracing

> **Updated:** Now traces both `(import "...")` and `(load "...")` forms. Only literal string arguments are traced; dynamic/variable arguments emit a warning.

**Files:**
- Create: `crates/sema/src/import_tracer.rs`

**Step 1: Create `crates/sema/src/import_tracer.rs`**

```rust
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Traces all transitive `(import "...")` and `(load "...")` dependencies starting from a root file.
/// Returns a map of relative_path -> file_contents for all discovered imports/loads.
pub fn trace_imports(root_file: &Path) -> Result<HashMap<String, Vec<u8>>, String> {
    let root_dir = root_file
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let root_dir = root_dir
        .canonicalize()
        .map_err(|e| format!("cannot resolve root directory: {e}"))?;

    let mut visited: HashSet<PathBuf> = HashSet::new();
    let mut result: HashMap<String, Vec<u8>> = HashMap::new();
    let mut warnings: Vec<String> = Vec::new();

    // The root file itself is compiled to bytecode, not included as source.
    // We only trace its imports.
    let root_canonical = root_file
        .canonicalize()
        .map_err(|e| format!("cannot resolve {}: {e}", root_file.display()))?;
    visited.insert(root_canonical.clone());

    let root_source = std::fs::read_to_string(&root_canonical)
        .map_err(|e| format!("cannot read {}: {e}", root_file.display()))?;

    trace_file_imports(
        &root_source,
        &root_canonical,
        &root_dir,
        &mut visited,
        &mut result,
        &mut warnings,
    )?;

    for warning in &warnings {
        eprintln!("Warning: {warning}");
    }

    Ok(result)
}

fn trace_file_imports(
    source: &str,
    file_path: &Path,
    root_dir: &Path,
    visited: &mut HashSet<PathBuf>,
    result: &mut HashMap<String, Vec<u8>>,
    warnings: &mut Vec<String>,
) -> Result<(), String> {
    let exprs = sema_reader::read_many(source)
        .map_err(|e| format!("parse error in {}: {}", file_path.display(), e.inner()))?;

    let file_dir = file_path
        .parent()
        .unwrap_or_else(|| Path::new("."));

    for expr in &exprs {
        extract_imports(expr, file_dir, file_path, root_dir, visited, result, warnings)?;
    }

    Ok(())
}

fn extract_imports(
    expr: &sema_core::Value,
    file_dir: &Path,
    file_path: &Path,
    root_dir: &Path,
    visited: &mut HashSet<PathBuf>,
    result: &mut HashMap<String, Vec<u8>>,
    warnings: &mut Vec<String>,
) -> Result<(), String> {
    let items = match expr.as_list() {
        Some(items) if !items.is_empty() => items,
        _ => return Ok(()),
    };

    // Check for (import "path" ...) and (load "path") forms
    if let Some(sym) = items[0].as_symbol() {
        if (sym == "import" || sym == "load") && items.len() >= 2 {
            if let Some(path_str) = items[1].as_str() {
                process_import(path_str, file_dir, file_path, root_dir, visited, result, warnings)?;
            } else {
                warnings.push(format!(
                    "dynamic {} at {} cannot be statically resolved — use --include to add it manually",
                    sym,
                    file_path.display()
                ));
            }
            return Ok(());
        }

        // Check for (module name (export ...) body...) — recurse into body
        if sym == "module" {
            for item in items.iter().skip(1) {
                extract_imports(item, file_dir, file_path, root_dir, visited, result, warnings)?;
            }
            return Ok(());
        }
    }

    // Recurse into nested forms (begin, let, define, etc.)
    for item in items.iter() {
        extract_imports(item, file_dir, file_path, root_dir, visited, result, warnings)?;
    }

    Ok(())
}

fn process_import(
    path_str: &str,
    file_dir: &Path,
    file_path: &Path,
    root_dir: &Path,
    visited: &mut HashSet<PathBuf>,
    result: &mut HashMap<String, Vec<u8>>,
    warnings: &mut Vec<String>,
) -> Result<(), String> {
    // Resolve path relative to the importing file
    let resolved = if Path::new(path_str).is_absolute() {
        PathBuf::from(path_str)
    } else {
        file_dir.join(path_str)
    };

    let canonical = resolved.canonicalize().map_err(|e| {
        format!(
            "imported file '{}' not found (referenced from {}): {e}",
            path_str,
            file_path.display()
        )
    })?;

    // Skip if already visited (circular import protection)
    if visited.contains(&canonical) {
        return Ok(());
    }
    visited.insert(canonical.clone());

    // Read the file
    let content = std::fs::read_to_string(&canonical).map_err(|e| {
        format!(
            "cannot read imported file '{}' (referenced from {}): {e}",
            path_str,
            file_path.display()
        )
    })?;

    // Compute relative path from root_dir for VFS key
    let rel_path = canonical
        .strip_prefix(root_dir)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| {
            // Absolute path outside project root — use the original import path
            path_str.to_string()
        });

    result.insert(rel_path, content.as_bytes().to_vec());

    // Recursively trace this file's imports
    trace_file_imports(&content, &canonical, root_dir, visited, result, warnings)?;

    Ok(())
}
```

**Step 2: Add `mod import_tracer;` to `crates/sema/src/main.rs`**

Add after the `mod archive;` line:

```rust
mod import_tracer;
```

**Step 3: Verify it compiles**

Run: `cargo build -p sema-lang 2>&1 | tail -5`
Expected: Compiles without errors.

**Step 4: Commit**

```bash
git add crates/sema/src/import_tracer.rs crates/sema/src/main.rs
git commit -m "feat: implement recursive import tracing for sema build"
```

---

### Task 7: Implement `sema build` CLI subcommand

**Files:**
- Modify: `crates/sema/src/main.rs`
- Modify: `crates/sema/Cargo.toml` (add `libsui` dependency)

**Step 0: Add `libsui` dependency**

In `crates/sema/Cargo.toml`, add under `[dependencies]`:

```toml
libsui = "0.5"  # check crates.io for latest version
```

**Step 1: Add `Build` variant to `Commands` enum (after `Disasm`, around line 211)**

```rust
/// Build a standalone executable from a sema source file
Build {
    /// Source file to compile and bundle
    file: String,

    /// Output executable path (default: filename without extension)
    #[arg(short, long)]
    output: Option<String>,

    /// Additional files or directories to bundle (repeatable)
    #[arg(long = "include", action = clap::ArgAction::Append)]
    includes: Vec<String>,

    /// Sema binary to use as runtime base (default: current executable)
    #[arg(long)]
    runtime: Option<String>,
},
```

**Step 2: Add the match arm in the `if let Some(command)` block (after the `Disasm` arm, around line 254)**

```rust
Commands::Build {
    file,
    output,
    includes,
    runtime,
} => {
    run_build(&file, output.as_deref(), &includes, runtime.as_deref());
}
```

**Step 3: Implement `run_build` function**

Add before the `run_ast` function (around line 417):

```rust
fn run_build(file: &str, output: Option<&str>, includes: &[String], runtime: Option<&str>) {
    let path = std::path::Path::new(file);
    if !path.exists() {
        eprintln!("Error: file not found: {file}");
        std::process::exit(1);
    }

    let root_dir = path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .canonicalize()
        .unwrap_or_else(|e| {
            eprintln!("Error: cannot resolve directory: {e}");
            std::process::exit(1);
        });

    // Step 1: Compile source to bytecode
    eprintln!("  Compiling {file}");
    let source = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error reading {file}: {e}");
            std::process::exit(1);
        }
    };

    let source_hash = crc32_simple(source.as_bytes());
    let sandbox = sema_core::Sandbox::allow_all();
    let interpreter = Interpreter::new_with_sandbox(&sandbox);

    let result = match interpreter.compile_to_bytecode(&source) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Compile error: {}", e.inner());
            std::process::exit(1);
        }
    };

    let bytecode = match sema_vm::serialize_to_bytes(&result, source_hash) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("Serialization error: {}", e.inner());
            std::process::exit(1);
        }
    };

    // Step 2: Trace imports
    let imports = match import_tracer::trace_imports(path) {
        Ok(imports) => imports,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    };

    // Step 3: Gather --include assets
    let mut asset_files: std::collections::HashMap<String, Vec<u8>> = std::collections::HashMap::new();
    for include_path in includes {
        let inc = std::path::Path::new(include_path);
        if !inc.exists() {
            eprintln!("Error: --include path not found: {include_path}");
            std::process::exit(1);
        }
        if inc.is_dir() {
            collect_directory_files(inc, inc, &mut asset_files);
        } else {
            let rel = inc
                .strip_prefix(&root_dir)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| {
                    inc.file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| include_path.clone())
                });
            match std::fs::read(inc) {
                Ok(data) => {
                    asset_files.insert(rel, data);
                }
                Err(e) => {
                    eprintln!("Error reading {include_path}: {e}");
                    std::process::exit(1);
                }
            }
        }
    }

    // Step 4: Build VFS
    let mut vfs_files: std::collections::HashMap<String, Vec<u8>> = std::collections::HashMap::new();
    vfs_files.insert("__main__.semac".to_string(), bytecode);
    for (path, data) in &imports {
        vfs_files.insert(path.clone(), data.clone());
    }
    for (path, data) in &asset_files {
        vfs_files.insert(path.clone(), data.clone());
    }

    // Step 5: Build metadata
    let mut metadata: std::collections::HashMap<String, Vec<u8>> = std::collections::HashMap::new();
    metadata.insert(
        "sema-version".to_string(),
        env!("CARGO_PKG_VERSION").as_bytes().to_vec(),
    );
    metadata.insert(
        "build-timestamp".to_string(),
        build_timestamp().as_bytes().to_vec(),
    );
    metadata.insert(
        "entry-point".to_string(),
        b"__main__.semac".to_vec(),
    );
    metadata.insert(
        "build-root".to_string(),
        root_dir.to_string_lossy().as_bytes().to_vec(),
    );

    // Step 6: Serialize archive
    let archive_bytes = archive::serialize_archive(&metadata, &vfs_files);

    // Step 7: Determine runtime and output paths
    let runtime_path = match runtime {
        Some(r) => std::path::PathBuf::from(r),
        None => std::env::current_exe().unwrap_or_else(|e| {
            eprintln!("Error: cannot determine current executable: {e}");
            std::process::exit(1);
        }),
    };

    let out_path = match output {
        Some(o) => std::path::PathBuf::from(o),
        None => {
            let stem = path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "output".to_string());
            std::path::PathBuf::from(stem)
        }
    };

    // Step 8: Write bundled executable
    let import_count = imports.len();
    let asset_count = asset_files.len();
    let total_bundled_size: usize = vfs_files.values().map(|v| v.len()).sum();

    eprintln!(
        "  Bundling {} import{}, {} asset{} ({:.1} KB)",
        import_count,
        if import_count == 1 { "" } else { "s" },
        asset_count,
        if asset_count == 1 { "" } else { "s" },
        total_bundled_size as f64 / 1024.0,
    );

    // Platform-specific binary injection
    match write_executable_platform(&runtime_path, &out_path, &archive_bytes) {
        Ok(()) => {}
        Err(e) => {
            eprintln!("Error writing executable: {e}");
            std::process::exit(1);
        }
    }

    let final_size = std::fs::metadata(&out_path).map(|m| m.len()).unwrap_or(0);
    eprintln!(
        "  Writing {} ({:.1} MB)",
        out_path.display(),
        final_size as f64 / (1024.0 * 1024.0),
    );
    eprintln!("  Done.");
}

fn collect_directory_files(
    dir: &std::path::Path,
    base: &std::path::Path,
    files: &mut std::collections::HashMap<String, Vec<u8>>,
) {
    let entries = std::fs::read_dir(dir).unwrap_or_else(|e| {
        eprintln!("Error reading directory {}: {e}", dir.display());
        std::process::exit(1);
    });
    for entry in entries.flatten() {
        let entry_path = entry.path();
        if entry_path.is_dir() {
            collect_directory_files(&entry_path, base, files);
        } else {
            let data = std::fs::read(&entry_path).unwrap_or_else(|e| {
                eprintln!("Error reading {}: {e}", entry_path.display());
                std::process::exit(1);
            });
            let rel = entry_path
                .strip_prefix(base)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| entry_path.to_string_lossy().to_string());
            // Validate VFS path safety
            if let Err(e) = sema_core::vfs::validate_vfs_path(&rel) {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
            files.insert(rel, data);
        }
    }
}

fn build_timestamp() -> String {
    // Store Unix timestamp as string — simple, no date formatting needed.
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    secs.to_string()
}

/// Write the bundled executable using platform-appropriate injection.
/// Primary: libsui for Mach-O/PE section injection. Fallback: raw append for ELF.
fn write_executable_platform(
    runtime_path: &std::path::Path,
    output_path: &std::path::Path,
    archive_bytes: &[u8],
) -> std::io::Result<()> {
    let runtime = std::fs::read(runtime_path)?;
    let mut out = std::fs::File::create(output_path)?;

    #[cfg(target_os = "macos")]
    {
        use std::io::Write;
        // Inject as Mach-O section + ad-hoc re-sign (handles ARM64 code signing)
        libsui::Macho::from(runtime)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("Mach-O parse: {e}")))?
            .write_section("semaexec", archive_bytes.to_vec())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("Mach-O inject: {e}")))?
            .build_and_sign(&mut out)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("Mach-O sign: {e}")))?;
    }

    #[cfg(target_os = "windows")]
    {
        use std::io::Write;
        // Inject as PE resource
        libsui::Pe::new(&runtime)
            .write_resource("semaexec", archive_bytes.to_vec())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("PE inject: {e}")))?
            .build(&mut out)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("PE build: {e}")))?;
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        // Linux/ELF: raw append + trailer (ELF loaders ignore appended data)
        archive::write_bundled_executable(runtime_path, output_path, archive_bytes)?;
    }

    // chmod +x on unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(output_path)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(output_path, perms)?;
    }

    Ok(())
}
```

**Step 4: Verify it compiles**

Run: `cargo build -p sema-lang 2>&1 | tail -5`
Expected: Compiles without errors.

**Step 5: Commit**

```bash
git add crates/sema/src/main.rs
git commit -m "feat: implement sema build CLI subcommand"
```

---

### Task 8: Implement embedded archive loader at startup

**Files:**
- Modify: `crates/sema/src/main.rs`

**Step 1: Add the embedded archive detection at the very start of `main()`**

Replace the current `fn main() {` block start (line 214-215) with:

```rust
fn main() {
    // Check if this binary has an embedded SEMAEXEC archive.
    // If so, run as a standalone bundled executable.
    if let Some(exit_code) = try_run_embedded() {
        std::process::exit(exit_code);
    }

    let cli = Cli::parse();
    // ... rest of main unchanged
```

**Step 2: Implement `try_run_embedded()`**

Add before `main()`:

```rust
/// If this binary has an appended SEMAEXEC archive, extract it, initialize the VFS,
/// and execute the embedded bytecode. Returns Some(exit_code) if embedded, None otherwise.
fn try_run_embedded() -> Option<i32> {
    let exe_path = std::env::current_exe().ok()?;

    // Try named section first (macOS Mach-O / Windows PE via libsui),
    // fall back to trailer scan (Linux ELF raw append).
    let archive_data = if let Some(data) = libsui::find_section("semaexec") {
        data.to_vec()
    } else if archive::has_embedded_archive(&exe_path).ok()? {
        match std::fs::read(&exe_path) {
            Ok(data) => {
                // Extract archive bytes using trailer
                let len = data.len();
                let trailer = &data[len - 16..];
                let archive_size = u64::from_le_bytes(trailer[0..8].try_into().unwrap()) as usize;
                data[len - 16 - archive_size..len - 16].to_vec()
            }
            Err(_) => return None,
        }
    } else {
        return None;
    };

    let arch = match archive::deserialize_archive_from_bytes(&archive_data) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("Error: failed to load embedded archive: {e}");
            return Some(1);
        }
    };

    // Read entry point from metadata
    let entry_point = arch
        .metadata
        .get("entry-point")
        .and_then(|v| std::str::from_utf8(v).ok())
        .unwrap_or("__main__.semac")
        .to_string();

    // Get the bytecode
    let bytecode = match arch.files.get(&entry_point) {
        Some(b) => b.clone(),
        None => {
            eprintln!("Error: entry point '{entry_point}' not found in embedded archive");
            return Some(1);
        }
    };

    // Initialize VFS with all files
    sema_core::vfs::init_vfs(arch.files);

    // Initialize interpreter and run
    let sandbox = sema_core::Sandbox::allow_all();
    let interpreter = Interpreter::new_with_sandbox(&sandbox);

    // Auto-configure LLM
    let _ = interpreter.eval_str("(llm/auto-configure)");

    match run_bytecode_bytes(&interpreter, &bytecode) {
        Ok(_) => Some(0),
        Err(e) => {
            print_error(&e);
            Some(1)
        }
    }
}
```

**Step 3: Verify it compiles**

Run: `cargo build -p sema-lang 2>&1 | tail -5`
Expected: Compiles without errors.

**Step 4: Commit**

```bash
git add crates/sema/src/main.rs
git commit -m "feat: detect and run embedded SEMAEXEC archive at startup"
```

---

### Task 9: Integration tests

**Files:**
- Modify: `crates/sema/tests/integration_test.rs`

> **Updated:** Tests use `std::env::temp_dir()` with unique subdirectories instead of hardcoded `/tmp/` paths, avoiding collisions when tests run in parallel.

**Step 1: Add archive round-trip test via CLI**

```rust
fn test_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("sema-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn test_sema_build_basic() {
    let dir = test_dir("build-basic");
    let dir = dir.to_str().unwrap();

    let source = r#"(println "hello from bundled sema")"#;
    std::fs::write(format!("{dir}/hello.sema"), source).unwrap();

    // Build the executable
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_sema"))
        .args(["build", &format!("{dir}/hello.sema"), "-o", &format!("{dir}/hello")])
        .output()
        .expect("failed to run sema build");

    assert!(output.status.success(), "sema build failed: {}", String::from_utf8_lossy(&output.stderr));
    assert!(std::path::Path::new(&format!("{dir}/hello")).exists(), "output executable not created");

    // Run the bundled executable
    let run_output = std::process::Command::new(format!("{dir}/hello"))
        .output()
        .expect("failed to run bundled executable");

    assert!(run_output.status.success(), "bundled executable failed: {}", String::from_utf8_lossy(&run_output.stderr));
    assert_eq!(
        String::from_utf8_lossy(&run_output.stdout).trim(),
        "hello from bundled sema"
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn test_sema_build_with_imports() {
    let dir = test_dir("build-imports");
    let dir = dir.to_str().unwrap();
    std::fs::create_dir_all(format!("{dir}/lib")).unwrap();

    // Create a library module
    std::fs::write(
        format!("{dir}/lib/math.sema"),
        "(module math (export square) (define (square x) (* x x)))",
    ).unwrap();

    // Create main file that imports it
    std::fs::write(
        format!("{dir}/app.sema"),
        r#"(import "lib/math.sema") (println (square 7))"#,
    ).unwrap();

    // Build
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_sema"))
        .args(["build", &format!("{dir}/app.sema"), "-o", &format!("{dir}/app")])
        .output()
        .expect("failed to run sema build");

    assert!(output.status.success(), "sema build failed: {}", String::from_utf8_lossy(&output.stderr));

    // Remove the source files to prove the VFS is working
    std::fs::remove_dir_all(format!("{dir}/lib")).unwrap();
    std::fs::remove_file(format!("{dir}/app.sema")).unwrap();

    // Run the bundled executable
    let run_output = std::process::Command::new(format!("{dir}/app"))
        .output()
        .expect("failed to run bundled executable");

    assert!(run_output.status.success(), "bundled executable failed: {}", String::from_utf8_lossy(&run_output.stderr));
    assert_eq!(
        String::from_utf8_lossy(&run_output.stdout).trim(),
        "49"
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn test_sema_build_with_include() {
    let dir = test_dir("build-include");
    let dir = dir.to_str().unwrap();
    std::fs::create_dir_all(format!("{dir}/data")).unwrap();

    // Create a data file
    std::fs::write(format!("{dir}/data/config.json"), r#"{"name": "test"}"#).unwrap();

    // Create main file that reads the included data
    std::fs::write(
        format!("{dir}/app.sema"),
        r#"(println (file/read "data/config.json"))"#,
    ).unwrap();

    // Build with --include
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_sema"))
        .args([
            "build",
            &format!("{dir}/app.sema"),
            "--include", &format!("{dir}/data"),
            "-o", &format!("{dir}/app"),
        ])
        .output()
        .expect("failed to run sema build");

    assert!(output.status.success(), "sema build failed: {}", String::from_utf8_lossy(&output.stderr));

    // Remove source and data to prove VFS works
    std::fs::remove_dir_all(format!("{dir}/data")).unwrap();
    std::fs::remove_file(format!("{dir}/app.sema")).unwrap();

    // Run
    let run_output = std::process::Command::new(format!("{dir}/app"))
        .output()
        .expect("failed to run bundled executable");

    assert!(run_output.status.success(), "bundled executable failed: {}", String::from_utf8_lossy(&run_output.stderr));
    assert_eq!(
        String::from_utf8_lossy(&run_output.stdout).trim(),
        r#"{"name": "test"}"#
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn test_sema_build_passes_args() {
    let dir = test_dir("build-args");
    let dir = dir.to_str().unwrap();

    std::fs::write(
        format!("{dir}/args.sema"),
        r#"(println (length (sys/args)))"#,
    ).unwrap();

    // Build
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_sema"))
        .args(["build", &format!("{dir}/args.sema"), "-o", &format!("{dir}/args")])
        .output()
        .expect("failed to run sema build");

    assert!(output.status.success(), "sema build failed: {}", String::from_utf8_lossy(&output.stderr));

    // Run with args
    let run_output = std::process::Command::new(format!("{dir}/args"))
        .args(["--foo", "bar"])
        .output()
        .expect("failed to run bundled executable");

    assert!(run_output.status.success());
    // argv should be: ["/tmp/.../args", "--foo", "bar"] = 3
    assert_eq!(
        String::from_utf8_lossy(&run_output.stdout).trim(),
        "3"
    );

    let _ = std::fs::remove_dir_all(dir);
}
```

**Step 2: Run the tests**

Run: `cargo test -p sema-lang --test integration_test -- test_sema_build 2>&1`
Expected: All 4 tests pass.

**Step 3: Commit**

```bash
git add crates/sema/tests/integration_test.rs
git commit -m "test: add integration tests for sema build"
```

---

### Task 10: Document the archive format in website docs

**Files:**
- Create: `website/docs/internals/executable-format.md`

**Step 1: Write the format documentation**

Create the file with the complete specification of the bundled executable format, including:
- Overview of how `sema build` works
- Binary layout diagram
- Trailer format (frozen)
- Archive format (versioned)
- Metadata keys
- TOC entry format
- VFS path conventions
- Runtime behavior

Reference `docs/plans/2026-02-20-sema-build-design.md` for the content. This should be a standalone reference doc, similar in style to `website/docs/internals/bytecode-format.md`.

**Step 2: Commit**

```bash
git add website/docs/internals/executable-format.md
git commit -m "docs: add executable format specification for sema build"
```

---

### Task 11: Final verification and cleanup

**Step 1: Run full test suite**

Run: `make test`
Expected: All tests pass (including new build tests).

**Step 2: Run lint**

Run: `make lint`
Expected: No warnings.

**Step 3: Manual smoke test**

```bash
cargo build --release
./target/release/sema build examples/hello.sema -o /tmp/hello-standalone
/tmp/hello-standalone
```

Expected: Prints the hello example output.

**Step 4: Test with an import-heavy example if one exists**

Check `examples/` for files that use `import`, build and run them.

**Step 5: Final commit if any cleanup needed**

```bash
git add -A
git commit -m "chore: cleanup after sema build implementation"
```
