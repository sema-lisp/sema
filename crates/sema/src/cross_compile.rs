//! Cross-compilation support for `sema build --target`.
//!
//! Downloads and caches pre-built sema runtime binaries from GitHub Releases
//! for use as cross-compilation targets.

use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const GITHUB_REPO: &str = "HelgeSverre/sema";

/// Maximum runtime binary size we'll accept (200 MB).
const MAX_RUNTIME_SIZE: u64 = 200 * 1024 * 1024;

/// All supported target triples (must match cargo-dist targets in dist-workspace.toml).
pub const SUPPORTED_TARGETS: &[&str] = &[
    "aarch64-apple-darwin",
    "aarch64-unknown-linux-gnu",
    "x86_64-apple-darwin",
    "x86_64-unknown-linux-gnu",
    "x86_64-pc-windows-msvc",
];

/// Resolve a target alias to a full target triple.
///
/// Accepts full triples as-is, or short aliases like "linux", "macos", "windows".
pub fn resolve_target(target: &str) -> Result<&'static str, String> {
    if let Some(t) = SUPPORTED_TARGETS.iter().find(|&&t| t == target) {
        return Ok(t);
    }

    match target {
        "linux" => Ok("x86_64-unknown-linux-gnu"),
        "linux-arm" | "linux-aarch64" => Ok("aarch64-unknown-linux-gnu"),
        "macos" | "darwin" => Ok("aarch64-apple-darwin"),
        "macos-intel" | "darwin-intel" | "macos-x86_64" => Ok("x86_64-apple-darwin"),
        "windows" | "win" => Ok("x86_64-pc-windows-msvc"),
        _ => Err(format!(
            "unknown target '{target}'. Supported targets:\n{}",
            SUPPORTED_TARGETS
                .iter()
                .map(|t| format!("  - {t}"))
                .collect::<Vec<_>>()
                .join("\n")
        )),
    }
}

/// Return the current host's target triple.
pub fn host_target() -> &'static str {
    env!("TARGET")
}

/// Check whether a resolved target triple matches the current host.
pub fn is_host_target(target: &str) -> bool {
    target == host_target()
}

/// Return true if the target produces Windows PE binaries.
pub fn is_windows_target(target: &str) -> bool {
    target.ends_with("-pc-windows-msvc")
}

/// Return the binary name for a given target ("sema" or "sema.exe").
fn binary_name(target: &str) -> &'static str {
    if is_windows_target(target) {
        "sema.exe"
    } else {
        "sema"
    }
}

/// Compute the cache path for a runtime binary.
///
/// Layout: `~/.sema/cache/runtimes/v{version}/{target}/sema[.exe]`
pub fn runtime_cache_path(version: &str, target: &str) -> PathBuf {
    sema_core::sema_home()
        .join("cache")
        .join("runtimes")
        .join(format!("v{version}"))
        .join(target)
        .join(binary_name(target))
}

/// Detected binary format of an executable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryFormat {
    MachO,
    Elf,
    Pe,
}

impl std::fmt::Display for BinaryFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BinaryFormat::MachO => write!(f, "Mach-O"),
            BinaryFormat::Elf => write!(f, "ELF"),
            BinaryFormat::Pe => write!(f, "PE"),
        }
    }
}

/// Detect the binary format from magic bytes (first 4 bytes).
pub fn detect_binary_format(magic: &[u8]) -> Option<BinaryFormat> {
    if magic.len() < 4 {
        return None;
    }
    match magic[..4] {
        [0xCF, 0xFA, 0xED, 0xFE] | [0xFE, 0xED, 0xFA, 0xCF] => Some(BinaryFormat::MachO),
        [0xCA, 0xFE, 0xBA, 0xBE] => Some(BinaryFormat::MachO),
        [0x7F, b'E', b'L', b'F'] => Some(BinaryFormat::Elf),
        [b'M', b'Z', _, _] => Some(BinaryFormat::Pe),
        _ => None,
    }
}

/// Return the expected binary format for a target triple.
pub fn expected_format(target: &str) -> BinaryFormat {
    if is_windows_target(target) {
        BinaryFormat::Pe
    } else if target.contains("apple-darwin") {
        BinaryFormat::MachO
    } else {
        BinaryFormat::Elf
    }
}

