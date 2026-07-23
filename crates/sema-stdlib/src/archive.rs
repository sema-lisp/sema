//! Archive builtins (`zip/*`, `tar/*`, `gzip/*`).
//!
//! `zip/create`/`zip/extract`/`zip/list`/`tar/create`/`tar/extract` do real
//! file I/O plus CPU-bound (de)compression — reading/writing whole files and
//! walking a zip/tar central directory can take a while for a large archive.
//! Each builtin's actual work lives in a plain `*_work` function that returns
//! `Result<T, SemaError>` and touches nothing but its own arguments (no VM
//! state, no `Value` construction beyond what the caller already parsed out of
//! `args`); a direct native call outside the scheduler runs it synchronously,
//! while a runtime quantum offloads it through `quarantined_compute` (`io.rs`)
//! so the archive work doesn't block the VM thread and every sibling task. The offload's
//! `work` closure runs `*_work` entirely on the worker thread and converts any
//! `SemaError` to its rendered `String` (`.to_string()`) before returning —
//! `SemaError` itself never crosses the thread boundary (GLOBAL RULES: never
//! move `Value`/`SemaError`/`Rc`/`RefCell` across threads), only the plain
//! `Send` result (`i64` counts, `Vec<String>` entry names) does. The final
//! `Value` is built back on the VM thread when the scheduler polls the
//! completed offload. Before dispatch, the runtime branch captures immutable
//! input-byte, output-byte, and entry caps; path metadata and owned payloads are
//! checked on the VM thread, and the worker rechecks file inputs and enforces
//! decompression/write limits. Because those caps are enforced incrementally on
//! the worker (`read_path_bounded` / `BoundedWriter`), the archive work is finite
//! by construction, so the offload's resource declares a **terminal
//! `QuarantineBound::finite_work` descriptor** (R02) carrying the input-byte cap —
//! the runtime carries the declared unit cap, not just the wall-clock cleanup net.
//! Cancellation discards the eventual completion; it does not interrupt a worker
//! that has already started. Direct native calls outside a runtime quantum retain
//! their existing synchronous behavior.
//!
//! `gzip/compress`/`gzip/decompress` are pure in-memory
//! transforms (no file I/O) but their DEFLATE pass is still CPU-bound, so they
//! follow the same runtime-quantum gate with an owned `Vec<u8>` in and
//! `Vec<u8>` out (both `Send`).

use std::io::{Read as _, Write as _};
use std::num::NonZeroU64;
use std::path::{Component, Path, PathBuf};

use sema_core::runtime::{NativeOutcome, NativeResult, QuarantineBound};
use sema_core::{check_arity, Caps, SemaError, Value};

use crate::{register_runtime_fn, register_runtime_fn_gated};

const ARCHIVE_INPUT_BYTE_CAP: u64 = 256 * 1024 * 1024;
const ARCHIVE_OUTPUT_BYTE_CAP: u64 = 512 * 1024 * 1024;
const ARCHIVE_ENTRY_CAP: usize = 100_000;

/// The declared unit of the archive offload's terminal work bound. The archive
/// (de)compression ops enforce their input/output/entry caps incrementally on the
/// worker (`read_path_bounded` / `BoundedWriter`), so the work is finite by
/// construction — the runtime descriptor carries this terminal input-byte cap via
/// `QuarantineBound::finite_work` rather than only the wall-clock cleanup net.
const ARCHIVE_BOUND_KIND: &str = "input-bytes";

/// Build the archive offload's terminal finite-work bound (R02). Shared by every
/// archive offload and the descriptor-presence unit test, so both agree on the
/// declared unit and cap.
fn archive_finite_bound() -> QuarantineBound {
    QuarantineBound::finite_work(
        ARCHIVE_BOUND_KIND,
        NonZeroU64::new(ARCHIVE_INPUT_BYTE_CAP).expect("archive input byte cap is nonzero"),
    )
}

/// Offload a terminally-bounded archive job onto the I/O pool, tagging the
/// resource with the archive finite-work descriptor. Thin wrapper over
/// [`crate::io::quarantined_compute_bounded`] so every archive op declares the
/// same bound.
fn archive_offload<T, F>(op: &'static str, to_value: fn(T) -> Value, job: F) -> NativeResult
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    crate::io::quarantined_compute_bounded(op, to_value, archive_finite_bound(), job)
}

