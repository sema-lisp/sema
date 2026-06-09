# `sema build --target` — Cross-Compilation

**Date:** 2026-02-24  
**Status:** Draft  
**Depends on:** `sema build` (completed)

## Overview

Add `--target <triple>` to `sema build` so users can produce standalone executables for platforms other than the host. The bytecode/VFS archive is already target-independent — the only missing piece is obtaining the correct runtime binary for the target platform and injecting the archive using the right binary format.

```bash
# Build for Linux from macOS
sema build app.sema --target x86_64-unknown-linux-gnu -o myapp-linux

# Build for Windows from macOS
sema build app.sema --target x86_64-pc-windows-msvc -o myapp.exe

# Build for all supported targets
sema build app.sema --target all
```

## Current State

- `sema build` uses `std::env::current_exe()` as the runtime base (or `--runtime` override)
- `write_executable_platform()` uses compile-time `#[cfg(target_os)]` to pick the injection strategy
- `libsui` (v0.13, from Deno) operates on raw bytes — `Elf`, `Macho`, `PortableExecutable` structs work on any host platform (no OS-specific syscalls for binary manipulation)
- GitHub releases publish binaries for all 5 targets via cargo-dist: `sema-lang-{target}.tar.xz` (`.zip` for Windows)
- `~/.sema/cache/` exists; `~/.sema/cache/runtimes/` was reserved in the design doc for this purpose
- SHA256 checksums are available for every release asset

## Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Runtime source | Download from GitHub Releases for the **matching sema version** | Ensures bytecode format compatibility; predictable URL pattern |
| Cache location | `~/.sema/cache/runtimes/v{VERSION}/{TARGET}/sema` | Avoids re-download; one binary per version+target pair |
| Integrity | Verify SHA256 from `.sha256` asset | Prevents corrupted/tampered downloads |
| Binary format detection | Runtime detection via magic bytes, not `#[cfg]` | Enables cross-platform injection from any host |
| `--target all` | Build for all 5 supported targets | Convenience for CI/release workflows |
| Version pinning | Always download the same version as the running `sema` | Bytecode format must match; avoids subtle incompatibilities |
| Offline support | `--runtime` flag still works as manual override | Users can pre-download or use custom builds |
| Network requirement | Fail fast with clear error if download needed and offline | No silent fallback to wrong binary |

## Supported Targets

These match cargo-dist configuration in `dist-workspace.toml`:

| Triple | OS | Arch | Archive format | Binary extension |
|--------|----|------|----------------|-----------------|
| `aarch64-apple-darwin` | macOS | ARM64 | `.tar.xz` | (none) |
| `x86_64-apple-darwin` | macOS | x86_64 | `.tar.xz` | (none) |
| `aarch64-unknown-linux-gnu` | Linux | ARM64 | `.tar.xz` | (none) |
| `x86_64-unknown-linux-gnu` | Linux | x86_64 | `.tar.xz` | (none) |
| `x86_64-pc-windows-msvc` | Windows | x86_64 | `.zip` | `.exe` |

Short aliases for convenience:

| Alias | Resolves to |
|-------|-------------|
| `linux` | `x86_64-unknown-linux-gnu` |
| `linux-arm` | `aarch64-unknown-linux-gnu` |
| `macos` | `aarch64-apple-darwin` |
| `macos-intel` | `x86_64-apple-darwin` |
| `windows` | `x86_64-pc-windows-msvc` |
| `all` | All 5 targets |

## CLI Interface

```
sema build <file> [options]

Options (new):
  --target <triple>    Target platform triple or alias (default: host)
                       Use "all" to build for all supported targets
  --target-version <v> Sema version to download runtime for (default: current)
```

When `--target` is specified:
- If it matches the host platform, use current executable (no download)
- Otherwise, download (or use cached) runtime binary for that target
- Inject archive using the correct binary format for the target