/// Validate that a cached runtime binary exists and looks correct.
///
/// Checks: non-empty file, magic bytes match expected format for the target.
fn validate_cached_runtime(cache_path: &Path, target: &str) -> bool {
    let meta = match std::fs::metadata(cache_path) {
        Ok(m) => m,
        Err(_) => return false,
    };
    if meta.len() == 0 {
        return false;
    }
    let mut f = match std::fs::File::open(cache_path) {
        Ok(f) => f,
        Err(_) => return false,
    };
    let mut magic = [0u8; 4];
    if f.read_exact(&mut magic).is_err() {
        return false;
    }
    detect_binary_format(&magic) == Some(expected_format(target))
}

/// Ensure a runtime binary is available for the given target.
///
/// Returns the path to the cached binary. Downloads from GitHub Releases
/// if not already cached.
pub fn ensure_runtime(target: &str, no_cache: bool) -> Result<PathBuf, Box<dyn std::error::Error>> {
    // Defense-in-depth: reject targets not in our allow-list
    if !SUPPORTED_TARGETS.contains(&target) {
        return Err(format!("unsupported target: {target}").into());
    }

    let version = env!("CARGO_PKG_VERSION");
    let cache_path = runtime_cache_path(version, target);

    if !no_cache && validate_cached_runtime(&cache_path, target) {
        eprintln!("  Using cached runtime for {target}");
        return Ok(cache_path);
    }

    // Invalid or missing cache — remove stale file if present
    if cache_path.exists() {
        let _ = std::fs::remove_file(&cache_path);
    }

    eprintln!("  Downloading runtime for {target}...");
    download_runtime(version, target, &cache_path)?;
    Ok(cache_path)
}

/// Build a reqwest client with timeouts and a user-agent.
fn http_client() -> Result<reqwest::blocking::Client, reqwest::Error> {
    reqwest::blocking::Client::builder()
        .user_agent(format!("sema/{}", env!("CARGO_PKG_VERSION")))
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(120))
        .build()
}