#[derive(Clone, Copy)]
struct ArchiveBounds {
    input_bytes: u64,
    output_bytes: u64,
    entries: usize,
}

const ARCHIVE_RUNTIME_BOUNDS: ArchiveBounds = ArchiveBounds {
    input_bytes: ARCHIVE_INPUT_BYTE_CAP,
    output_bytes: ARCHIVE_OUTPUT_BYTE_CAP,
    entries: ARCHIVE_ENTRY_CAP,
};

fn check_archive_limit(
    op: &str,
    dimension: &str,
    actual: u64,
    limit: u64,
) -> Result<(), SemaError> {
    if actual > limit {
        return Err(SemaError::eval(format!(
            "{op}: {dimension} {actual} exceeds the quarantined limit {limit}"
        ))
        .with_hint("reduce or split the archive input"));
    }
    Ok(())
}

fn archive_path_preflight(op: &str, path: &str, bounds: ArchiveBounds) -> Result<(), SemaError> {
    if let Ok(metadata) = std::fs::metadata(path) {
        if !metadata.is_file() {
            return Err(SemaError::eval(format!(
                "{op}: archive input must be a regular file: {path}"
            )));
        }
        check_archive_limit(op, "input bytes", metadata.len(), bounds.input_bytes)?;
    }
    Ok(())
}

fn archive_create_preflight(
    op: &str,
    files: &[String],
    bounds: ArchiveBounds,
) -> Result<(), SemaError> {
    check_archive_limit(op, "entries", files.len() as u64, bounds.entries as u64)?;
    let mut total = 0u64;
    for path in files {
        if let Ok(metadata) = std::fs::metadata(path) {
            if !metadata.is_file() {
                return Err(SemaError::eval(format!(
                    "{op}: archive input must be a regular file: {path}"
                )));
            }
            total = total.checked_add(metadata.len()).ok_or_else(|| {
                SemaError::eval(format!("{op}: total input byte count overflowed"))
            })?;
            check_archive_limit(op, "input bytes", total, bounds.input_bytes)?;
        }
    }
    Ok(())
}

struct BoundedWriter<W> {
    inner: W,
    position: u64,
    limit: u64,
}

impl<W> BoundedWriter<W> {
    fn new(inner: W, limit: u64) -> Self {
        Self {
            inner,
            position: 0,
            limit,
        }
    }

    fn into_inner(self) -> W {
        self.inner
    }
}

impl<W: std::io::Write> std::io::Write for BoundedWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let requested_end = self
            .position
            .checked_add(buf.len() as u64)
            .ok_or_else(|| std::io::Error::other("archive output byte count overflowed"))?;
        if requested_end > self.limit {
            return Err(std::io::Error::other(format!(
                "archive output exceeds the {}-byte quarantined limit",
                self.limit
            )));
        }
        let written = self.inner.write(buf)?;
        self.position += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

impl<W: std::io::Seek> std::io::Seek for BoundedWriter<W> {
    fn seek(&mut self, position: std::io::SeekFrom) -> std::io::Result<u64> {
        let next = self.inner.seek(position)?;
        if next > self.limit {
            return Err(std::io::Error::other(format!(
                "archive output seek exceeds the {}-byte quarantined limit",
                self.limit
            )));
        }
        self.position = next;
        Ok(next)
    }
}

fn open_regular_bounded(op: &str, path: &str, limit: u64) -> Result<std::fs::File, SemaError> {
    let metadata =
        std::fs::metadata(path).map_err(|e| SemaError::Io(format!("{op} {path}: {e}")))?;
    if !metadata.is_file() {
        return Err(SemaError::eval(format!(
            "{op}: archive input must be a regular file: {path}"
        )));
    }
    check_archive_limit(op, "input bytes", metadata.len(), limit)?;

    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options
        .open(path)
        .map_err(|e| SemaError::Io(format!("{op} {path}: {e}")))?;
    let opened_metadata = file
        .metadata()
        .map_err(|e| SemaError::Io(format!("{op} {path}: {e}")))?;
    if !opened_metadata.is_file() {
        return Err(SemaError::eval(format!(
            "{op}: archive input must be a regular file: {path}"
        )));
    }
    check_archive_limit(op, "input bytes", opened_metadata.len(), limit)?;
    Ok(file)
}

