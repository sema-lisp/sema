---
outline: [2, 3]
---

# Bundled Executable Format

## Overview

`sema build` compiles a Sema program into a standalone executable by embedding a VFS (Virtual File System) archive into the Sema runtime binary. The resulting binary is self-contained and requires no Sema installation to run.

```
Entry file (.sema) → Compile to bytecode → Trace imports → Build VFS archive → Inject into runtime binary → Executable
```

Running a bundled executable skips CLI argument parsing, loads the embedded bytecode from the VFS archive, and executes it directly.

### CLI Interface

```bash
# Basic build
sema build script.sema                        # → ./script
sema build script.sema -o myapp               # explicit output path

# Bundle additional files
sema build script.sema --include data.json    # bundle a file
sema build script.sema --include assets/      # bundle a directory (recursive)

# Use a specific runtime binary
sema build script.sema --runtime /path/to/sema

# Cross-compile for another platform (downloads a cached runtime)
sema build script.sema --target linux         # x86_64-unknown-linux-gnu
sema build script.sema --target all           # every supported target
sema build --list-targets                     # list targets and aliases

# Run the resulting standalone executable
./myapp --name hello
```

### Options

| Option | Description |
|--------|-------------|
| `-o, --output <path>` | Output executable path (default: filename without extension) |
| `--include <path>...` | Additional files or directories to bundle (repeatable) |
| `--runtime <path>` | Sema binary to use as runtime base (default: current executable); conflicts with `--target` |
| `--target <target>` | Target triple or alias (`linux`, `macos`, `windows`, …) for cross-compilation; `all` builds every supported target |
| `--list-targets` | Show all supported target platforms and aliases |
| `--no-cache` | Skip the cached runtime and re-download it (no effect for host-target builds, which never download) |

## Binary Layout

The injection strategy varies by binary format — detected from the runtime binary's magic bytes, not the build host, so cross-compilation works from any platform — to preserve binary integrity and OS loader compatibility.

### Linux (ELF): Raw Append

```
┌─────────────────────────────┐
│  Original Sema Binary (ELF) │
├─────────────────────────────┤
│  VFS Archive                │
├─────────────────────────────┤
│  Trailer (16 bytes)         │
│    archive_size: u64 LE     │
│    magic: "SEMAEXEC"        │
└─────────────────────────────┘
```

ELF loaders ignore appended data, so the binary remains valid.

### macOS (Mach-O): Section Injection

```
┌─────────────────────────────┐
│  Modified Mach-O Binary     │
│  ├── Mach-O Header          │
│  ├── Load Commands          │
│  ├── ...segments...         │
│  └── "semaexec" section     │  ← VFS archive injected here
└─────────────────────────────┘
```

Injected via `libsui`, which ad-hoc re-signs the binary for macOS ARM64 compatibility.

### Windows (PE): Resource Injection

```
┌─────────────────────────────┐
│  Modified PE Binary         │
│  ├── PE Header              │
│  ├── .text, .data, ...      │
│  └── .rsrc                  │
│       └── "semaexec"        │  ← VFS archive injected here
└─────────────────────────────┘
```

Injected via `libsui`. Existing Authenticode signatures are stripped.

## Trailer Format

**16 bytes, frozen — only used on Linux/ELF.**

| Offset | Size | Type | Description |
|--------|------|------|-------------|
| 0 | 8 | u64 LE | Size of the VFS archive in bytes |
| 8 | 8 | bytes | Magic: `SEMAEXEC` (`0x53 0x45 0x4D 0x41 0x45 0x58 0x45 0x43`) |

The trailer format is permanent and will never change. Old loaders can always detect new binaries and reject them if the archive format version is unsupported.

On macOS and Windows, the archive is stored in a named binary section — no trailer is used.

## VFS Archive Format

The VFS archive is a flat binary format with a versioned header, metadata, table of contents, and file data.

All multi-byte integers are **little-endian**. All strings are **UTF-8**.

