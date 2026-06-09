# `sema build` — Standalone Executable Builder

**Date:** 2026-02-20
**Status:** Approved
**Issue:** https://github.com/HelgeSverre/sema/issues/9

## Overview

`sema build` compiles a sema program into a standalone executable by appending a VFS archive (compiled bytecode + imports + assets) to the sema runtime binary. The resulting binary is self-contained — no sema installation needed to run it.

```bash
sema build app.sema -o myapp          # basic
sema build app.sema --include data/   # with assets
./myapp --name hello                  # runs standalone
```

## Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Runtime binary | Use current `sema` binary (full, not stripped) | Tree-walker required at runtime for HOFs, imports, eval. See `docs/plans/vm-runtime-limitations.md` |
| Config directory | `~/.sema/` with `SEMA_HOME` env override | Simple, discoverable, matches cargo/deno/bun precedent |
| Import handling | Auto-trace and bundle all transitive imports | User expects standalone output |
| Asset handling | Explicit `--include` flag | Static analysis of `file/read` is unreliable; `sema.toml` manifest in future |
| CLI args | Pass-through `argv` as-is | Standard behavior, script uses `(sys/args)` |
| VFS implementation | Thread-local `HashMap` in sema-core | Needs access from both sema-eval (import) and sema-stdlib (file/read). Matches eval callback pattern. Document multi-interpreter contamination risk for future fix. |
| Import tracing | Trace literal `(import ...)` and `(load ...)` statically | Best-effort; macro-generated or dynamic imports cannot be traced statically — use `--include` as escape hatch |
| Binary injection | Platform-aware: Mach-O section (macOS), raw append (Linux), PE section (Windows) | Raw append breaks macOS code signing on ARM64 and Windows Authenticode. See [Platform-Specific Binary Injection](#platform-specific-binary-injection). |
| Cross-compilation | Deferred to v2 | Should work offline; future: pre-download runtimes to `~/.sema/cache/runtimes/` |

## Binary Format

### Trailer (last 16 bytes — frozen, never changes)

```
archive_size: u64 LE     ← size of the VFS archive in bytes
magic: "SEMAEXEC"        ← 8 bytes, detection marker
```

The trailer is the only thing the loader reads first. Its format is permanent — old loaders can always detect new binaries (and reject if archive format version is too new).

> **Note:** On macOS and Windows, the archive is injected as a named binary section (not appended raw) — but the archive _contents_ use the same format. The trailer is only used on Linux/ELF where raw append is safe. See [Platform-Specific Binary Injection](#platform-specific-binary-injection).

### VFS Archive

```
┌─ Archive Header ──────────────────────┐
│  format_version: u16                  │  bump when archive layout changes (v1 initially)
│  flags: u16                           │  reserved bitfield
│  archive_checksum: u32                │  CRC32 of everything after this field
│  metadata_count: u32                  │
│  ┌─ Metadata entries ───────────────┐ │
│  │ key_len(u16) + key(utf8)         │ │
│  │ val_len(u32) + val(bytes)        │ │
│  │ ...repeats metadata_count times  │ │
│  └──────────────────────────────────┘ │
├─ TOC ─────────────────────────────────┤
│  entry_count: u32                     │
│  ┌─ TOC entries ────────────────────┐ │
│  │ path_len(u32) + path(utf8)       │ │
│  │ offset(u64) + size(u64)          │ │
│  │ ...repeats entry_count times     │ │
│  └──────────────────────────────────┘ │
├─ File data ───────────────────────────┤
│  raw bytes for all bundled files      │
│  (offsets relative to archive start)  │
└───────────────────────────────────────┘
```

### v1 Metadata Keys

| Key | Value | Description |
|-----|-------|-------------|
| `sema-version` | `"1.10.0"` | Sema version that built the executable |
| `build-timestamp` | Unix timestamp string (seconds since epoch) | When it was built |
| `entry-point` | `"__main__.semac"` | VFS path of the compiled entry bytecode |
| `build-root` | absolute path string | Original project root (for path normalization) |

### VFS Entry Conventions

| VFS Path | Contents |
|----------|----------|
| `__main__.semac` | Compiled bytecode of the entry file |
| `lib/utils.sema` | Auto-traced import (relative to project root) |
| `data.json` | Asset from `--include` |
| `prompts/system.txt` | Asset from `--include prompts/` |

## CLI Interface

```
sema build <file> [options]

Arguments:
  <file>               Source file to compile and bundle

Options:
  -o, --output <path>  Output executable path (default: filename without extension)
  --include <path>...  Additional files/directories to bundle (repeatable)
  --runtime <path>     Sema binary to use as runtime base (default: current exe)
```

## Build Flow

1. Read and parse the entry file
2. Trace all `(import ...)` and `(load ...)` dependencies recursively
   - Track visited files to handle circular imports
   - Warn on dynamic imports/loads that can't be resolved statically
   - Hard error on missing files
3. Compile the entry file to bytecode via `interpreter.compile_to_bytecode()`
4. Resolve all `--include` paths (expand directories recursively, hard error on unreadable files)
5. Build the VFS archive:
   - `__main__.semac` → compiled bytecode
   - Traced imports/loads → source files (parsed by tree-walker at runtime)
   - `--include` files → raw bytes
6. Populate metadata
7. Compute CRC32 checksum over the archive body
8. Inject archive into runtime binary (platform-specific, see below)
9. `chmod +x` on unix

## Platform-Specific Binary Injection

Raw appending bytes after a binary breaks code signing on macOS (ARM64 kernel kills signature-invalid binaries) and Windows (Authenticode invalidated). Each platform needs a different injection strategy.

### Primary approach: `libsui` (Rust crate)

Use the [`libsui`](https://github.com/nicholasgasior/libsui) crate — the same library Deno uses for `deno compile`:

| Platform | Strategy | Signing |
|----------|----------|---------|
| **Linux (ELF)** | Append archive + trailer to end of binary | N/A — ELF loaders ignore appended data |
| **macOS (Mach-O)** | Inject as Mach-O section named `semaexec` via `libsui::Macho::write_section()` + `build_and_sign()` | Ad-hoc re-signed automatically by libsui |
| **Windows (PE)** | Inject as PE resource named `semaexec` via `libsui::Pe::write_resource()` | Authenticode stripped (sign after build if needed) |

Build-time pseudocode:

```rust
match std::env::consts::OS {
    "linux" => {
        // Append: archive + trailer (raw append is safe on ELF)
        libsui::Elf::new(&runtime_bin).append("semaexec", &archive_bytes, &mut writer)?;
    }
    "macos" => {
        libsui::Macho::from(runtime_bin)?
            .write_section("semaexec", archive_bytes)?
            .build_and_sign(&mut writer)?;
    }
    "windows" => {
        libsui::Pe::new(&runtime_bin)
            .write_resource("semaexec", archive_bytes)?
            .build(&mut writer)?;
    }
}
```

Runtime detection:

```rust
// Try named section first (macOS/Windows), fall back to trailer scan (Linux)
fn find_embedded_archive() -> Option<Vec<u8>> {
    if let Some(data) = libsui::find_section("semaexec") {
        return Some(data.to_vec());
    }
    // Fallback: trailer scan for Linux/ELF
    try_read_trailer()
}
```

### Fallback approach: pre-allocated section

If `libsui` proves problematic (build speed, compatibility), the fallback is Bun's approach: compile the sema runtime with a pre-allocated zeroed section large enough for typical archives, and overwrite it at build time. This avoids section injection entirely but requires controlling the runtime binary's compilation and limits archive size to the pre-allocated space.

## Runtime Startup Flow

In `main()`, before clap parsing:

1. Get `std::env::current_exe()`
2. Try `libsui::find_section("semaexec")` for named section (macOS/Windows)
3. If not found: seek to last 16 bytes, check for `SEMAEXEC` trailer magic (Linux)
4. If found: deserialize archive, validate CRC32 checksum
5. Populate thread-local VFS with all entries
6. Find `entry-point` in metadata, read the bytecode from VFS
7. Initialize `Interpreter` (registers stdlib, eval callbacks, VM delegates)
8. Execute bytecode via `VM::execute()`
9. Exit with appropriate code

If no embedded archive found, proceed with normal CLI parsing.

## VFS Interception

Thread-local VFS in `sema-core/src/vfs.rs` (in sema-core so both sema-eval and sema-stdlib can access it):

```rust
thread_local! {
    static EMBEDDED_VFS: RefCell<Option<HashMap<String, Vec<u8>>>> = RefCell::new(None);
}
```

> **Known limitation:** Thread-local state is shared across all interpreter instances on the same thread. If multiple interpreters run sequentially (tests, embedding), VFS state can leak between them. Acceptable for v1; long-term, make VFS interpreter-owned.

**Intercepted functions** (read-only, check VFS first then fall back to filesystem):
- `file/read`, `file/read-bytes`, `file/read-lines`
- `file/exists?`
- `import` (via `__vm-import` and tree-walker `eval_import`)
- `load` (via tree-walker `eval_load`)

**Not intercepted** (always real filesystem):
- `file/write`, `file/append`, `file/delete`, `file/rename`, `file/mkdir`

**VFS path safety:** All paths stored in the archive are validated at build time — reject absolute paths, `..` segments, NUL bytes, and Windows device names. At runtime, VFS lookup is purely in-memory (no host FS mapping), so path traversal cannot escape the VFS sandbox.

## `~/.sema/` Home Directory

New utility in `sema-core/src/home.rs`:

```rust
pub fn sema_home() -> PathBuf
```

Resolution order: `$SEMA_HOME` > `$HOME/.sema` > `%USERPROFILE%\.sema` > `.sema` (fallback).

Subdirectory convention:
- `~/.sema/cache/` — temp files, future runtime downloads
- `~/.sema/history` — potential future REPL history location

Also exposed as `sys/sema-home` builtin.

## Future Work (Not in v1)

- **Cross-compilation** — download pre-built runtimes for other platforms to `~/.sema/cache/runtimes/`
- **`sema.toml` manifest** — declare includes, metadata, build options in a config file
- **Runtime-only binary** — requires decoupling VM from tree-walker (see `docs/plans/vm-runtime-limitations.md`)
- **Code signing** — proper Apple notarization / Authenticode signing (v1 uses ad-hoc signing via libsui)
- **Compression** — optionally compress VFS entries (zstd or deflate)
- **Interpreter-owned VFS** — move VFS from thread-local to interpreter-owned field to fix multi-interpreter contamination
- **Build-by-execution tracing** — run program during build to intercept dynamic imports/loads (alternative to static AST tracing)
