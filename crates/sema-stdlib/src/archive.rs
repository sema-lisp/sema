use std::io::{Read as _, Write as _};
use std::path::{Component, Path, PathBuf};

use sema_core::{check_arity, Caps, SemaError, Value};

use crate::{register_fn, register_fn_gated};

/// Extract the byte payload of an argument: accept either a bytevector or a
/// string (whose UTF-8 bytes are used). gzip/compress should be usable on
/// text directly, so we don't force callers to build a bytevector first.
fn arg_bytes(arg: &Value, fn_name: &str) -> Result<Vec<u8>, SemaError> {
    if let Some(s) = arg.as_str() {
        Ok(s.as_bytes().to_vec())
    } else if let Some(bv) = arg.as_bytevector() {
        Ok(bv.to_vec())
    } else {
        Err(SemaError::type_error(
            "bytevector or string",
            arg.type_name(),
        ))
    }
    .map_err(|e: SemaError| SemaError::eval(format!("{fn_name}: {e}")))
}

/// Reject path components that would let an archive entry escape the
/// destination directory (zip-slip / tar traversal). We allow only normal
/// components and the current-dir marker; anything containing `..`, an
/// absolute root, or a Windows prefix is refused.
fn safe_relative(name: &str) -> Option<PathBuf> {
    let p = Path::new(name);
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::Normal(seg) => out.push(seg),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    if out.as_os_str().is_empty() {
        None
    } else {
        Some(out)
    }
}

/// True if the path looks gzip-compressed by extension.
fn looks_gzip(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".tar.gz") || lower.ends_with(".tgz")
}