fn read_path_bounded(op: &str, path: &str, limit: u64) -> Result<Vec<u8>, SemaError> {
    let file = open_regular_bounded(op, path, limit)?;
    let mut bytes = Vec::new();
    file.take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|e| SemaError::Io(format!("{op} {path}: {e}")))?;
    check_archive_limit(op, "input bytes", bytes.len() as u64, limit)?;
    Ok(bytes)
}

/// Decode `zip/list`'s off-thread result (a `Vec<String>` of entry names) into a
/// Sema list on the VM thread. A plain `fn` (no captures) so it fits
/// [`crate::io::quarantined_compute`]'s `fn(T) -> Value` decoder slot.
fn zip_names_to_value(names: Vec<String>) -> Value {
    Value::list(names.iter().map(|s| Value::string(s)).collect())
}

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

/// `zip/create`'s actual work: build `out_path` from `files`, each stored
/// under its basename. Runtime callers supply the bounds captured before
/// dispatch; direct callers outside the runtime supply none.
fn zip_create_work(
    out_path: &str,
    files: &[String],
    bounds: Option<ArchiveBounds>,
) -> Result<i64, SemaError> {
    let file = std::fs::File::create(out_path)
        .map_err(|e| SemaError::Io(format!("zip/create {out_path}: {e}")))?;
    let mut writer = zip::ZipWriter::new(BoundedWriter::new(
        file,
        bounds.map_or(u64::MAX, |bounds| bounds.output_bytes),
    ));
    let options: zip::write::FileOptions<()> =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    let mut count = 0i64;
    let mut input_bytes = 0u64;
    let mut seen = std::collections::HashSet::new();
    for src in files {
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
        let data = match bounds {
            Some(bounds) => read_path_bounded(
                "zip/create",
                src,
                bounds.input_bytes.saturating_sub(input_bytes),
            )?,
            None => {
                std::fs::read(src).map_err(|e| SemaError::Io(format!("zip/create {src}: {e}")))?
            }
        };
        if let Some(bounds) = bounds {
            input_bytes = input_bytes
                .checked_add(data.len() as u64)
                .ok_or_else(|| SemaError::eval("zip/create: input byte count overflowed"))?;
            check_archive_limit("zip/create", "input bytes", input_bytes, bounds.input_bytes)?;
        }
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
    Ok(count)
}

/// `zip/extract`'s actual work: unpack `zip_path` into `dest_dir`, enforcing
/// captured runtime bounds when present.
fn zip_extract_work(
    zip_path: &str,
    dest_dir: &str,
    bounds: Option<ArchiveBounds>,
) -> Result<i64, SemaError> {
    match bounds {
        Some(bounds) => {
            let bytes = read_path_bounded("zip/extract", zip_path, bounds.input_bytes)?;
            zip_extract_from_reader(
                std::io::Cursor::new(bytes),
                zip_path,
                dest_dir,
                Some(bounds),
            )
        }
        None => {
            let file = std::fs::File::open(zip_path)
                .map_err(|e| SemaError::Io(format!("zip/extract {zip_path}: {e}")))?;
            zip_extract_from_reader(file, zip_path, dest_dir, None)
        }
    }
}

fn zip_extract_from_reader<R: std::io::Read + std::io::Seek>(
    reader: R,
    zip_path: &str,
    dest_dir: &str,
    bounds: Option<ArchiveBounds>,
) -> Result<i64, SemaError> {
    let mut archive = zip::ZipArchive::new(reader)
        .map_err(|e| SemaError::eval(format!("zip/extract {zip_path}: {e}")))?;
    if let Some(bounds) = bounds {
        check_archive_limit(
            "zip/extract",
            "entries",
            archive.len() as u64,
            bounds.entries as u64,
        )?;
    }

    let dest_root = Path::new(dest_dir);
    std::fs::create_dir_all(dest_root)
        .map_err(|e| SemaError::Io(format!("zip/extract {dest_dir}: {e}")))?;

    let mut count = 0i64;
    let mut declared_output_bytes = 0u64;
    let mut actual_output_bytes = 0u64;
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
        if let Some(bounds) = bounds {
            declared_output_bytes = declared_output_bytes
                .checked_add(entry.size())
                .ok_or_else(|| SemaError::eval("zip/extract: output byte count overflowed"))?;
            check_archive_limit(
                "zip/extract",
                "output bytes",
                declared_output_bytes,
                bounds.output_bytes,
            )?;
        }
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
            let out = std::fs::File::create(&target)
                .map_err(|e| SemaError::Io(format!("zip/extract {name}: {e}")))?;
            let remaining = bounds.map_or(u64::MAX, |bounds| {
                bounds.output_bytes.saturating_sub(actual_output_bytes)
            });
            let mut out = BoundedWriter::new(out, remaining);
            let copied = std::io::copy(&mut entry, &mut out)
                .map_err(|e| SemaError::Io(format!("zip/extract {name}: {e}")))?;
            actual_output_bytes = actual_output_bytes
                .checked_add(copied)
                .ok_or_else(|| SemaError::eval("zip/extract: output byte count overflowed"))?;
        }
        count += 1;
    }
    Ok(count)
}

