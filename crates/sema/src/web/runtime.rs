//! The browser runtime `sema web` serves: the WASM VM glue + JS bundle a
//! sema-web app needs to boot with no bundler. The generated assets under
//! `assets/` are tracked package inputs, embedded into every Sema binary at
//! compile time. `debug-embed` forces embedding in every build profile, so a
//! packaged/installed binary never reads them from a (nonexistent) source tree
//! at runtime — the whole point is that `sema web` works offline after install.

use std::borrow::Cow;
use std::fmt::Write;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use directories::ProjectDirs;
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

fn asset_path(root: &Path, relative_path: &str) -> std::io::Result<PathBuf> {
    let mut path = root.to_owned();
    let mut has_component = false;
    for component in Path::new(relative_path).components() {
        let Component::Normal(component) = component else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("invalid embedded web runtime path: {relative_path}"),
            ));
        };
        has_component = true;
        path.push(component);
    }
    if !has_component {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "embedded web runtime path is empty",
        ));
    }
    Ok(path)
}

fn real_asset_path(root: &Path, relative_path: &str) -> std::io::Result<Option<PathBuf>> {
    let mut path = root.to_owned();
    let mut components = Path::new(relative_path).components().peekable();
    let mut has_component = false;
    while let Some(component) = components.next() {
        let Component::Normal(component) = component else {
            return Ok(None);
        };
        has_component = true;
        path.push(component);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let file_type = metadata.file_type();
        let expected_type = if components.peek().is_some() {
            file_type.is_dir()
        } else {
            file_type.is_file()
        };
        if !expected_type {
            return Ok(None);
        }
    }
    Ok(has_component.then_some(path))
}

fn assets_match(dir: &Path, assets: &[RuntimeAsset]) -> bool {
    let real_dir = fs::symlink_metadata(dir).is_ok_and(|metadata| metadata.file_type().is_dir());
    real_dir
        && assets.iter().all(|asset| {
            real_asset_path(dir, &asset.relative_path)
                .and_then(|path| {
                    path.map_or(Ok(false), |path| {
                        fs::read(path).map(|contents| contents.as_slice() == asset.data.as_ref())
                    })
                })
                .unwrap_or(false)
        })
}

fn validate_staged_assets(dir: &Path, assets: &[RuntimeAsset]) -> std::io::Result<()> {
    for asset in assets {
        let path = real_asset_path(dir, &asset.relative_path)?.ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "staged web runtime asset is not a regular file: {}",
                    asset.relative_path
                ),
            )
        })?;
        if fs::read(path)?.as_slice() != asset.data.as_ref() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "staged web runtime asset failed content validation: {}",
                    asset.relative_path
                ),
            ));
        }
    }
    Ok(())
}

fn cache_path_error(action: &str, path: &Path, error: std::io::Error) -> std::io::Error {
    std::io::Error::new(
        error.kind(),
        format!(
            "cannot {action} web runtime cache path {}: {error}",
            path.display()
        ),
    )
}

fn prepare_cache_root(cache_root: &Path) -> std::io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(cache_root)
        .map_err(|error| cache_path_error("create", cache_root, error))?;

    let metadata = fs::symlink_metadata(cache_root)
        .map_err(|error| cache_path_error("inspect", cache_root, error))?;
    if !metadata.file_type().is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "web runtime cache root must be a real directory: {}",
                cache_root.display()
            ),
        ));
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        fs::set_permissions(cache_root, fs::Permissions::from_mode(0o700))
            .map_err(|error| cache_path_error("secure", cache_root, error))?;
        let mode = fs::symlink_metadata(cache_root)
            .map_err(|error| cache_path_error("verify", cache_root, error))?
            .mode()
            & 0o777;
        if mode != 0o700 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!(
                    "web runtime cache root {} has mode {mode:o}, expected 700",
                    cache_root.display()
                ),
            ));
        }
    }
    fs::read_dir(cache_root).map_err(|error| cache_path_error("search", cache_root, error))?;
    Ok(())
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
        let path = asset_path(staging, &asset.relative_path)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, &asset.data)?;
    }
    validate_staged_assets(staging, assets)
}

