//! `sema update` — download and install the latest released sema binary.

use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::Path;

use crate::cross_compile::{
    extract_tar_xz, extract_zip, host_target, http_client, is_windows_target,
    parse_sha256_checksum, GITHUB_REPO,
};

/// Options for `sema update`.
pub struct UpdateOptions {
    pub check_only: bool,
    pub target_version: Option<String>,
    pub yes: bool,
}

/// Entry point for `sema update`.
pub fn run(opts: UpdateOptions) -> Result<(), Box<dyn std::error::Error>> {
    let current_version = env!("CARGO_PKG_VERSION");
    let target = host_target();
    let client = http_client()?;

    let latest_tag = fetch_release_tag(&client, opts.target_version.as_deref())?;
    let latest_version = strip_v_prefix(&latest_tag).to_string();

    if opts.target_version.is_none() && !is_newer_version(current_version, &latest_version)? {
        println!("sema is already up to date (v{current_version})");
        return Ok(());
    }

    println!("sema v{current_version} -> v{latest_version}");
    if opts.check_only {
        return Ok(());
    }

    let current_exe = std::env::current_exe()?;
    let current_exe = current_exe.canonicalize().unwrap_or(current_exe);

    if let Some(pm) = detect_package_manager(&current_exe) {
        eprintln!(
            "Note: sema looks like it was installed via {pm}. `sema update` will still try \
             to replace it in place, but you may prefer `{}`.",
            package_manager_update_hint(pm)
        );
    }

    let install_dir = current_exe
        .parent()
        .ok_or("cannot determine sema's install directory")?;
    check_writable(install_dir)?;

    if !opts.yes {
        print!("Install sema v{latest_version}? [y/N] ");
        std::io::stdout().flush()?;
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if !matches!(input.trim().to_lowercase().as_str(), "y" | "yes") {
            println!("Aborted.");
            return Ok(());
        }
    }

    let pid = std::process::id();
    let tmp_ext = if is_windows_target(target) {
        ".exe"
    } else {
        ""
    };
    let new_exe_tmp = install_dir.join(format!(".sema-update-{pid}{tmp_ext}"));

    if let Err(e) = download_and_verify(&client, &latest_version, target, &new_exe_tmp) {
        let _ = std::fs::remove_file(&new_exe_tmp);
        return Err(e);
    }

    if let Err(e) = sanity_check(&new_exe_tmp, &latest_version) {
        let _ = std::fs::remove_file(&new_exe_tmp);
        return Err(e.into());
    }

    self_replace::self_replace(&new_exe_tmp)?;
    let _ = std::fs::remove_file(&new_exe_tmp);

    println!("Updated sema to v{latest_version}");
    Ok(())
}

/// Query the GitHub Releases API for the release tag to install: `target_version`
/// if given, otherwise the latest release.
fn fetch_release_tag(
    client: &reqwest::blocking::Client,
    target_version: Option<&str>,
) -> Result<String, Box<dyn std::error::Error>> {
    let url = match target_version {
        Some(v) => format!("https://api.github.com/repos/{GITHUB_REPO}/releases/tags/v{v}"),
        None => format!("https://api.github.com/repos/{GITHUB_REPO}/releases/latest"),
    };
    let response = client.get(&url).send()?;
    if !response.status().is_success() {
        let status = response.status();
        if status.as_u16() == 403 || status.as_u16() == 429 {
            return Err("GitHub rate-limited the update check. Try again later.".into());
        }
        return Err(format!("failed to query GitHub releases: HTTP {status}").into());
    }
    let body = response.text()?;
    parse_release_tag(&body).map_err(Into::into)
}

