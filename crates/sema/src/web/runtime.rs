//! The browser runtime `sema web` serves: the WASM VM glue + JS bundle a
//! sema-web app needs to boot with no bundler. The generated assets under
//! `assets/` are tracked package inputs, embedded into every Sema binary at
//! compile time. `debug-embed` forces embedding in every build profile, so a
//! packaged/installed binary never reads them from a (nonexistent) source tree
//! at runtime — the whole point is that `sema web` works offline after install.

use std::borrow::Cow;
use std::fmt::Write;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use rust_embed::RustEmbed;
use sha2::{Digest, Sha256};

static NEXT_STAGING_DIR: AtomicU64 = AtomicU64::new(0);

/// Every file under `src/web/assets/`, embedded at compile time. `iter()` walks
/// the tree (including `sema/backends/*`), so adding, removing, or renaming a
/// runtime file needs no change here: the served path is the file's path
/// relative to `assets/`. The `.wasm` glue fetches `sema_wasm_bg.wasm` relative
/// to its own URL, so the two must stay co-located under the same prefix.
#[derive(RustEmbed)]
#[folder = "src/web/assets/"]
struct WebRuntime;

struct RuntimeAsset {
    relative_path: String,
    data: Cow<'static, [u8]>,
}

fn embedded_assets() -> Vec<RuntimeAsset> {
    let mut assets: Vec<_> = WebRuntime::iter()
        .map(|relative_path| {
            let file = WebRuntime::get(relative_path.as_ref())
                .expect("embedded asset from iter() must resolve");
            RuntimeAsset {
                relative_path: relative_path.into_owned(),
                data: file.data,
            }
        })
        .collect();
    assets.sort_unstable_by(|left, right| left.relative_path.cmp(&right.relative_path));
    assets
}

fn asset_set_digest(assets: &[RuntimeAsset]) -> String {
    let mut ordered: Vec<_> = assets.iter().collect();
    ordered.sort_unstable_by(|left, right| left.relative_path.cmp(&right.relative_path));

    let mut hasher = Sha256::new();
    for asset in ordered {
        let path = asset.relative_path.as_bytes();
        hasher.update((path.len() as u64).to_le_bytes());
        hasher.update(path);
        hasher.update((asset.data.len() as u64).to_le_bytes());
        hasher.update(&asset.data);
    }

    let mut encoded = String::with_capacity(64);
    for byte in hasher.finalize() {
        write!(&mut encoded, "{byte:02x}").expect("writing to a string cannot fail");
    }
    encoded
}

fn assets_match(dir: &Path, assets: &[RuntimeAsset]) -> std::io::Result<bool> {
    for asset in assets {
        match fs::read(dir.join(&asset.relative_path)) {
            Ok(contents) if contents.as_slice() == asset.data.as_ref() => {}
            Ok(_) => return Ok(false),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error),
        }
    }
    Ok(true)
}

struct StagingDir(PathBuf);

impl StagingDir {
    fn create(cache_root: &Path) -> std::io::Result<Self> {
        loop {
            let sequence = NEXT_STAGING_DIR.fetch_add(1, Ordering::Relaxed);
            let path = cache_root.join(format!(
                ".sema-web-runtime-staging-{}-{sequence}",
                std::process::id()
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
        }
    }
}

impl Drop for StagingDir {
    fn drop(&mut self) {
        match fs::remove_dir_all(&self.0) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => {}
        }
    }
}

fn write_generation(staging: &Path, assets: &[RuntimeAsset]) -> std::io::Result<()> {
    for asset in assets {
        let path = staging.join(&asset.relative_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, &asset.data)?;
    }
    if !assets_match(staging, assets)? {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "staged web runtime failed content validation",
        ));
    }
    Ok(())
}