fn extract_assets(
    cache_root: &Path,
    version: &str,
    assets: &[RuntimeAsset],
) -> std::io::Result<PathBuf> {
    prepare_cache_root(cache_root)?;
    let digest = asset_set_digest(assets);
    let generation_stem = format!("sema-web-runtime-{version}-{digest}");
    let mut repair = 0_u64;

    loop {
        let generation = if repair == 0 {
            cache_root.join(&generation_stem)
        } else {
            cache_root.join(format!("{generation_stem}-{repair}"))
        };

        match fs::symlink_metadata(&generation) {
            Ok(metadata) => {
                if metadata.file_type().is_dir() && assets_match(&generation, assets) {
                    return Ok(generation);
                }
                repair = repair.checked_add(1).ok_or_else(|| {
                    std::io::Error::other("web runtime cache repair sequence exhausted")
                })?;
                continue;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(cache_path_error("inspect candidate", &generation, error));
            }
        }

        let staging = StagingDir::create(cache_root)?;
        write_generation(&staging.0, assets)?;
        match fs::rename(&staging.0, &generation) {
            Ok(()) => return Ok(generation),
            Err(rename_error) => match fs::symlink_metadata(&generation) {
                Ok(metadata) => {
                    if metadata.file_type().is_dir() && assets_match(&generation, assets) {
                        return Ok(generation);
                    }
                    repair = repair.checked_add(1).ok_or_else(|| {
                        std::io::Error::other("web runtime cache repair sequence exhausted")
                    })?;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return Err(rename_error);
                }
                Err(error) => {
                    return Err(cache_path_error("inspect candidate", &generation, error));
                }
            },
        }
    }
}