/// Download the release archive for `target`, verify its SHA256 checksum against
/// the published `.sha256` sidecar, and extract the `sema` binary to `output_path`.
/// Shows a byte-progress bar while downloading.
fn download_and_verify(
    client: &reqwest::blocking::Client,
    version: &str,
    target: &str,
    output_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let ext = if is_windows_target(target) {
        "zip"
    } else {
        "tar.xz"
    };
    let base_url = std::env::var("SEMA_UPDATE_BASE_URL").unwrap_or_else(|_| {
        format!("https://github.com/{GITHUB_REPO}/releases/download/v{version}")
    });
    let archive_url = format!("{base_url}/sema-lang-{target}.{ext}");
    let checksum_url = format!("{archive_url}.sha256");

    let checksum_response = client
        .get(&checksum_url)
        .send()
        .map_err(|e| format!("failed to download checksum for {target}: {e}"))?;
    if !checksum_response.status().is_success() {
        return Err(format!(
            "failed to download checksum for {target}: HTTP {}",
            checksum_response.status()
        )
        .into());
    }
    let checksum_text = checksum_response.text()?;
    let expected_hash = parse_sha256_checksum(&checksum_text)
        .ok_or_else(|| format!("invalid checksum file for {target}: no valid SHA256 hash found"))?;

    let response = client
        .get(&archive_url)
        .send()
        .map_err(|e| format!("failed to download sema v{version} for {target}: {e}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "failed to download sema v{version} for {target}: HTTP {}",
            response.status()
        )
        .into());
    }

    let total_size = response.content_length().unwrap_or(0);
    let progress = indicatif::ProgressBar::new(total_size);
    progress.set_style(
        indicatif::ProgressStyle::with_template(
            "  Downloading sema v{msg} [{bar:30}] {bytes}/{total_bytes} ({eta})",
        )
        .unwrap_or_else(|_| indicatif::ProgressStyle::default_bar())
        .progress_chars("=>-"),
    );
    progress.set_message(version.to_string());

    let mut hasher = Sha256::new();
    let mut archive_bytes = Vec::new();
    let mut response = response;
    let mut buf = [0u8; 65536];
    loop {
        let n = response.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        archive_bytes.extend_from_slice(&buf[..n]);
        progress.inc(n as u64);
    }
    progress.finish_and_clear();

    let actual_hash = format!("{:x}", hasher.finalize());
    if actual_hash != expected_hash {
        return Err(format!(
            "SHA256 mismatch for {target} archive.\n  Expected: {expected_hash}\n  Got:      {actual_hash}"
        )
        .into());
    }
    println!("  Checksum verified ✓");

    if is_windows_target(target) {
        extract_zip(&archive_bytes, output_path, target)?;
    } else {
        extract_tar_xz(&archive_bytes, output_path, target)?;
    }

    Ok(())
}

/// Confirm the freshly downloaded binary actually runs and reports the expected
/// version before it's used to replace the live binary.
fn sanity_check(exe_path: &Path, expected_version: &str) -> Result<(), String> {
    let output = std::process::Command::new(exe_path)
        .arg("--version")
        .output()
        .map_err(|e| format!("downloaded binary failed to run: {e}"))?;
    if !output.status.success() {
        return Err("downloaded binary exited with an error when checking --version".to_string());
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    if !stdout.contains(expected_version) {
        return Err(format!(
            "downloaded binary reports an unexpected version (expected {expected_version}, got: {})",
            stdout.trim()
        ));
    }
    Ok(())
}

/// Verify the install directory is writable before downloading anything, so a
/// permissions problem fails fast instead of after a wasted download.
fn check_writable(install_dir: &Path) -> Result<(), String> {
    let probe = install_dir.join(format!(".sema-update-write-test-{}", std::process::id()));
    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            Ok(())
        }
        Err(e) => Err(format!(
            "no write access to '{}': {e}\n  Try re-running with elevated privileges, or reinstall \
             to a user-writable directory (e.g. ~/.local/bin).",
            install_dir.display()
        )),
    }
}

/// Suggested command to update via the detected package manager instead.
fn package_manager_update_hint(pm: &str) -> &'static str {
    match pm {
        "Homebrew" | "Linuxbrew" => "brew upgrade sema-lang",
        "Scoop" => "scoop update sema",
        _ => "your package manager's upgrade command",
    }
}

/// Strip a leading `v` from a git tag, e.g. `"v1.31.0"` -> `"1.31.0"`.
fn strip_v_prefix(tag: &str) -> &str {
    tag.strip_prefix('v').unwrap_or(tag)
}

/// Extract the release tag name from a GitHub Releases API JSON response
/// (e.g. the body of `GET /repos/{owner}/{repo}/releases/latest`).
fn parse_release_tag(json: &str) -> Result<String, String> {
    let value: serde_json::Value = serde_json::from_str(json)
        .map_err(|e| format!("failed to parse GitHub release response: {e}"))?;
    value
        .get("tag_name")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| "GitHub release response missing 'tag_name'".to_string())
}

/// Returns `true` if `latest` is a newer semantic version than `current`.
/// Both arguments must be bare `MAJOR.MINOR.PATCH` versions (no leading `v`).
fn is_newer_version(current: &str, latest: &str) -> Result<bool, String> {
    let current = semver::Version::parse(current)
        .map_err(|e| format!("invalid current version '{current}': {e}"))?;
    let latest = semver::Version::parse(latest)
        .map_err(|e| format!("invalid latest version '{latest}': {e}"))?;
    Ok(latest > current)
}