/// `zip/list`'s actual work: read `zip_path`'s central directory entry names,
/// enforcing captured runtime bounds when present.
fn zip_list_work(zip_path: &str, bounds: Option<ArchiveBounds>) -> Result<Vec<String>, SemaError> {
    match bounds {
        Some(bounds) => {
            let bytes = read_path_bounded("zip/list", zip_path, bounds.input_bytes)?;
            zip_list_from_reader(std::io::Cursor::new(bytes), zip_path, Some(bounds))
        }
        None => {
            let file = std::fs::File::open(zip_path)
                .map_err(|e| SemaError::Io(format!("zip/list {zip_path}: {e}")))?;
            zip_list_from_reader(file, zip_path, None)
        }
    }
}

fn zip_list_from_reader<R: std::io::Read + std::io::Seek>(
    reader: R,
    zip_path: &str,
    bounds: Option<ArchiveBounds>,
) -> Result<Vec<String>, SemaError> {
    let mut archive = zip::ZipArchive::new(reader)
        .map_err(|e| SemaError::eval(format!("zip/list {zip_path}: {e}")))?;
    if let Some(bounds) = bounds {
        check_archive_limit(
            "zip/list",
            "entries",
            archive.len() as u64,
            bounds.entries as u64,
        )?;
    }
    let mut names = Vec::with_capacity(archive.len());
    let mut output_bytes = 0u64;
    for i in 0..archive.len() {
        let entry = archive
            .by_index(i)
            .map_err(|e| SemaError::eval(format!("zip/list: {e}")))?;
        let name = entry.name().to_string();
        if let Some(bounds) = bounds {
            output_bytes = output_bytes
                .checked_add(name.len() as u64)
                .ok_or_else(|| SemaError::eval("zip/list: output byte count overflowed"))?;
            check_archive_limit(
                "zip/list",
                "output bytes",
                output_bytes,
                bounds.output_bytes,
            )?;
        }
        names.push(name);
    }
    Ok(names)
}

/// Append `files` to `builder`, each stored under its basename; refuses a
/// duplicate basename rather than silently shadowing earlier data. Used by
/// both the gzip and plain writers in [`tar_create_work`].
fn tar_add_files<W: std::io::Write>(
    builder: &mut tar::Builder<W>,
    files: &[String],
    bounds: Option<ArchiveBounds>,
) -> Result<i64, SemaError> {
    let mut count = 0i64;
    let mut input_bytes = 0u64;
    let mut seen = std::collections::HashSet::new();
    for src in files {
        let name = Path::new(src)
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| SemaError::eval(format!("tar/create: bad file path {src}")))?;
        if !seen.insert(name.to_string()) {
            return Err(SemaError::eval(format!(
                "tar/create: duplicate entry name {name:?} (from {src})"
            )));
        }
        if let Some(bounds) = bounds {
            let data = read_path_bounded(
                "tar/create",
                src,
                bounds.input_bytes.saturating_sub(input_bytes),
            )?;
            input_bytes = input_bytes
                .checked_add(data.len() as u64)
                .ok_or_else(|| SemaError::eval("tar/create: input byte count overflowed"))?;
            check_archive_limit("tar/create", "input bytes", input_bytes, bounds.input_bytes)?;
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            builder
                .append_data(&mut header, name, &data[..])
                .map_err(|e| SemaError::Io(format!("tar/create {src}: {e}")))?;
        } else {
            builder
                .append_path_with_name(src, name)
                .map_err(|e| SemaError::Io(format!("tar/create {src}: {e}")))?;
        }
        count += 1;
    }
    Ok(count)
}