pub fn register(env: &sema_core::Env, sandbox: &sema_core::Sandbox) {
    // (gzip/compress bytes-or-string) -> gzip-compressed bytevector. Pure.
    register_fn(env, "gzip/compress", |args| {
        check_arity!(args, "gzip/compress", 1);
        let data = arg_bytes(&args[0], "gzip/compress")?;
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder
            .write_all(&data)
            .map_err(|e| SemaError::eval(format!("gzip/compress: {e}")))?;
        let compressed = encoder
            .finish()
            .map_err(|e| SemaError::eval(format!("gzip/compress: {e}")))?;
        Ok(Value::bytevector(compressed))
    });

    // (gzip/decompress bytes) -> decompressed bytevector. Pure.
    register_fn(env, "gzip/decompress", |args| {
        check_arity!(args, "gzip/decompress", 1);
        let data = arg_bytes(&args[0], "gzip/decompress")?;
        let mut decoder = flate2::read::GzDecoder::new(&data[..]);
        let mut out = Vec::new();
        decoder
            .read_to_end(&mut out)
            .map_err(|e| SemaError::eval(format!("gzip/decompress: {e}")))?;
        Ok(Value::bytevector(out))
    });

    // (zip/create out-path files) -> entry count. Each file added under its basename.
    register_fn_gated(env, sandbox, Caps::FS_WRITE, "zip/create", |args| {
        check_arity!(args, "zip/create", 2);
        let out_path = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let files = args[1]
            .as_list()
            .ok_or_else(|| SemaError::type_error("list", args[1].type_name()))?;

        let file = std::fs::File::create(out_path)
            .map_err(|e| SemaError::Io(format!("zip/create {out_path}: {e}")))?;
        let mut writer = zip::ZipWriter::new(file);
        let options: zip::write::FileOptions<()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);

        let mut count = 0i64;
        let mut seen = std::collections::HashSet::new();
        for f in files {
            let src = f
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", f.type_name()))?;
            let name = Path::new(src)
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| SemaError::eval(format!("zip/create: bad file path {src}")))?;
            // Entries are keyed by basename; a duplicate would shadow earlier
            // data (most extractors keep the last). Refuse rather than lose it.
            if !seen.insert(name.to_string()) {
                return Err(SemaError::eval(format!(
                    "zip/create: duplicate entry name {name:?} (from {src})"
                )));
            }
            let data =
                std::fs::read(src).map_err(|e| SemaError::Io(format!("zip/create {src}: {e}")))?;
            writer
                .start_file(name, options)
                .map_err(|e| SemaError::eval(format!("zip/create: {e}")))?;
            writer
                .write_all(&data)
                .map_err(|e| SemaError::Io(format!("zip/create {src}: {e}")))?;
            count += 1;
        }
        writer
            .finish()
            .map_err(|e| SemaError::eval(format!("zip/create: {e}")))?;
        Ok(Value::int(count))
    });

    // (zip/extract zip-path dest-dir) -> count of entries extracted.
    register_fn_gated(env, sandbox, Caps::FS_WRITE, "zip/extract", |args| {
        check_arity!(args, "zip/extract", 2);
        let zip_path = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let dest_dir = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;

        let file = std::fs::File::open(zip_path)
            .map_err(|e| SemaError::Io(format!("zip/extract {zip_path}: {e}")))?;
        let mut archive = zip::ZipArchive::new(file)
            .map_err(|e| SemaError::eval(format!("zip/extract {zip_path}: {e}")))?;

        let dest_root = Path::new(dest_dir);
        std::fs::create_dir_all(dest_root)
            .map_err(|e| SemaError::Io(format!("zip/extract {dest_dir}: {e}")))?;

        let mut count = 0i64;
        let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
        for i in 0..archive.len() {
            let mut entry = archive
                .by_index(i)
                .map_err(|e| SemaError::eval(format!("zip/extract: {e}")))?;
            let name = entry.name().to_string();
            // zip-slip guard: skip entries whose path would escape dest-dir.
            let rel = match safe_relative(&name) {
                Some(r) => r,
                None => continue,
            };
            let target = dest_root.join(&rel);
            if entry.is_dir() || name.ends_with('/') {
                std::fs::create_dir_all(&target)
                    .map_err(|e| SemaError::Io(format!("zip/extract {name}: {e}")))?;
            } else {
                // A foreign archive can carry two file entries that map to the
                // same target; the create-side dedup doesn't protect extraction,
                // so refuse the duplicate instead of silently overwriting.
                if !seen.insert(rel.clone()) {
                    return Err(SemaError::eval(format!(
                        "zip/extract: duplicate entry target {} — refusing to overwrite",
                        rel.display()
                    )));
                }
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| SemaError::Io(format!("zip/extract {name}: {e}")))?;
                }
                let mut out = std::fs::File::create(&target)
                    .map_err(|e| SemaError::Io(format!("zip/extract {name}: {e}")))?;
                std::io::copy(&mut entry, &mut out)
                    .map_err(|e| SemaError::Io(format!("zip/extract {name}: {e}")))?;
            }
            count += 1;
        }
        Ok(Value::int(count))
    });

    // (zip/list zip-path) -> list of entry-name strings.
    register_fn_gated(env, sandbox, Caps::FS_READ, "zip/list", |args| {
        check_arity!(args, "zip/list", 1);
        let zip_path = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let file = std::fs::File::open(zip_path)
            .map_err(|e| SemaError::Io(format!("zip/list {zip_path}: {e}")))?;
        let mut archive = zip::ZipArchive::new(file)
            .map_err(|e| SemaError::eval(format!("zip/list {zip_path}: {e}")))?;
        let mut names = Vec::with_capacity(archive.len());
        for i in 0..archive.len() {
            let entry = archive
                .by_index(i)
                .map_err(|e| SemaError::eval(format!("zip/list: {e}")))?;
            names.push(Value::string(entry.name()));
        }
        Ok(Value::list(names))
    });

    // (tar/create out-path files) -> entry count. gzip-compressed if out-path
    // ends in .tar.gz / .tgz, else plain tar. Each file added under its basename.
    register_fn_gated(env, sandbox, Caps::FS_WRITE, "tar/create", |args| {
        check_arity!(args, "tar/create", 2);
        let out_path = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let files = args[1]
            .as_list()
            .ok_or_else(|| SemaError::type_error("list", args[1].type_name()))?;

        let out_file = std::fs::File::create(out_path)
            .map_err(|e| SemaError::Io(format!("tar/create {out_path}: {e}")))?;

        // Build into the appropriate writer; the count is gathered first so we
        // can return it after finishing/flushing the underlying stream.
        fn add_files<W: std::io::Write>(
            builder: &mut tar::Builder<W>,
            files: &[Value],
        ) -> Result<i64, SemaError> {
            let mut count = 0i64;
            let mut seen = std::collections::HashSet::new();
            for f in files {
                let src = f
                    .as_str()
                    .ok_or_else(|| SemaError::type_error("string", f.type_name()))?;
                let name = Path::new(src)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .ok_or_else(|| SemaError::eval(format!("tar/create: bad file path {src}")))?;
                // Files are stored under their basename; a duplicate basename
                // would silently shadow earlier data. Refuse it rather than lose it.
                if !seen.insert(name.to_string()) {
                    return Err(SemaError::eval(format!(
                        "tar/create: duplicate entry name {name:?} (from {src})"
                    )));
                }
                builder
                    .append_path_with_name(src, name)
                    .map_err(|e| SemaError::Io(format!("tar/create {src}: {e}")))?;
                count += 1;
            }
            Ok(count)
        }

        let count = if looks_gzip(out_path) {
            let encoder = flate2::write::GzEncoder::new(out_file, flate2::Compression::default());
            let mut builder = tar::Builder::new(encoder);
            let count = add_files(&mut builder, files)?;
            let encoder = builder
                .into_inner()
                .map_err(|e| SemaError::Io(format!("tar/create {out_path}: {e}")))?;
            encoder
                .finish()
                .map_err(|e| SemaError::Io(format!("tar/create {out_path}: {e}")))?;
            count
        } else {
            let mut builder = tar::Builder::new(out_file);
            let count = add_files(&mut builder, files)?;
            builder
                .finish()
                .map_err(|e| SemaError::Io(format!("tar/create {out_path}: {e}")))?;
            count
        };
        Ok(Value::int(count))
    });

    // (tar/extract tar-path dest-dir) -> entry count. gzip auto-detected by
    // extension or magic bytes. Guards against path traversal.
    register_fn_gated(env, sandbox, Caps::FS_WRITE, "tar/extract", |args| {
        check_arity!(args, "tar/extract", 2);
        let tar_path = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let dest_dir = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;

        let raw = std::fs::read(tar_path)
            .map_err(|e| SemaError::Io(format!("tar/extract {tar_path}: {e}")))?;
        // gzip magic: 0x1f 0x8b. Auto-detect by extension OR magic bytes.
        let gzipped = looks_gzip(tar_path) || raw.starts_with(&[0x1f, 0x8b]);

        let dest_root = Path::new(dest_dir);
        std::fs::create_dir_all(dest_root)
            .map_err(|e| SemaError::Io(format!("tar/extract {dest_dir}: {e}")))?;

        // Decompress up front (if needed) so the rest is a single tar-reading path.
        let tar_bytes: Vec<u8> = if gzipped {
            let mut decoder = flate2::read::GzDecoder::new(&raw[..]);
            let mut out = Vec::new();
            decoder
                .read_to_end(&mut out)
                .map_err(|e| SemaError::eval(format!("tar/extract {tar_path}: {e}")))?;
            out
        } else {
            raw
        };

        let mut archive = tar::Archive::new(&tar_bytes[..]);
        let mut count = 0i64;
        let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
        for entry in archive
            .entries()
            .map_err(|e| SemaError::eval(format!("tar/extract {tar_path}: {e}")))?
        {
            let mut entry = entry.map_err(|e| SemaError::eval(format!("tar/extract: {e}")))?;
            // Symlink/hardlink guard: a link entry (e.g. `evil -> /etc`) followed
            // by a regular entry written *through* it (`evil/passwd`) escapes
            // dest-dir even though neither path contains `..`. Refuse link
            // entries entirely so no traversal symlink is ever materialized.
            let etype = entry.header().entry_type();
            if etype.is_symlink() || etype.is_hard_link() {
                continue;
            }
            let path = entry
                .path()
                .map_err(|e| SemaError::eval(format!("tar/extract: {e}")))?;
            let name = path.to_string_lossy().to_string();
            // Traversal guard: skip entries that would escape dest-dir.
            let rel = match safe_relative(&name) {
                Some(r) => r,
                None => continue,
            };
            let target = dest_root.join(&rel);
            // Refuse a second entry mapping to the same file target (foreign
            // archives bypass the create-side dedup); directories may recur.
            if !entry.header().entry_type().is_dir() && !seen.insert(rel.clone()) {
                return Err(SemaError::eval(format!(
                    "tar/extract: duplicate entry target {} — refusing to overwrite",
                    rel.display()
                )));
            }
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| SemaError::Io(format!("tar/extract {name}: {e}")))?;
            }
            entry
                .unpack(&target)
                .map_err(|e| SemaError::Io(format!("tar/extract {name}: {e}")))?;
            count += 1;
        }
        Ok(Value::int(count))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_env() -> sema_core::Env {
        let env = sema_core::Env::new();
        let sandbox = sema_core::Sandbox::allow_all();
        register(&env, &sandbox);
        env
    }

    fn call(env: &sema_core::Env, name: &str, args: &[Value]) -> Value {
        let f = env
            .get(sema_core::intern(name))
            .unwrap_or_else(|| panic!("{name} not registered"));
        let nf = f.as_native_fn_ref().expect("native fn");
        let ctx = sema_core::EvalContext::new();
        (nf.func)(&ctx, args).unwrap_or_else(|e| panic!("{name} failed: {e}"))
    }

    /// Unique scratch directory under the system temp dir, removed on drop.
    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            let mut p = std::env::temp_dir();
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            p.push(format!("sema-archive-{tag}-{nanos}"));
            std::fs::create_dir_all(&p).unwrap();
            TempDir(p)
        }
        fn join(&self, s: &str) -> PathBuf {
            self.0.join(s)
        }
        fn join_str(&self, s: &str) -> String {
            self.join(s).to_string_lossy().to_string()
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn gzip_round_trip() {
        let env = test_env();
        let original = b"hello, sema gzip round-trip \x00\x01\x02 payload".to_vec();
        let compressed = call(
            &env,
            "gzip/compress",
            &[Value::bytevector(original.clone())],
        );
        assert!(compressed.as_bytevector().is_some());
        let decompressed = call(&env, "gzip/decompress", &[compressed]);
        assert_eq!(decompressed.as_bytevector().unwrap(), &original[..]);
    }

    #[test]
    fn tar_create_extract_round_trip() {
        let env = test_env();
        let dir = TempDir::new("tar");
        let f1 = dir.join_str("a.txt");
        let f2 = dir.join_str("b.txt");
        std::fs::write(&f1, b"alpha contents").unwrap();
        std::fs::write(&f2, b"beta contents").unwrap();

        let out = dir.join_str("bundle.tar.gz");
        let files = Value::list(vec![Value::string(&f1), Value::string(&f2)]);
        let n = call(&env, "tar/create", &[Value::string(&out), files]);
        assert_eq!(n.as_int(), Some(2));

        let dest = dir.join_str("out");
        let extracted = call(
            &env,
            "tar/extract",
            &[Value::string(&out), Value::string(&dest)],
        );
        assert_eq!(extracted.as_int(), Some(2));

        let a = std::fs::read(dir.join("out").join("a.txt")).unwrap();
        let b = std::fs::read(dir.join("out").join("b.txt")).unwrap();
        assert_eq!(a, b"alpha contents");
        assert_eq!(b, b"beta contents");
    }

    #[test]
    fn zip_create_and_list() {
        let env = test_env();
        let dir = TempDir::new("zip");
        let f1 = dir.join_str("one.txt");
        let f2 = dir.join_str("two.txt");
        std::fs::write(&f1, b"first").unwrap();
        std::fs::write(&f2, b"second").unwrap();

        let out = dir.join_str("bundle.zip");
        let files = Value::list(vec![Value::string(&f1), Value::string(&f2)]);
        let n = call(&env, "zip/create", &[Value::string(&out), files]);
        assert_eq!(n.as_int(), Some(2));

        let listed = call(&env, "zip/list", &[Value::string(&out)]);
        let entries = listed.as_list().unwrap();
        let mut names: Vec<String> = entries
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        names.sort();
        assert_eq!(names, vec!["one.txt".to_string(), "two.txt".to_string()]);
    }
}