/// Extract the embedded runtime to an immutable, content-addressed per-user
/// cache directory and return its path, so the dev server can serve it as
/// static files. A complete generation is published with one directory rename;
/// repeat launches validate and reuse the existing generation without rewriting
/// the ~4.5 MB wasm file.
pub fn extract() -> std::io::Result<std::path::PathBuf> {
    let cache_root = ProjectDirs::from("com", "sema-lang", "sema")
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "could not determine the per-user cache directory",
            )
        })?
        .cache_dir()
        .join("web-runtime");
    extract_assets(&cache_root, env!("CARGO_PKG_VERSION"), &embedded_assets())
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Barrier};

    use super::{asset_set_digest, extract_assets, prepare_cache_root, RuntimeAsset, WebRuntime};

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

    fn generation_path(cache_root: &Path, version: &str, assets: &[RuntimeAsset]) -> PathBuf {
        cache_root.join(format!(
            "sema-web-runtime-{version}-{}",
            asset_set_digest(assets)
        ))
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

    #[cfg(unix)]
    #[test]
    fn symlinked_asset_with_correct_bytes_is_rejected() {
        use std::os::unix::fs::symlink;

        let cache = TestDir::new();
        let assets = [asset("sema_wasm.js", b"expected")];
        let corrupt_generation = generation_path(cache.path(), "9.9.9", &assets);
        std::fs::create_dir(&corrupt_generation).unwrap();
        let external = cache.path().join("external.js");
        std::fs::write(&external, b"expected").unwrap();
        symlink(&external, corrupt_generation.join("sema_wasm.js")).unwrap();

        let repaired =
            extract_assets(cache.path(), "9.9.9", &assets).expect("repair symlinked asset");

        assert_ne!(repaired, corrupt_generation);
        assert!(std::fs::symlink_metadata(repaired.join("sema_wasm.js"))
            .unwrap()
            .file_type()
            .is_file());
        assert_eq!(
            std::fs::read(repaired.join("sema_wasm.js")).unwrap(),
            b"expected"
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_generation_with_correct_bytes_is_rejected() {
        use std::os::unix::fs::symlink;

        let cache = TestDir::new();
        let assets = [asset("sema_wasm.js", b"expected")];
        let external_generation = cache.path().join("external-generation");
        std::fs::create_dir(&external_generation).unwrap();
        std::fs::write(external_generation.join("sema_wasm.js"), b"expected").unwrap();
        let corrupt_generation = generation_path(cache.path(), "9.9.9", &assets);
        symlink(&external_generation, &corrupt_generation).unwrap();

        let repaired =
            extract_assets(cache.path(), "9.9.9", &assets).expect("repair symlinked generation");

        assert_ne!(repaired, corrupt_generation);
        assert!(
            std::fs::symlink_metadata(&corrupt_generation)
                .unwrap()
                .file_type()
                .is_symlink(),
            "repair must not mutate a suspicious candidate"
        );
        assert_eq!(
            std::fs::read(repaired.join("sema_wasm.js")).unwrap(),
            b"expected"
        );
    }

    #[test]
    fn asset_directory_is_repaired_as_a_regular_file() {
        let cache = TestDir::new();
        let assets = [asset("sema_wasm.js", b"expected")];
        let corrupt_generation = generation_path(cache.path(), "9.9.9", &assets);
        std::fs::create_dir_all(corrupt_generation.join("sema_wasm.js")).unwrap();

        let repaired =
            extract_assets(cache.path(), "9.9.9", &assets).expect("repair directory asset");

        assert_ne!(repaired, corrupt_generation);
        assert!(std::fs::symlink_metadata(repaired.join("sema_wasm.js"))
            .unwrap()
            .file_type()
            .is_file());
        assert_eq!(
            std::fs::read(repaired.join("sema_wasm.js")).unwrap(),
            b"expected"
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_cache_root_is_rejected() {
        use std::os::unix::fs::symlink;

        let parent = TestDir::new();
        let real_root = parent.path().join("real-cache");
        let linked_root = parent.path().join("linked-cache");
        std::fs::create_dir(&real_root).unwrap();
        symlink(&real_root, &linked_root).unwrap();

        let error = extract_assets(&linked_root, "9.9.9", &[asset("sema_wasm.js", b"expected")])
            .expect_err("a cache root must not be a symlink");

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(std::fs::read_dir(real_root).unwrap().next().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn non_executable_cache_root_is_repaired_before_namespace_scan() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        for initial_mode in [0o000, 0o600] {
            let cache = TestDir::new();
            std::fs::set_permissions(cache.path(), std::fs::Permissions::from_mode(initial_mode))
                .unwrap();

            let prepared = prepare_cache_root(cache.path());
            let actual_mode = std::fs::symlink_metadata(cache.path()).unwrap().mode() & 0o777;
            if actual_mode != 0o700 {
                std::fs::set_permissions(cache.path(), std::fs::Permissions::from_mode(0o700))
                    .unwrap();
            }

            prepared.expect("repair an owner-controlled non-executable cache root");
            assert_eq!(actual_mode, 0o700);
            let published =
                extract_assets(cache.path(), "9.9.9", &[asset("sema_wasm.js", b"expected")])
                    .expect("cache namespace scan must complete after repairing its mode");
            assert_eq!(
                std::fs::read(published.join("sema_wasm.js")).unwrap(),
                b"expected"
            );
        }
    }

    #[test]
    fn concurrent_extractors_publish_one_coherent_generation() {
        const EXTRACTORS: usize = 8;

        let cache = Arc::new(TestDir::new());
        let assets = [asset("sema_wasm.js", b"expected")];
        let corrupt_generation = generation_path(cache.path(), "9.9.9", &assets);
        std::fs::create_dir(&corrupt_generation).unwrap();
        std::fs::write(corrupt_generation.join("sema_wasm.js"), b"stale---").unwrap();
        let barrier = Arc::new(Barrier::new(EXTRACTORS));

        let extractors: Vec<_> = (0..EXTRACTORS)
            .map(|_| {
                let cache = Arc::clone(&cache);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    extract_assets(cache.path(), "9.9.9", &[asset("sema_wasm.js", b"expected")])
                        .expect("concurrent extraction")
                })
            })
            .collect();
        let generations: Vec<_> = extractors
            .into_iter()
            .map(|extractor| extractor.join().expect("extractor thread"))
            .collect();

        assert!(generations.windows(2).all(|pair| pair[0] == pair[1]));
        let published = &generations[0];
        assert_ne!(published, &corrupt_generation);
        assert_eq!(
            std::fs::read(published.join("sema_wasm.js")).unwrap(),
            b"expected"
        );
        assert!(
            std::fs::read_dir(cache.path()).unwrap().all(|entry| {
                !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".sema-web-runtime-staging-")
            }),
            "no extractor may return or leave a staging directory"
        );
    }
}