```
┌─ Archive Header ──────────────────────┐
│  format_version: u16                  │  Currently v1
│  flags: u16                           │  Reserved bitfield (must be 0)
│  archive_checksum: u32                │  CRC32-IEEE of all bytes after this field
│  metadata_count: u32                  │
│  ┌─ Metadata entries ───────────────┐ │
│  │ key_len(u16) + key(utf8)         │ │
│  │ val_len(u32) + val(bytes)        │ │
│  │ ...repeats metadata_count times  │ │
│  └──────────────────────────────────┘ │
├─ TOC (Table of Contents) ─────────────┤
│  entry_count: u32                     │
│  ┌─ TOC entries ────────────────────┐ │
│  │ path_len(u32) + path(utf8)       │ │
│  │ offset(u64) + size(u64)          │ │
│  │ ...repeats entry_count times     │ │
│  └──────────────────────────────────┘ │
├─ File data ───────────────────────────┤
│  raw bytes for all bundled files      │
│  (offsets relative to file data start)│
└───────────────────────────────────────┘
```

### Header

| Offset | Size | Type | Description |
|--------|------|------|-------------|
| 0 | 2 | u16 LE | `format_version` — currently `1` |
| 2 | 2 | u16 LE | `flags` — reserved for future use, must be `0` |
| 4 | 4 | u32 LE | `archive_checksum` — CRC32-IEEE of all bytes from offset 8 to end of archive |
| 8 | 4 | u32 LE | `metadata_count` — number of metadata key-value entries |

**Total header: 12 bytes**

### Metadata Entry

Repeated `metadata_count` times, immediately after the header.

| Field | Size | Type | Description |
|-------|------|------|-------------|
| `key_len` | 2 | u16 LE | Length of key string in bytes |
| `key` | `key_len` | UTF-8 | Metadata key |
| `val_len` | 4 | u32 LE | Length of value in bytes |
| `val` | `val_len` | bytes | Metadata value (opaque bytes, typically UTF-8) |

Unknown metadata keys are ignored by the loader (forward compatibility).

### v1 Metadata Keys

| Key | Value | Description |
|-----|-------|-------------|
| `sema-version` | e.g. `"1.10.0"` | Sema version that built the executable |
| `build-timestamp` | Unix timestamp string | Seconds since epoch when the executable was built |
| `entry-point` | `"__main__.semac"` | VFS path of the compiled entry bytecode |
| `build-root` | absolute path string | Original project root directory |

### TOC (Table of Contents)

Starts immediately after the last metadata entry.

| Field | Size | Type | Description |
|-------|------|------|-------------|
| `entry_count` | 4 | u32 LE | Number of file entries |

Each TOC entry:

| Field | Size | Type | Description |
|-------|------|------|-------------|
| `path_len` | 4 | u32 LE | Length of VFS path in bytes |
| `path` | `path_len` | UTF-8 | VFS path (relative, forward-slash separated) |
| `offset` | 8 | u64 LE | Byte offset from start of file data section |
| `size` | 8 | u64 LE | Size of file data in bytes |

### File Data

Raw concatenated bytes for all files, in TOC order. Offsets in TOC entries are relative to the start of this section (byte 0 = first byte after the last TOC entry).

### VFS Path Conventions

| VFS Path | Contents |
|----------|----------|
| `__main__.semac` | Compiled bytecode of the entry file (always present) |
| `lib/utils.sema` | Auto-traced import (relative to project root) |
| `github.com/user/repo` | Package entry (git-style, keyed by package name) |
| `github.com/user/repo/helpers.sema` | Package internal file (relative to packages dir) |
| `json-utils` | Package entry (registry short-name) |
| `json-utils/src/core.sema` | Package internal file (registry package) |
| `data.json` | Asset from `--include data.json` |
| `prompts/system.txt` | Asset from `--include prompts/` |

All VFS paths must be:

- Relative (no leading `/` or `\`)
- Forward-slash separated
- No `..` segments
- No NUL bytes
- No Windows reserved device names (`CON`, `PRN`, `AUX`, `NUL`, `COM1`–`COM3`, `LPT1`–`LPT3`)

Paths are validated at build time. Invalid paths cause a build error.

### Integrity

The `archive_checksum` is a **CRC32-IEEE** checksum (polynomial `0xEDB88320`, same as gzip/zlib) computed over all archive bytes from offset 8 (after the checksum field) to the end of the archive.

On load, the runtime recomputes the checksum and rejects the archive if it doesn't match. This detects accidental corruption but is not a cryptographic security feature.

## Runtime Startup

When a Sema binary starts, **before** CLI argument parsing:

1. Try `libsui::find_section("semaexec")` for named section (macOS/Windows)
2. If not found: read last 16 bytes, check for `SEMAEXEC` magic (Linux/ELF)
3. If archive found:
   - Deserialize and validate CRC32 checksum
   - Populate thread-local VFS with all archive files
   - Read `entry-point` from metadata (default: `__main__.semac`)
   - Load and execute the bytecode
   - Exit with appropriate status code
4. If no archive found: proceed with normal CLI parsing (REPL/interpreter mode)

## VFS Interception

When the VFS is active, the following functions check VFS first, then fall back to the real filesystem:

| Function | Behavior |
|----------|----------|
| `(file/read path)` | Read UTF-8 text from VFS or filesystem |
| `(file/read-bytes path)` | Read raw bytes from VFS or filesystem |
| `(file/read-lines path)` | Read lines from VFS or filesystem |
| `(file/exists? path)` | Check VFS first, then filesystem |
| `(import "module")` | Resolve relative to VFS if active |
| `(load "file.sema")` | Resolve relative to VFS if active |

Write operations (`file/write`, `file/append`, `file/delete`, etc.) always target the real filesystem.

## Build Flow

1. **Compile** the entry file to bytecode (`.semac` format)
2. **Trace** all `(import ...)` and `(load ...)` dependencies recursively
   - Circular imports are detected and handled
   - Dynamic imports (non-literal paths) emit a warning
3. **Collect** `--include` assets (directories are expanded recursively)
4. **Build** VFS archive with metadata and CRC32 checksum
5. **Inject** archive into runtime binary (format-aware: ELF append, Mach-O/PE via libsui)
6. **Set** executable permissions on Unix

## Cross-Compilation

`sema build --target <target>` produces executables for other platforms. Supported targets (matching the cargo-dist release matrix):

| Triple | Aliases |
|--------|---------|
| `aarch64-apple-darwin` | `macos`, `darwin` |
| `x86_64-apple-darwin` | `macos-intel`, `darwin-intel`, `macos-x86_64` |
| `x86_64-unknown-linux-gnu` | `linux` |
| `aarch64-unknown-linux-gnu` | `linux-arm`, `linux-aarch64` |
| `x86_64-pc-windows-msvc` | `windows`, `win` |

`--target all` builds for every supported target, producing one `<name>-<triple>` executable each.

Runtime binaries for non-host targets are downloaded from GitHub Releases (capped at 200 MB), verified against the published SHA256 checksum, and cached at `~/.sema/cache/runtimes/v{version}/{target}/sema[.exe]`. Cached runtimes are validated by magic bytes against the expected format for the target; `--no-cache` skips the cached copy and re-downloads. If the target matches the host, the local `sema` binary is used directly (no download, and `--no-cache` is a no-op). `SEMA_RUNTIME_BASE_URL` overrides the download location (for mirrors or air-gapped builds).

Injection is format-aware rather than host-specific — `libsui` performs Mach-O ad-hoc signing in pure Rust, so e.g. macOS ARM64 binaries can be produced from Linux.

## Platform Notes

| Platform | Injection | Signing | Notes |
|----------|-----------|---------|-------|
| Linux (ELF) | Raw append + trailer | N/A | ELF loaders ignore appended data |
| macOS (Mach-O) | `libsui` section injection | Ad-hoc re-signed | Re-sign with Developer ID for distribution |
| Windows (PE) | `libsui` resource injection | Authenticode stripped | Sema icon + `VERSIONINFO` resource embedded; re-sign with `signtool` if needed |

## Implementation

| Component | File |
|-----------|------|
| Archive serialization | `crates/sema/src/archive.rs` |
| Import tracer | `crates/sema/src/import_tracer.rs` |
| Cross-compilation (runtime download/cache) | `crates/sema/src/cross_compile.rs` |
| Build command | `crates/sema/src/main.rs` |
| VFS core | `crates/sema-core/src/vfs.rs` |
| VFS I/O interception | `crates/sema-stdlib/src/io.rs` |
| Import/load VFS interception | `crates/sema-eval/src/special_forms.rs` |

## Future Work

- **Compression** — optional zstd/deflate compression for VFS entries
- **Build options in `sema.toml`** — declare includes, metadata, and build options in the project manifest (`sema.toml` exists today for dependencies and formatter config, but `sema build` does not read it)
- **Slimmer runtime** — trim unused runtime components for smaller executables (requires architectural changes)
- **Code signing** — proper Apple notarization / Authenticode signing integration