When `--target all` is specified:
- Output files are named `{stem}-{target}` (e.g., `myapp-x86_64-unknown-linux-gnu`)
- Windows target gets `.exe` suffix
- All targets built sequentially; any failure stops the build

## Download & Cache Strategy

### URL Pattern

```
https://github.com/HelgeSverre/sema/releases/download/v{VERSION}/sema-lang-{TARGET}.tar.xz
https://github.com/HelgeSverre/sema/releases/download/v{VERSION}/sema-lang-{TARGET}.tar.xz.sha256
```

Windows uses `.zip` instead of `.tar.xz`.

### Cache Layout

```
~/.sema/cache/runtimes/
  v1.10.0/
    aarch64-apple-darwin/
      sema                    # extracted binary
    x86_64-unknown-linux-gnu/
      sema
    x86_64-pc-windows-msvc/
      sema.exe
```

### Download Flow

```
1. Check cache: ~/.sema/cache/runtimes/v{VERSION}/{TARGET}/sema[.exe]
2. If cached and valid → use it
3. Download .tar.xz/.zip asset from GitHub Releases
4. Download .sha256 checksum file
5. Verify SHA256 of downloaded archive
6. Extract binary from archive → cache directory
7. Return path to cached binary
```

### Error Cases

| Scenario | Behavior |
|----------|----------|
| No network | Error: "Cannot download runtime for {target}. Use --runtime to provide a local binary." |
| Version not found | Error: "No release found for v{VERSION}. Available: ..." |
| Target not found | Error: "No binary available for {target}. Supported targets: ..." |
| SHA256 mismatch | Error: "Integrity check failed for {target} runtime. Expected {expected}, got {actual}." |
| Disk full | Pass through OS error |

## Binary Format Detection & Cross-Injection

### Current: compile-time dispatch (host-only)

```rust
// Current code — can only inject into HOST format
#[cfg(target_os = "macos")]   → libsui::Macho
#[cfg(target_os = "windows")] → libsui::PortableExecutable  
#[cfg(not(...))]               → raw append (ELF)
```

### New: runtime dispatch (target-aware)

Detect the target binary format by reading magic bytes from the runtime binary, then use the corresponding `libsui` struct:

```rust
enum BinaryFormat {
    MachO,   // starts with 0xFEEDFACF (64-bit) or 0xCFFAEDFE (universal)
    Elf,     // starts with 0x7F454C46 (\x7FELF)
    Pe,      // starts with 0x4D5A (MZ)
}

fn detect_binary_format(data: &[u8]) -> Result<BinaryFormat> {
    match &data[..4] {
        [0xCF, 0xFA, 0xED, 0xFE] | [0xFE, 0xED, 0xFA, 0xCF] => Ok(BinaryFormat::MachO),
        [0xCA, 0xFE, 0xBA, 0xBE] => Ok(BinaryFormat::MachO), // fat/universal
        [0x7F, b'E', b'L', b'F'] => Ok(BinaryFormat::Elf),
        [b'M', b'Z', ..] => Ok(BinaryFormat::Pe),
        _ => Err("unrecognized binary format"),
    }
}

fn write_executable_for_target(runtime: &[u8], output: &Path, archive: &[u8]) -> Result<()> {
    match detect_binary_format(runtime)? {
        BinaryFormat::MachO => {
            let mut out = File::create(output)?;
            libsui::Macho::from(runtime.to_vec())?
                .write_section("semaexec", archive.to_vec())?
                .build_and_sign(&mut out)?;
        }
        BinaryFormat::Pe => {
            let mut out = File::create(output)?;
            libsui::PortableExecutable::from(runtime)?
                .write_resource(&["semaexec"], archive.to_vec())?
                .build(&mut out)?;
        }
        BinaryFormat::Elf => {
            // Raw append: runtime + archive + trailer
            archive::write_bundled_executable_from_bytes(runtime, output, archive)?;
        }
    }
    // chmod +x on unix (skip for PE targets when on unix — won't hurt)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(output, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}
```