fn extract_assets(
    cache_root: &Path,
    version: &str,
    assets: &[RuntimeAsset],
) -> std::io::Result<PathBuf> {
    fs::create_dir_all(cache_root)?;
    let digest = asset_set_digest(assets);
    let generation_stem = format!("sema-web-runtime-{version}-{digest}");
    let mut repair = 0_u64;

    loop {
        let generation = if repair == 0 {
            cache_root.join(&generation_stem)
        } else {
            cache_root.join(format!("{generation_stem}-{repair}"))
        };

        match fs::metadata(&generation) {
            Ok(metadata) => {
                if metadata.is_dir() && assets_match(&generation, assets)? {
                    return Ok(generation);
                }
                repair = repair.checked_add(1).ok_or_else(|| {
                    std::io::Error::other("web runtime cache repair sequence exhausted")
                })?;
                continue;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }

        let staging = StagingDir::create(cache_root)?;
        write_generation(&staging.0, assets)?;
        match fs::rename(&staging.0, &generation) {
            Ok(()) => return Ok(generation),
            Err(rename_error) => match fs::metadata(&generation) {
                Ok(metadata) => {
                    if metadata.is_dir() && assets_match(&generation, assets)? {
                        return Ok(generation);
                    }
                    repair = repair.checked_add(1).ok_or_else(|| {
                        std::io::Error::other("web runtime cache repair sequence exhausted")
                    })?;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return Err(rename_error);
                }
                Err(error) => return Err(error),
            },
        }
    }
}

/// Extract the embedded runtime to an immutable, content-addressed temp dir and
/// return its path, so the dev server can serve it as static files. A complete
/// generation is published with one directory rename; repeat launches validate
/// and reuse the existing generation without rewriting the ~4.5 MB wasm file.
pub fn extract() -> std::io::Result<std::path::PathBuf> {
    extract_assets(
        &std::env::temp_dir(),
        env!("CARGO_PKG_VERSION"),
        &embedded_assets(),
    )
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::{extract_assets, RuntimeAsset, WebRuntime};

    static NEXT_TEST_DIR: AtomicU64 = AtomicU64::new(0);

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            loop {
                let sequence = NEXT_TEST_DIR.fetch_add(1, Ordering::Relaxed);
                let path = std::env::temp_dir().join(format!(
                    "sema-web-runtime-test-{}-{sequence}",
                    std::process::id()
                ));
                match std::fs::create_dir(&path) {
                    Ok(()) => return Self(path),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(error) => panic!("create isolated runtime cache: {error}"),
                }
            }
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).expect("remove isolated runtime cache");
        }
    }

    fn asset(relative_path: &str, data: &'static [u8]) -> RuntimeAsset {
        RuntimeAsset {
            relative_path: relative_path.to_owned(),
            data: Cow::Borrowed(data),
        }
    }

    /// Directory embedding silently yields *nothing* if the asset dir is missing
    /// or empty — which would reproduce the shipped-broken-`sema web` bug this
    /// module exists to prevent. Assert every file the shell's import map depends
    /// on (see `shell.html`) is embedded, so a vanished or renamed runtime file
    /// fails here (and in CI) instead of 404-ing in a user's browser. The
    /// packaged smoke test (`scripts/test-packaged-sema-web.sh`) additionally
    /// serves the *entire* embedded set over HTTP, covering transitive imports.
    #[test]
    fn embeds_the_critical_runtime_files() {
        for required in [
            "sema_wasm_bg.wasm",      // fetched by the wasm glue
            "sema_wasm.js",           // @sema-lang/sema-wasm
            "sema-web.js",            // @sema-lang/sema-web
            "sema/index.js",          // @sema-lang/sema
            "signals-core.module.js", // @preact/signals-core
            "morphdom-esm.js",        // morphdom
        ] {
            assert!(
                WebRuntime::get(required).is_some(),
                "{required} missing from embedded web runtime — packaged `sema web` would be broken",
            );
        }
    }

    #[test]
    fn embedded_wasm_glue_has_no_retired_blocking_compatibility_surface() {
        let glue = WebRuntime::get("sema_wasm.js").expect("WASM glue is embedded");
        let glue = std::str::from_utf8(&glue.data).expect("wasm-bindgen glue is UTF-8");
        for retired in [
            "debugPerformFetch",
            "installAtomicsSleep",
            "XMLHttpRequest",
            "Atomics.wait",
            "HTTP_AWAIT_MARKER",
        ] {
            assert!(
                !glue.contains(retired),
                "retired WASM compatibility marker {retired:?} remains in the embedded runtime",
            );
        }
    }

    #[test]
    fn same_version_same_size_asset_change_publishes_a_coherent_generation() {
        let cache = TestDir::new();
        let original = [
            asset("sema_wasm.js", b"old-js"),
            asset("sema_wasm_bg.wasm", b"old-wa"),
        ];
        let changed = [
            asset("sema_wasm.js", b"new-js"),
            asset("sema_wasm_bg.wasm", b"new-wa"),
        ];

        let original_dir =
            extract_assets(cache.path(), "9.9.9", &original).expect("extract original assets");
        let changed_dir =
            extract_assets(cache.path(), "9.9.9", &changed).expect("extract changed assets");

        assert_ne!(
            original_dir, changed_dir,
            "a content change needs a distinct immutable generation"
        );
        assert_eq!(
            std::fs::read(original_dir.join("sema_wasm.js")).unwrap(),
            b"old-js"
        );
        assert_eq!(
            std::fs::read(original_dir.join("sema_wasm_bg.wasm")).unwrap(),
            b"old-wa"
        );
        assert_eq!(
            std::fs::read(changed_dir.join("sema_wasm.js")).unwrap(),
            b"new-js"
        );
        assert_eq!(
            std::fs::read(changed_dir.join("sema_wasm_bg.wasm")).unwrap(),
            b"new-wa"
        );

        std::fs::write(changed_dir.join("sema_wasm.js"), b"bad-js").unwrap();
        let repaired_dir =
            extract_assets(cache.path(), "9.9.9", &changed).expect("repair corrupt generation");
        assert_ne!(changed_dir, repaired_dir);
        assert_eq!(
            std::fs::read(repaired_dir.join("sema_wasm.js")).unwrap(),
            b"new-js"
        );
        assert_eq!(
            std::fs::read(repaired_dir.join("sema_wasm_bg.wasm")).unwrap(),
            b"new-wa"
        );
        assert!(
            std::fs::read_dir(cache.path()).unwrap().all(|entry| {
                !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".sema-web-runtime-staging-")
            }),
            "atomic publication leaves no staging directories"
        );
    }
}