/// `tar/create`'s actual work: build `out_path` from `files`, gzip-compressed
/// if `out_path` looks gzip (see [`looks_gzip`]), else plain tar.
fn tar_create_work(
    out_path: &str,
    files: &[String],
    bounds: Option<ArchiveBounds>,
) -> Result<i64, SemaError> {
    let out_file = std::fs::File::create(out_path)
        .map_err(|e| SemaError::Io(format!("tar/create {out_path}: {e}")))?;
    let out_file = BoundedWriter::new(
        out_file,
        bounds.map_or(u64::MAX, |bounds| bounds.output_bytes),
    );

    if looks_gzip(out_path) {
        let encoder = flate2::write::GzEncoder::new(out_file, flate2::Compression::default());
        let mut builder = tar::Builder::new(encoder);
        let count = tar_add_files(&mut builder, files, bounds)?;
        let encoder = builder
            .into_inner()
            .map_err(|e| SemaError::Io(format!("tar/create {out_path}: {e}")))?;
        encoder
            .finish()
            .map_err(|e| SemaError::Io(format!("tar/create {out_path}: {e}")))?;
        Ok(count)
    } else {
        let mut builder = tar::Builder::new(out_file);
        let count = tar_add_files(&mut builder, files, bounds)?;
        builder
            .finish()
            .map_err(|e| SemaError::Io(format!("tar/create {out_path}: {e}")))?;
        Ok(count)
    }
}