/// Download, verify, and extract the runtime binary for a given target.
///
/// Streams the download to a temp file (hashing incrementally) to avoid
/// holding the entire archive in memory. Uses atomic write (temp file +
/// rename) for the extracted binary to prevent corrupt cache entries.
fn download_runtime(
    version: &str,
    target: &str,
    cache_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let is_windows = is_windows_target(target);
    let ext = if is_windows { "zip" } else { "tar.xz" };
    let pid = std::process::id();

    let base_url = std::env::var("SEMA_RUNTIME_BASE_URL").unwrap_or_else(|_| {
        format!("https://github.com/{GITHUB_REPO}/releases/download/v{version}")
    });
    let archive_url = format!("{base_url}/sema-lang-{target}.{ext}");
    let checksum_url = format!("{archive_url}.sha256");

    let client = http_client()?;

    // Download checksum first (small)
    let has_custom_base = std::env::var("SEMA_RUNTIME_BASE_URL").is_ok();
    let checksum_response = client
        .get(&checksum_url)
        .send()
        .map_err(|e| format!("failed to download checksum for {target}: {e}"))?;
    if !checksum_response.status().is_success() {
        let status = checksum_response.status();
        if status.as_u16() == 404 {
            let mut msg = format!(
                "No runtime release found for {target} (v{version}). \
                 You may be running a dev build without published release assets.\n  \
                 Hint: use `--runtime /path/to/sema` to provide a runtime binary manually."
            );
            if !has_custom_base {
                msg.push_str(
                    "\n  Hint: set SEMA_RUNTIME_BASE_URL to use a different release location.",
                );
            }
            return Err(msg.into());
        } else if status.as_u16() == 403 || status.as_u16() == 429 {
            return Err("GitHub rate-limited the download. \
                 Try again later or set SEMA_RUNTIME_BASE_URL to use a mirror."
                .into());
        } else {
            return Err(format!("failed to download checksum for {target}: HTTP {status}").into());
        }
    }
    let checksum_text = checksum_response
        .text()
        .map_err(|e| format!("failed to read checksum for {target}: {e}"))?;
    let expected_hash = parse_sha256_checksum(&checksum_text)
        .ok_or_else(|| format!("invalid checksum file for {target}: no valid SHA256 hash found"))?;

    // Create cache directory early so we can write temp files there
    let parent = cache_path
        .parent()
        .ok_or("cannot determine cache directory")?;
    std::fs::create_dir_all(parent)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
    }

    // Warn if disk space looks low
    #[cfg(unix)]
    {
        use std::mem::MaybeUninit;
        let mut stat = MaybeUninit::<libc::statvfs>::uninit();
        let c_path = std::ffi::CString::new(parent.to_string_lossy().as_bytes().to_vec())
            .unwrap_or_default();
        if unsafe { libc::statvfs(c_path.as_ptr(), stat.as_mut_ptr()) } == 0 {
            let stat = unsafe { stat.assume_init() };
            #[allow(clippy::unnecessary_cast)]
            let avail = stat.f_bavail as u64 * stat.f_frsize as u64;
            if avail < 200 * 1024 * 1024 {
                eprintln!(
                    "  Warning: only {}MB free on disk — download may fail",
                    avail / (1024 * 1024)
                );
            }
        }
    }

    // Stream download to a temp file while computing SHA256 incrementally
    let archive_tmp = cache_path.with_extension(format!("download-{pid}"));
    let mut hasher = Sha256::new();
    let download_result: Result<(), Box<dyn std::error::Error>> = (|| {
        let response = client
            .get(&archive_url)
            .send()
            .map_err(|e| format!("failed to download runtime for {target}: {e}"))?;
        if !response.status().is_success() {
            let status = response.status();
            if status.as_u16() == 404 {
                let mut msg = format!(
                    "No runtime release found for {target} (v{version}). \
                     You may be running a dev build without published release assets.\n  \
                     Hint: use `--runtime /path/to/sema` to provide a runtime binary manually."
                );
                if !has_custom_base {
                    msg.push_str(
                        "\n  Hint: set SEMA_RUNTIME_BASE_URL to use a different release location.",
                    );
                }
                return Err(msg.into());
            } else if status.as_u16() == 403 || status.as_u16() == 429 {
                return Err("GitHub rate-limited the download. \
                     Try again later or set SEMA_RUNTIME_BASE_URL to use a mirror."
                    .into());
            } else {
                return Err(
                    format!("failed to download runtime for {target}: HTTP {status}").into(),
                );
            }
        }
        let mut response = response;
        let mut archive_file = std::fs::File::create(&archive_tmp)?;
        let mut buf = [0u8; 65536];
        loop {
            let n = response.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            archive_file.write_all(&buf[..n])?;
        }
        archive_file.flush()?;
        Ok(())
    })();

    if let Err(e) = download_result {
        let _ = std::fs::remove_file(&archive_tmp);
        return Err(e);
    }

    // Verify SHA256
    let actual_hash = format!("{:x}", hasher.finalize());
    if actual_hash != expected_hash {
        let _ = std::fs::remove_file(&archive_tmp);
        return Err(format!(
            "SHA256 mismatch for {target} runtime.\n  Expected: {expected_hash}\n  Got:      {actual_hash}"
        )
        .into());
    }

    eprintln!("  Checksum verified ✓");

    // Read verified archive and extract the binary
    let archive_bytes = std::fs::read(&archive_tmp)?;
    let _ = std::fs::remove_file(&archive_tmp);

    let binary_tmp = cache_path.with_extension(format!("tmp-{pid}"));
    let extract_result = if is_windows {
        extract_zip(&archive_bytes, &binary_tmp, target)
    } else {
        extract_tar_xz(&archive_bytes, &binary_tmp, target)
    };

    if let Err(e) = extract_result {
        let _ = std::fs::remove_file(&binary_tmp);
        return Err(e);
    }

    // Atomic rename into final cache path
    if let Err(e) = std::fs::rename(&binary_tmp, cache_path) {
        let _ = std::fs::remove_file(&binary_tmp);
        return Err(format!("failed to finalize cached runtime: {e}").into());
    }

    eprintln!("  Cached at {}", cache_path.display());
    Ok(())
}