/// Heuristically detect whether `path` looks like it's managed by a package
/// manager (Homebrew, Linuxbrew, Scoop) rather than sema's own installer.
///
/// Advisory only: used to warn the user before self-updating, not to hard-block —
/// path heuristics can misfire on mixed installs or custom prefixes.
fn detect_package_manager(path: &Path) -> Option<&'static str> {
    let s = path.to_string_lossy();
    if s.contains("/Cellar/") {
        Some("Homebrew")
    } else if s.contains("linuxbrew") {
        Some("Linuxbrew")
    } else if s.contains("\\scoop\\") || s.contains("/scoop/") {
        Some("Scoop")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    // -----------------------------------------------------------------------
    // strip_v_prefix
    // -----------------------------------------------------------------------

    #[test]
    fn strip_v_prefix_strips_leading_v() {
        assert_eq!(strip_v_prefix("v1.31.0"), "1.31.0");
    }

    #[test]
    fn strip_v_prefix_leaves_bare_version() {
        assert_eq!(strip_v_prefix("1.31.0"), "1.31.0");
    }

    // -----------------------------------------------------------------------
    // parse_release_tag
    // -----------------------------------------------------------------------

    #[test]
    fn parse_release_tag_extracts_tag_name() {
        let json = r#"{"tag_name": "v1.31.0", "name": "v1.31.0"}"#;
        assert_eq!(parse_release_tag(json).unwrap(), "v1.31.0");
    }

    #[test]
    fn parse_release_tag_errors_on_missing_field() {
        let json = r#"{"name": "no tag here"}"#;
        assert!(parse_release_tag(json).is_err());
    }

    #[test]
    fn parse_release_tag_errors_on_invalid_json() {
        assert!(parse_release_tag("not json").is_err());
    }

    // -----------------------------------------------------------------------
    // is_newer_version
    // -----------------------------------------------------------------------

    #[test]
    fn is_newer_version_true_when_latest_is_greater() {
        assert!(is_newer_version("1.30.0", "1.31.0").unwrap());
    }

    #[test]
    fn is_newer_version_false_when_equal() {
        assert!(!is_newer_version("1.30.0", "1.30.0").unwrap());
    }

    #[test]
    fn is_newer_version_false_when_latest_is_older() {
        assert!(!is_newer_version("1.31.0", "1.30.0").unwrap());
    }

    #[test]
    fn is_newer_version_errors_on_invalid_current() {
        assert!(is_newer_version("not-a-version", "1.30.0").is_err());
    }

    #[test]
    fn is_newer_version_errors_on_invalid_latest() {
        assert!(is_newer_version("1.30.0", "not-a-version").is_err());
    }

    // -----------------------------------------------------------------------
    // detect_package_manager
    // -----------------------------------------------------------------------

    #[test]
    fn detect_package_manager_finds_homebrew_cellar() {
        let path = Path::new("/opt/homebrew/Cellar/sema-lang/1.30.0/bin/sema");
        assert_eq!(detect_package_manager(path), Some("Homebrew"));
    }

    #[test]
    fn detect_package_manager_finds_intel_homebrew_prefix() {
        let path = Path::new("/usr/local/Cellar/sema-lang/1.30.0/bin/sema");
        assert_eq!(detect_package_manager(path), Some("Homebrew"));
    }

    #[test]
    fn detect_package_manager_finds_linuxbrew() {
        let path = Path::new("/home/linuxbrew/.linuxbrew/bin/sema");
        assert_eq!(detect_package_manager(path), Some("Linuxbrew"));
    }

    #[test]
    fn detect_package_manager_finds_scoop() {
        let path = Path::new(r"C:\Users\me\scoop\shims\sema.exe");
        assert_eq!(detect_package_manager(path), Some("Scoop"));
    }

    #[test]
    fn detect_package_manager_none_for_script_install() {
        let path = Path::new("/home/user/.sema/bin/sema");
        assert_eq!(detect_package_manager(path), None);
    }

    #[test]
    fn detect_package_manager_none_for_cargo_install() {
        let path = Path::new("/home/user/.cargo/bin/sema");
        assert_eq!(detect_package_manager(path), None);
    }

    // -----------------------------------------------------------------------
    // Network tests — require internet access, run with `cargo test -- --ignored`
    // (mirrors the `jake test.http` convention: excluded from the default suite).
    // -----------------------------------------------------------------------

    #[test]
    #[ignore] // requires network
    fn fetch_release_tag_returns_the_real_latest_tag() {
        let client = http_client().unwrap();
        let tag = fetch_release_tag(&client, None).unwrap();
        assert!(tag.starts_with('v'), "tag '{tag}' should start with 'v'");
        semver::Version::parse(strip_v_prefix(&tag))
            .unwrap_or_else(|e| panic!("tag '{tag}' is not valid semver: {e}"));
    }

    #[test]
    #[ignore] // requires network
    fn fetch_release_tag_resolves_a_specific_version() {
        let client = http_client().unwrap();
        let tag = fetch_release_tag(&client, Some("1.30.0")).unwrap();
        assert_eq!(tag, "v1.30.0");
    }

    #[test]
    #[ignore] // requires network, downloads a real release archive
    fn download_and_verify_downloads_a_real_release_for_host_target() {
        let client = http_client().unwrap();
        let target = host_target();
        let dir = std::env::temp_dir().join("sema_test_update_download_and_verify");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let output_path = dir.join("sema-downloaded");

        download_and_verify(&client, "1.30.0", target, &output_path).unwrap();

        let meta = std::fs::metadata(&output_path).unwrap();
        assert!(meta.len() > 0, "downloaded binary should be non-empty");
        assert!(sanity_check(&output_path, "1.30.0").is_ok());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