/// `tar/extract`'s actual work: unpack `tar_path` (gzip auto-detected by
/// extension or magic bytes) into `dest_dir`.
fn tar_extract_work(
    tar_path: &str,
    dest_dir: &str,
    bounds: Option<ArchiveBounds>,
) -> Result<i64, SemaError> {
    let raw = match bounds {
        Some(bounds) => read_path_bounded("tar/extract", tar_path, bounds.input_bytes)?,
        None => std::fs::read(tar_path)
            .map_err(|e| SemaError::Io(format!("tar/extract {tar_path}: {e}")))?,
    };
    // gzip magic: 0x1f 0x8b. Auto-detect by extension OR magic bytes.
    let gzipped = looks_gzip(tar_path) || raw.starts_with(&[0x1f, 0x8b]);

    let dest_root = Path::new(dest_dir);
    std::fs::create_dir_all(dest_root)
        .map_err(|e| SemaError::Io(format!("tar/extract {dest_dir}: {e}")))?;

    // Decompress up front (if needed) so the rest is a single tar-reading path.
    let tar_bytes: Vec<u8> = if gzipped {
        let mut decoder = flate2::read::GzDecoder::new(&raw[..]);
        let mut out = BoundedWriter::new(
            Vec::new(),
            bounds.map_or(u64::MAX, |bounds| bounds.output_bytes),
        );
        std::io::copy(&mut decoder, &mut out)
            .map_err(|e| SemaError::eval(format!("tar/extract {tar_path}: {e}")))?;
        out.into_inner()
    } else {
        raw
    };

    let mut archive = tar::Archive::new(&tar_bytes[..]);
    let mut count = 0i64;
    let mut entries_seen = 0u64;
    let mut output_bytes = 0u64;
    let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    for entry in archive
        .entries()
        .map_err(|e| SemaError::eval(format!("tar/extract {tar_path}: {e}")))?
    {
        let mut entry = entry.map_err(|e| SemaError::eval(format!("tar/extract: {e}")))?;
        let etype = entry.header().entry_type();
        if let Some(bounds) = bounds {
            entries_seen += 1;
            check_archive_limit(
                "tar/extract",
                "entries",
                entries_seen,
                bounds.entries as u64,
            )?;
            // `Entry::size()` is the stored payload length for GNU sparse
            // entries. `Header::size()` is the logical file size that unpack
            // materializes, including sparse holes, so it is the resource
            // quantity the extraction budget must cover.
            let logical_size = entry
                .header()
                .size()
                .map_err(|e| SemaError::eval(format!("tar/extract: {e}")))?;
            output_bytes = output_bytes
                .checked_add(logical_size)
                .ok_or_else(|| SemaError::eval("tar/extract: output byte count overflowed"))?;
            check_archive_limit(
                "tar/extract",
                "output bytes",
                output_bytes,
                bounds.output_bytes,
            )?;
        }
        // Symlink/hardlink guard: a link entry (e.g. `evil -> /etc`) followed
        // by a regular entry written *through* it (`evil/passwd`) escapes
        // dest-dir even though neither path contains `..`. Refuse link
        // entries entirely so no traversal symlink is ever materialized.
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
    Ok(count)
}

/// Extract a list of string paths out of a Sema list `Value`, for builtins
/// that need the owned `Vec<String>` up front (both to validate before doing
/// any work, and because the runtime path needs `Send` owned data).
/// The CPU-bound half of `gzip/compress`: DEFLATE `data` into a gzip byte
/// stream. Touches nothing but its own argument, so it's safe to run on an
/// I/O-pool worker via `quarantined_compute` during a runtime quantum.
fn gzip_compress_work(data: &[u8], output_limit: Option<u64>) -> Result<Vec<u8>, SemaError> {
    let output = BoundedWriter::new(Vec::new(), output_limit.unwrap_or(u64::MAX));
    let mut encoder = flate2::write::GzEncoder::new(output, flate2::Compression::default());
    encoder
        .write_all(data)
        .map_err(|e| SemaError::eval(format!("gzip/compress: {e}")))?;
    encoder
        .finish()
        .map(BoundedWriter::into_inner)
        .map_err(|e| SemaError::eval(format!("gzip/compress: {e}")))
}

/// The CPU-bound half of `gzip/decompress`: inflate a gzip byte stream. Same
/// offload rationale as `gzip_compress_work`.
fn gzip_decompress_work(data: &[u8], output_limit: Option<u64>) -> Result<Vec<u8>, SemaError> {
    let mut decoder = flate2::read::GzDecoder::new(data);
    let mut out = BoundedWriter::new(Vec::new(), output_limit.unwrap_or(u64::MAX));
    std::io::copy(&mut decoder, &mut out)
        .map_err(|e| SemaError::eval(format!("gzip/decompress: {e}")))?;
    Ok(out.into_inner())
}

fn string_list_arg(list: &[Value], fn_name: &str) -> Result<Vec<String>, SemaError> {
    list.iter()
        .map(|f| {
            f.as_str()
                .map(|s| s.to_string())
                .ok_or_else(|| SemaError::type_error("string", f.type_name()))
        })
        .collect::<Result<Vec<String>, SemaError>>()
        .map_err(|e| SemaError::eval(format!("{fn_name}: {e}")))
}

pub fn register(env: &sema_core::Env, sandbox: &sema_core::Sandbox) {
    // (gzip/compress bytes-or-string) -> gzip-compressed bytevector. The
    // DEFLATE pass is CPU-bound; during a runtime quantum it is offloaded so it
    // does not stall the VM thread for a large payload.
    register_runtime_fn(env, "gzip/compress", |args| {
        check_arity!(args, "gzip/compress", 1);
        let data = arg_bytes(&args[0], "gzip/compress")?;
        if sema_core::in_runtime_quantum() {
            let bounds = ARCHIVE_RUNTIME_BOUNDS;
            check_archive_limit(
                "gzip/compress",
                "input bytes",
                data.len() as u64,
                bounds.input_bytes,
            )?;
            return archive_offload("gzip/compress", Value::bytevector, move || {
                gzip_compress_work(&data, Some(bounds.output_bytes)).map_err(|e| e.to_string())
            });
        }
        Ok(NativeOutcome::Return(Value::bytevector(
            gzip_compress_work(&data, None)?,
        )))
    });

    // (gzip/decompress bytes) -> decompressed bytevector. Same offload gate
    // as gzip/compress.
    register_runtime_fn(env, "gzip/decompress", |args| {
        check_arity!(args, "gzip/decompress", 1);
        let data = arg_bytes(&args[0], "gzip/decompress")?;
        if sema_core::in_runtime_quantum() {
            let bounds = ARCHIVE_RUNTIME_BOUNDS;
            check_archive_limit(
                "gzip/decompress",
                "input bytes",
                data.len() as u64,
                bounds.input_bytes,
            )?;
            return archive_offload("gzip/decompress", Value::bytevector, move || {
                gzip_decompress_work(&data, Some(bounds.output_bytes)).map_err(|e| e.to_string())
            });
        }
        Ok(NativeOutcome::Return(Value::bytevector(
            gzip_decompress_work(&data, None)?,
        )))
    });

    // (zip/create out-path files) -> entry count. Each file added under its basename.
    register_runtime_fn_gated(env, sandbox, Caps::FS_WRITE, "zip/create", |args| {
        check_arity!(args, "zip/create", 2);
        let out_path = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?
            .to_string();
        let files = string_list_arg(
            args[1]
                .as_list()
                .ok_or_else(|| SemaError::type_error("list", args[1].type_name()))?,
            "zip/create",
        )?;

        if sema_core::in_runtime_quantum() {
            let bounds = ARCHIVE_RUNTIME_BOUNDS;
            archive_create_preflight("zip/create", &files, bounds)?;
            return archive_offload("zip/create", Value::int, move || {
                zip_create_work(&out_path, &files, Some(bounds)).map_err(|e| e.to_string())
            });
        }
        Ok(NativeOutcome::Return(Value::int(zip_create_work(
            &out_path, &files, None,
        )?)))
    });

    // (zip/extract zip-path dest-dir) -> count of entries extracted.
    register_runtime_fn_gated(env, sandbox, Caps::FS_WRITE, "zip/extract", |args| {
        check_arity!(args, "zip/extract", 2);
        let zip_path = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?
            .to_string();
        let dest_dir = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?
            .to_string();

        if sema_core::in_runtime_quantum() {
            let bounds = ARCHIVE_RUNTIME_BOUNDS;
            archive_path_preflight("zip/extract", &zip_path, bounds)?;
            return archive_offload("zip/extract", Value::int, move || {
                zip_extract_work(&zip_path, &dest_dir, Some(bounds)).map_err(|e| e.to_string())
            });
        }
        Ok(NativeOutcome::Return(Value::int(zip_extract_work(
            &zip_path, &dest_dir, None,
        )?)))
    });

    // (zip/list zip-path) -> list of entry-name strings.
    register_runtime_fn_gated(env, sandbox, Caps::FS_READ, "zip/list", |args| {
        check_arity!(args, "zip/list", 1);
        let zip_path = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?
            .to_string();

        if sema_core::in_runtime_quantum() {
            let bounds = ARCHIVE_RUNTIME_BOUNDS;
            archive_path_preflight("zip/list", &zip_path, bounds)?;
            return archive_offload("zip/list", zip_names_to_value, move || {
                zip_list_work(&zip_path, Some(bounds)).map_err(|e| e.to_string())
            });
        }
        Ok(NativeOutcome::Return(zip_names_to_value(zip_list_work(
            &zip_path, None,
        )?)))
    });

    // (tar/create out-path files) -> entry count. gzip-compressed if out-path
    // ends in .tar.gz / .tgz, else plain tar. Each file added under its basename.
    register_runtime_fn_gated(env, sandbox, Caps::FS_WRITE, "tar/create", |args| {
        check_arity!(args, "tar/create", 2);
        let out_path = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?
            .to_string();
        let files = string_list_arg(
            args[1]
                .as_list()
                .ok_or_else(|| SemaError::type_error("list", args[1].type_name()))?,
            "tar/create",
        )?;

        if sema_core::in_runtime_quantum() {
            let bounds = ARCHIVE_RUNTIME_BOUNDS;
            archive_create_preflight("tar/create", &files, bounds)?;
            return archive_offload("tar/create", Value::int, move || {
                tar_create_work(&out_path, &files, Some(bounds)).map_err(|e| e.to_string())
            });
        }
        Ok(NativeOutcome::Return(Value::int(tar_create_work(
            &out_path, &files, None,
        )?)))
    });

    // (tar/extract tar-path dest-dir) -> entry count. gzip auto-detected by
    // extension or magic bytes. Guards against path traversal.
    register_runtime_fn_gated(env, sandbox, Caps::FS_WRITE, "tar/extract", |args| {
        check_arity!(args, "tar/extract", 2);
        let tar_path = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?
            .to_string();
        let dest_dir = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?
            .to_string();

        if sema_core::in_runtime_quantum() {
            let bounds = ARCHIVE_RUNTIME_BOUNDS;
            archive_path_preflight("tar/extract", &tar_path, bounds)?;
            return archive_offload("tar/extract", Value::int, move || {
                tar_extract_work(&tar_path, &dest_dir, Some(bounds)).map_err(|e| e.to_string())
            });
        }
        Ok(NativeOutcome::Return(Value::int(tar_extract_work(
            &tar_path, &dest_dir, None,
        )?)))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quarantine_limit_accepts_boundary_and_rejects_one_over() {
        assert!(check_archive_limit("zip/create", "input bytes", 8, 8).is_ok());
        let error = check_archive_limit("zip/create", "input bytes", 9, 8)
            .expect_err("one byte over the captured limit must fail");
        assert!(error.to_string().contains("9"));
        assert!(error.to_string().contains("8"));
    }

    /// R02 finalization: the archive offload declares a TERMINAL finite-work bound
    /// (the input-byte cap), not just the wall-clock cleanup deadline.
    #[test]
    fn archive_offload_declares_finite_work_bound() {
        use sema_core::runtime::QuarantineBoundDescriptor;
        let bound = archive_finite_bound();
        match bound.descriptor() {
            QuarantineBoundDescriptor::FiniteWork {
                kind,
                maximum_units,
            } => {
                assert_eq!(kind, ARCHIVE_BOUND_KIND);
                assert_eq!(maximum_units.get(), ARCHIVE_INPUT_BYTE_CAP);
            }
            QuarantineBoundDescriptor::HardDeadline(_) => {
                panic!("archive offload must carry a terminal finite-work bound, not a deadline")
            }
        }
        assert!(
            bound.hard_deadline_value().is_none(),
            "a finite-work bound carries no hard deadline"
        );
    }

    #[test]
    fn quarantine_output_writer_stops_at_captured_boundary() {
        let mut writer = BoundedWriter::new(Vec::new(), 8);
        writer.write_all(b"12345678").expect("exact boundary");
        let error = writer
            .write_all(b"9")
            .expect_err("one byte over output bound must fail");
        assert!(error.to_string().contains("8-byte"));
        assert_eq!(writer.into_inner(), b"12345678");
    }

    #[test]
    fn quarantine_create_preflight_rejects_oversized_input() {
        let dir = TempDir::new("preflight-cap");
        let input = dir.join("oversized.bin");
        std::fs::File::create(&input)
            .and_then(|file| file.set_len(9))
            .expect("create sparse input");
        let input = input.to_string_lossy().into_owned();
        let bounds = ArchiveBounds {
            input_bytes: 8,
            output_bytes: 16,
            entries: 1,
        };

        let error = archive_create_preflight("zip/create", &[input], bounds)
            .expect_err("oversized source must fail before dispatch");
        assert!(error.to_string().contains("input bytes"));

        let entries = vec!["missing-a".to_string(), "missing-b".to_string()];
        let error = archive_create_preflight("zip/create", &entries, bounds)
            .expect_err("one entry over the captured limit must fail");
        assert!(error.to_string().contains("entries"));
    }

    #[test]
    fn bounded_tar_extract_counts_gnu_sparse_logical_size() {
        let mut header = tar::Header::new_gnu();
        header
            .set_path("sparse.bin")
            .expect("set sparse entry path");
        header.set_entry_type(tar::EntryType::GNUSparse);
        header.set_size(1);
        let gnu = header.as_gnu_mut().expect("GNU header");
        gnu.set_real_size(9);
        gnu.sparse[0].set_offset(8);
        gnu.sparse[0].set_length(1);
        header.set_cksum();

        let mut builder = tar::Builder::new(Vec::new());
        builder
            .append(&header, std::io::Cursor::new(b"x"))
            .expect("append sparse entry");
        let archive = builder.into_inner().expect("finish sparse archive");

        let dir = TempDir::new("sparse-output-cap");
        let tar_path = dir.join("sparse.tar");
        std::fs::write(&tar_path, archive).expect("write sparse archive");
        let dest = dir.join("out");
        let bounds = ArchiveBounds {
            input_bytes: 4096,
            output_bytes: 8,
            entries: 1,
        };

        let error = tar_extract_work(
            tar_path.to_str().expect("UTF-8 tar path"),
            dest.to_str().expect("UTF-8 destination"),
            Some(bounds),
        )
        .expect_err("logical sparse size must be checked before unpack");

        assert!(error.to_string().contains("output bytes"), "{error}");
        assert!(!dest.join("sparse.bin").exists());
    }

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