/// Parse a SHA256 hash from a checksum file.
///
/// Accepts formats like:
/// - `abc123def...`  (hash only)
/// - `abc123def...  filename.tar.xz`  (hash + filename, separated by whitespace)
///
/// Returns the lowercase hex hash, or None if invalid.
fn parse_sha256_checksum(text: &str) -> Option<String> {
    let hash = text.split_whitespace().next()?;
    // SHA256 is exactly 64 hex characters
    if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some(hash.to_lowercase())
}

/// Extract the sema binary from a `.tar.xz` archive.
///
/// Only extracts regular files; rejects symlinks, hardlinks, and oversized entries.
fn extract_tar_xz(
    data: &[u8],
    output_path: &Path,
    target: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let xz_reader = xz2::read::XzDecoder::new(data);
    let mut archive = tar::Archive::new(xz_reader);

    let bin_name = binary_name(target);

    for entry in archive.entries()? {
        let entry = entry?;

        // Only extract regular files
        if entry.header().entry_type() != tar::EntryType::Regular {
            continue;
        }

        let path = entry.path()?;
        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

        if file_name != bin_name {
            continue;
        }

        // Enforce size limit
        let size = entry.header().size()?;
        if size > MAX_RUNTIME_SIZE {
            return Err(
                format!("runtime binary too large ({size} bytes, max {MAX_RUNTIME_SIZE})").into(),
            );
        }

        // Stream to output with size limit
        let mut limited = entry.take(MAX_RUNTIME_SIZE);
        let mut out = std::fs::File::create(output_path)?;
        std::io::copy(&mut limited, &mut out)?;
        out.flush()?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(output_path, std::fs::Permissions::from_mode(0o755))?;
        }

        return Ok(());
    }

    Err(format!("'{bin_name}' not found in archive for {target}").into())
}

/// Extract the sema binary from a `.zip` archive.
///
/// Only extracts regular files; rejects directories and oversized entries.
fn extract_zip(
    data: &[u8],
    output_path: &Path,
    target: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let cursor = std::io::Cursor::new(data);
    let mut archive = zip::ZipArchive::new(cursor)?;

    let bin_name = binary_name(target);

    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;

        if file.is_dir() {
            continue;
        }

        let file_name = Path::new(file.name())
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");

        if file_name != bin_name {
            continue;
        }

        // Enforce size limit
        if file.size() > MAX_RUNTIME_SIZE {
            return Err(format!(
                "runtime binary too large ({} bytes, max {MAX_RUNTIME_SIZE})",
                file.size()
            )
            .into());
        }

        // Stream to output with size limit
        let mut limited = (&mut file).take(MAX_RUNTIME_SIZE);
        let mut out = std::fs::File::create(output_path)?;
        std::io::copy(&mut limited, &mut out)?;
        out.flush()?;

        return Ok(());
    }

    Err(format!("'{bin_name}' not found in zip archive for {target}").into())
}