Key insight: `libsui::Macho` and `libsui::PortableExecutable` operate on byte buffers, not the OS, so Mach-O injection from Linux and PE injection from macOS both work fine.

## Implementation Plan

### Phase 1: Runtime binary format detection (refactor `write_executable_platform`)

**Files:** `crates/sema/src/main.rs`, `crates/sema/src/archive.rs`

1. Add `BinaryFormat` enum and `detect_binary_format()` function
2. Replace the `#[cfg]`-based `write_executable_platform()` with runtime-detected `write_executable_for_target()`
3. Add `write_bundled_executable_from_bytes()` to `archive.rs` (takes `&[u8]` instead of path)
4. Existing behavior unchanged — host builds still work identically
5. Add unit tests: detect ELF/Mach-O/PE from magic bytes

### Phase 2: Target resolution & runtime download

**Files:** `crates/sema/src/cross_compile.rs` (new), `crates/sema/src/main.rs`

1. Add `cross_compile.rs` module with:
   - `SUPPORTED_TARGETS` const array
   - `resolve_target_alias()` — expand short names to triples
   - `host_target()` — return the current host's target triple
   - `is_host_target()` — check if a target matches the host
   - `runtime_cache_path()` — compute `~/.sema/cache/runtimes/v{ver}/{target}/sema`
   - `download_runtime()` — download + verify + extract + cache
   - `ensure_runtime()` — check cache, download if needed, return path
2. New dependencies in `crates/sema/Cargo.toml`:
   - `xz2` — for `.tar.xz` decompression (bindings to liblzma, well-maintained)
   - `zip` — for `.zip` extraction (Windows release assets)
   - Already available: `reqwest`, `sha2`, `tar`, `flate2`

### Phase 3: CLI integration

**Files:** `crates/sema/src/main.rs`

1. Add `--target` option to the `Build` variant in `Commands` enum
2. Modify `run_build()`:
   - If `--target` is specified and isn't the host → call `ensure_runtime()` to get the target runtime path
   - If `--target all` → loop over all targets
   - Pass resolved runtime path to `write_executable_for_target()`
3. Handle output naming for `--target all` (append target triple to stem)

### Phase 4: Cache management (optional)

**Files:** `crates/sema/src/main.rs` or `crates/sema/src/cross_compile.rs`

1. `sema build --list-targets` — show available targets
2. `sema build --clean-cache` — remove cached runtimes (or fold into `sema cache clean`)

## Testing

| Test | Type | Location |
|------|------|----------|
| `detect_binary_format` with ELF/MachO/PE magic bytes | Unit | `archive.rs` or `cross_compile.rs` |
| `resolve_target_alias` maps short names correctly | Unit | `cross_compile.rs` |
| `host_target` returns valid triple | Unit | `cross_compile.rs` |
| `runtime_cache_path` produces correct paths | Unit | `cross_compile.rs` |
| Cross-inject Mach-O archive from non-macOS | Integration | Only testable in CI (needs both platforms) |
| Download + verify + extract runtime | Integration | Needs network; `#[ignore]` by default |
| `sema build --target linux` e2e | Integration | CI-only; build on macOS, verify ELF output |

## Open Questions

1. **Should `--target` require network on first use?** Yes — no bundled runtimes. The `--runtime` flag is the escape hatch for air-gapped environments.
2. **Universal/fat Mach-O binaries?** cargo-dist builds separate `aarch64` and `x86_64` Darwin binaries. We could offer a `macos-universal` alias that creates a fat binary using `lipo`, but this is a future enhancement.

## Future Work

- `sema build --target macos-universal` — lipo two Darwin binaries into a universal binary
- Pre-download all runtimes: `sema build --prefetch` for CI warm-up
- `sema.toml` manifest with `[build.targets]` array
- Build-time notarization for macOS (beyond ad-hoc signing)
