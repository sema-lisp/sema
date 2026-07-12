//! The browser runtime `sema web` serves: the WASM VM glue + JS bundle a
//! sema-web app needs to boot with no bundler. The generated assets under
//! `assets/` are tracked package inputs, embedded into every Sema binary at
//! compile time. `debug-embed` forces embedding in every build profile, so a
//! packaged/installed binary never reads them from a (nonexistent) source tree
//! at runtime — the whole point is that `sema web` works offline after install.

use rust_embed::RustEmbed;

/// Every file under `src/web/assets/`, embedded at compile time. `iter()` walks
/// the tree (including `sema/backends/*`), so adding, removing, or renaming a
/// runtime file needs no change here: the served path is the file's path
/// relative to `assets/`. The `.wasm` glue fetches `sema_wasm_bg.wasm` relative
/// to its own URL, so the two must stay co-located under the same prefix.
#[derive(RustEmbed)]
#[folder = "src/web/assets/"]
struct WebRuntime;

/// Extract the embedded runtime to a versioned temp dir and return its path, so
/// the dev server can serve it as static files. Idempotent: a file is rewritten
/// only when missing or size-mismatched, keeping the ~4.5 MB wasm write off the
/// hot path on repeat launches.
pub fn extract() -> std::io::Result<std::path::PathBuf> {
    let dir = std::env::temp_dir().join(concat!("sema-web-runtime-", env!("CARGO_PKG_VERSION")));
    for rel in WebRuntime::iter() {
        // Present by construction: `get` resolves any path `iter` yielded.
        let file = WebRuntime::get(rel.as_ref()).expect("embedded asset from iter() must resolve");
        let path = dir.join(rel.as_ref());
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let needs_write = std::fs::metadata(&path)
            .map(|m| m.len() != file.data.len() as u64)
            .unwrap_or(true);
        if needs_write {
            std::fs::write(&path, &file.data)?;
        }
    }
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::WebRuntime;

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
}