/// List all supported target triples (for `--list-targets`).
pub fn list_targets() {
    let host = host_target();
    eprintln!("Supported targets:");
    for target in SUPPORTED_TARGETS {
        if *target == host {
            eprintln!("  {target} (host)");
        } else {
            eprintln!("  {target}");
        }
    }
    eprintln!();
    eprintln!("Special targets:");
    eprintln!("  web          → emits a .vfs archive for SemaWeb");
    eprintln!();
    eprintln!("Aliases:");
    eprintln!("  linux        → x86_64-unknown-linux-gnu");
    eprintln!("  linux-arm    → aarch64-unknown-linux-gnu");
    eprintln!("  macos        → aarch64-apple-darwin");
    eprintln!("  macos-intel  → x86_64-apple-darwin");
    eprintln!("  windows      → x86_64-pc-windows-msvc");
    eprintln!("  all          → all supported targets");
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // resolve_target
    // -----------------------------------------------------------------------

    #[test]
    fn test_resolve_target_full_triple() {
        for t in SUPPORTED_TARGETS {
            assert_eq!(resolve_target(t).unwrap(), *t);
        }
    }

    #[test]
    fn test_resolve_target_aliases() {
        assert_eq!(resolve_target("linux").unwrap(), "x86_64-unknown-linux-gnu");
        assert_eq!(resolve_target("macos").unwrap(), "aarch64-apple-darwin");
        assert_eq!(resolve_target("darwin").unwrap(), "aarch64-apple-darwin");
        assert_eq!(resolve_target("windows").unwrap(), "x86_64-pc-windows-msvc");
        assert_eq!(resolve_target("win").unwrap(), "x86_64-pc-windows-msvc");
        assert_eq!(
            resolve_target("linux-arm").unwrap(),
            "aarch64-unknown-linux-gnu"
        );
        assert_eq!(
            resolve_target("linux-aarch64").unwrap(),
            "aarch64-unknown-linux-gnu"
        );
        assert_eq!(
            resolve_target("macos-intel").unwrap(),
            "x86_64-apple-darwin"
        );
        assert_eq!(
            resolve_target("darwin-intel").unwrap(),
            "x86_64-apple-darwin"
        );
        assert_eq!(
            resolve_target("macos-x86_64").unwrap(),
            "x86_64-apple-darwin"
        );
    }

    #[test]
    fn test_resolve_target_unknown() {
        assert!(resolve_target("mips-unknown-linux-gnu").is_err());
        assert!(resolve_target("foobar").is_err());
        assert!(resolve_target("").is_err());
    }

    #[test]
    fn test_resolve_target_rejects_path_traversal() {
        assert!(resolve_target("../../../etc/passwd").is_err());
        assert!(resolve_target("x86_64-unknown-linux-gnu/../..").is_err());
        assert!(resolve_target("..").is_err());
    }

    // -----------------------------------------------------------------------
    // binary_name / is_windows_target
    // -----------------------------------------------------------------------

    #[test]
    fn test_binary_name() {
        assert_eq!(binary_name("x86_64-unknown-linux-gnu"), "sema");
        assert_eq!(binary_name("aarch64-apple-darwin"), "sema");
        assert_eq!(binary_name("x86_64-pc-windows-msvc"), "sema.exe");
    }

    #[test]
    fn test_is_windows_target() {
        assert!(is_windows_target("x86_64-pc-windows-msvc"));
        assert!(!is_windows_target("x86_64-unknown-linux-gnu"));
        assert!(!is_windows_target("aarch64-apple-darwin"));
        // Substring "windows" not at end shouldn't match
        assert!(!is_windows_target("windows-fake-target"));
    }

    // -----------------------------------------------------------------------
    // runtime_cache_path
    // -----------------------------------------------------------------------

    #[test]
    fn test_runtime_cache_path() {
        let p = runtime_cache_path("1.10.0", "x86_64-unknown-linux-gnu");
        assert!(p.ends_with("cache/runtimes/v1.10.0/x86_64-unknown-linux-gnu/sema"));

        let p = runtime_cache_path("1.10.0", "x86_64-pc-windows-msvc");
        assert!(p.ends_with("cache/runtimes/v1.10.0/x86_64-pc-windows-msvc/sema.exe"));
    }

    // -----------------------------------------------------------------------
    // host_target / is_host_target
    // -----------------------------------------------------------------------

    #[test]
    fn test_host_target_is_valid() {
        let host = host_target();
        assert!(
            SUPPORTED_TARGETS.contains(&host),
            "host target '{host}' should be in SUPPORTED_TARGETS"
        );
    }

    #[test]
    fn test_is_host_target() {
        assert!(is_host_target(host_target()));
        assert!(!is_host_target("some-fake-triple"));
    }

    // -----------------------------------------------------------------------
    // parse_sha256_checksum
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_checksum_hash_only() {
        let hash = "a".repeat(64);
        assert_eq!(parse_sha256_checksum(&hash), Some(hash.clone()));
    }

    #[test]
    fn test_parse_checksum_hash_with_filename() {
        let hash = "b".repeat(64);
        let input = format!("{hash}  sema-lang-x86_64-unknown-linux-gnu.tar.xz");
        assert_eq!(parse_sha256_checksum(&input), Some(hash));
    }

    #[test]
    fn test_parse_checksum_uppercase() {
        let hash = "A".repeat(64);
        assert_eq!(parse_sha256_checksum(&hash), Some("a".repeat(64)));
    }

    #[test]
    fn test_parse_checksum_with_whitespace() {
        let hash = "c".repeat(64);
        let input = format!("  {hash}  \n");
        assert_eq!(parse_sha256_checksum(&input), Some(hash));
    }

    #[test]
    fn test_parse_checksum_empty() {
        assert_eq!(parse_sha256_checksum(""), None);
    }

    #[test]
    fn test_parse_checksum_too_short() {
        assert_eq!(parse_sha256_checksum("abcdef"), None);
    }

    #[test]
    fn test_parse_checksum_too_long() {
        let hash = "a".repeat(65);
        assert_eq!(parse_sha256_checksum(&hash), None);
    }

    #[test]
    fn test_parse_checksum_non_hex() {
        let hash = format!("{}zz", "a".repeat(62));
        assert_eq!(parse_sha256_checksum(&hash), None);
    }

    // -----------------------------------------------------------------------
    // ensure_runtime — target validation
    // -----------------------------------------------------------------------

    #[test]
    fn test_ensure_runtime_rejects_unsupported_target() {
        let result = ensure_runtime("totally-fake-target", false);
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("unsupported"),
            "error should mention 'unsupported'"
        );
    }

    #[test]
    fn test_ensure_runtime_rejects_path_traversal() {
        let result = ensure_runtime("../../../etc/passwd", false);
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // validate_cached_runtime
    // -----------------------------------------------------------------------

    #[test]
    fn test_validate_cached_runtime_nonexistent() {
        assert!(!validate_cached_runtime(
            Path::new("/tmp/sema_test_nonexistent_binary"),
            "x86_64-unknown-linux-gnu"
        ));
    }

    #[test]
    fn test_validate_cached_runtime_empty_file() {
        let dir = std::env::temp_dir().join("sema_test_validate_empty");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("sema");
        std::fs::write(&path, b"").unwrap();
        assert!(!validate_cached_runtime(&path, "x86_64-unknown-linux-gnu"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_validate_cached_runtime_wrong_magic() {
        let dir = std::env::temp_dir().join("sema_test_validate_wrong_magic");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("sema");
        std::fs::write(&path, b"NOT_A_REAL_BINARY_AT_ALL").unwrap();
        assert!(!validate_cached_runtime(&path, "x86_64-unknown-linux-gnu"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_validate_cached_runtime_correct_elf_magic() {
        let dir = std::env::temp_dir().join("sema_test_validate_elf");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("sema");
        let mut data = vec![0x7F, b'E', b'L', b'F'];
        data.extend_from_slice(&[0u8; 100]); // padding
        std::fs::write(&path, &data).unwrap();
        assert!(validate_cached_runtime(&path, "x86_64-unknown-linux-gnu"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_validate_cached_runtime_correct_macho_magic() {
        let dir = std::env::temp_dir().join("sema_test_validate_macho");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("sema");
        let mut data = vec![0xCF, 0xFA, 0xED, 0xFE];
        data.extend_from_slice(&[0u8; 100]);
        std::fs::write(&path, &data).unwrap();
        assert!(validate_cached_runtime(&path, "aarch64-apple-darwin"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_validate_cached_runtime_correct_pe_magic() {
        let dir = std::env::temp_dir().join("sema_test_validate_pe");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("sema.exe");
        let mut data = vec![b'M', b'Z'];
        data.extend_from_slice(&[0u8; 100]);
        std::fs::write(&path, &data).unwrap();
        assert!(validate_cached_runtime(&path, "x86_64-pc-windows-msvc"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_validate_cached_runtime_elf_expected_but_pe_found() {
        let dir = std::env::temp_dir().join("sema_test_validate_cross_magic");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("sema");
        let mut data = vec![b'M', b'Z'];
        data.extend_from_slice(&[0u8; 100]);
        std::fs::write(&path, &data).unwrap();
        // Linux target but PE magic — should reject
        assert!(!validate_cached_runtime(&path, "x86_64-unknown-linux-gnu"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    // -----------------------------------------------------------------------
    // extract_tar_xz
    // -----------------------------------------------------------------------

    #[test]
    fn test_extract_tar_xz_with_real_archive() {
        // Build a real .tar.xz with a "sema" file inside
        let dir = std::env::temp_dir().join("sema_test_extract_tar_xz");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let content = b"\x7FELF_fake_binary_content_for_testing";

        // Build tar
        let mut tar_buf = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_buf);
            let mut header = tar::Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_entry_type(tar::EntryType::Regular);
            header.set_mode(0o755);
            header.set_cksum();
            builder
                .append_data(&mut header, "sema-lang-test/sema", &content[..])
                .unwrap();
            builder.finish().unwrap();
        }

        // Compress to xz
        let mut xz_buf = Vec::new();
        {
            let mut encoder = xz2::write::XzEncoder::new(&mut xz_buf, 1);
            encoder.write_all(&tar_buf).unwrap();
            encoder.finish().unwrap();
        }

        let out_path = dir.join("extracted_sema");
        extract_tar_xz(&xz_buf, &out_path, "x86_64-unknown-linux-gnu").unwrap();

        let extracted = std::fs::read(&out_path).unwrap();
        assert_eq!(extracted, content);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_extract_tar_xz_skips_symlinks() {
        let dir = std::env::temp_dir().join("sema_test_extract_symlink");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // Build tar with a symlink named "sema" and a regular file named "other"
        let mut tar_buf = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_buf);

            // Symlink entry named "sema" → should be skipped
            let mut sym_header = tar::Header::new_gnu();
            sym_header.set_entry_type(tar::EntryType::Symlink);
            sym_header.set_size(0);
            sym_header.set_cksum();
            builder
                .append_link(&mut sym_header, "sema", "/etc/passwd")
                .unwrap();

            builder.finish().unwrap();
        }

        let mut xz_buf = Vec::new();
        {
            let mut encoder = xz2::write::XzEncoder::new(&mut xz_buf, 1);
            encoder.write_all(&tar_buf).unwrap();
            encoder.finish().unwrap();
        }

        let out_path = dir.join("extracted_sema");
        let result = extract_tar_xz(&xz_buf, &out_path, "x86_64-unknown-linux-gnu");

        // Should fail because no regular file named "sema" was found
        assert!(result.is_err());
        assert!(!out_path.exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_extract_tar_xz_missing_binary() {
        let dir = std::env::temp_dir().join("sema_test_extract_missing");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // Build tar with a file that's NOT named "sema"
        let content = b"not the right file";
        let mut tar_buf = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_buf);
            let mut header = tar::Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_entry_type(tar::EntryType::Regular);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, "wrong-name", &content[..])
                .unwrap();
            builder.finish().unwrap();
        }

        let mut xz_buf = Vec::new();
        {
            let mut encoder = xz2::write::XzEncoder::new(&mut xz_buf, 1);
            encoder.write_all(&tar_buf).unwrap();
            encoder.finish().unwrap();
        }

        let out_path = dir.join("extracted_sema");
        let result = extract_tar_xz(&xz_buf, &out_path, "x86_64-unknown-linux-gnu");
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("not found"),
            "error should say binary not found"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // -----------------------------------------------------------------------
    // extract_zip
    // -----------------------------------------------------------------------

    #[test]
    fn test_extract_zip_with_real_archive() {
        let dir = std::env::temp_dir().join("sema_test_extract_zip");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let content = b"MZ_fake_pe_binary_content_for_testing";

        // Build zip
        let mut zip_buf = Vec::new();
        {
            let cursor = std::io::Cursor::new(&mut zip_buf);
            let mut writer = zip::ZipWriter::new(cursor);
            let options = zip::write::SimpleFileOptions::default();
            writer
                .start_file("sema-lang-test/sema.exe", options)
                .unwrap();
            writer.write_all(content).unwrap();
            writer.finish().unwrap();
        }

        let out_path = dir.join("extracted_sema.exe");
        extract_zip(&zip_buf, &out_path, "x86_64-pc-windows-msvc").unwrap();

        let extracted = std::fs::read(&out_path).unwrap();
        assert_eq!(extracted, content);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_extract_zip_missing_binary() {
        let dir = std::env::temp_dir().join("sema_test_extract_zip_missing");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let mut zip_buf = Vec::new();
        {
            let cursor = std::io::Cursor::new(&mut zip_buf);
            let mut writer = zip::ZipWriter::new(cursor);
            let options = zip::write::SimpleFileOptions::default();
            writer.start_file("wrong-name.txt", options).unwrap();
            writer.write_all(b"not the binary").unwrap();
            writer.finish().unwrap();
        }

        let out_path = dir.join("extracted_sema.exe");
        let result = extract_zip(&zip_buf, &out_path, "x86_64-pc-windows-msvc");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_extract_zip_skips_directories() {
        let dir = std::env::temp_dir().join("sema_test_extract_zip_dirs");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let content = b"MZ_actual_binary";

        let mut zip_buf = Vec::new();
        {
            let cursor = std::io::Cursor::new(&mut zip_buf);
            let mut writer = zip::ZipWriter::new(cursor);
            let options = zip::write::SimpleFileOptions::default();
            // Add a directory entry
            writer.add_directory("sema-lang-test/", options).unwrap();
            // Add the actual binary
            writer
                .start_file("sema-lang-test/sema.exe", options)
                .unwrap();
            writer.write_all(content).unwrap();
            writer.finish().unwrap();
        }

        let out_path = dir.join("extracted_sema.exe");
        extract_zip(&zip_buf, &out_path, "x86_64-pc-windows-msvc").unwrap();

        let extracted = std::fs::read(&out_path).unwrap();
        assert_eq!(extracted, content);

        let _ = std::fs::remove_dir_all(&dir);
    }

    // -----------------------------------------------------------------------
    // detect_binary_format / expected_format (shared helpers)
    // -----------------------------------------------------------------------

    #[test]
    fn test_detect_binary_format_elf() {
        assert_eq!(
            detect_binary_format(&[0x7F, b'E', b'L', b'F']),
            Some(BinaryFormat::Elf)
        );
    }

    #[test]
    fn test_detect_binary_format_macho_le() {
        assert_eq!(
            detect_binary_format(&[0xCF, 0xFA, 0xED, 0xFE]),
            Some(BinaryFormat::MachO)
        );
    }

    #[test]
    fn test_detect_binary_format_macho_be() {
        assert_eq!(
            detect_binary_format(&[0xFE, 0xED, 0xFA, 0xCF]),
            Some(BinaryFormat::MachO)
        );
    }

    #[test]
    fn test_detect_binary_format_macho_fat() {
        assert_eq!(
            detect_binary_format(&[0xCA, 0xFE, 0xBA, 0xBE]),
            Some(BinaryFormat::MachO)
        );
    }

    #[test]
    fn test_detect_binary_format_pe() {
        assert_eq!(
            detect_binary_format(&[b'M', b'Z', 0x00, 0x00]),
            Some(BinaryFormat::Pe)
        );
    }

    #[test]
    fn test_detect_binary_format_unknown() {
        assert_eq!(detect_binary_format(&[0x00, 0x00, 0x00, 0x00]), None);
    }

    #[test]
    fn test_detect_binary_format_too_short() {
        assert_eq!(detect_binary_format(&[0x7F, b'E']), None);
        assert_eq!(detect_binary_format(&[]), None);
    }

    #[test]
    fn test_expected_format() {
        assert_eq!(
            expected_format("x86_64-unknown-linux-gnu"),
            BinaryFormat::Elf
        );
        assert_eq!(
            expected_format("aarch64-unknown-linux-gnu"),
            BinaryFormat::Elf
        );
        assert_eq!(expected_format("aarch64-apple-darwin"), BinaryFormat::MachO);
        assert_eq!(expected_format("x86_64-apple-darwin"), BinaryFormat::MachO);
        assert_eq!(expected_format("x86_64-pc-windows-msvc"), BinaryFormat::Pe);
    }
}
