//! PDF builtins (`pdf/*`).
//!
//! Every builtin reads a whole file plus runs a CPU-bound parse (page
//! extraction, xref/trailer walk), so — like `archive.rs` — each one's actual
//! work lives in a plain `*_work` function returning `Result<T, SemaError>`.
//! All runtime paths capture a regular-file descriptor before dispatch and read
//! a capped worker-owned byte snapshot.
//!
//! **R10 splits honestly into a terminal admission arm and a non-terminal parser
//! arm.** *R10A — input-byte admission (terminal):* the pre-dispatch
//! `open_pdf_runtime_input` `stat`s the file and rejects an oversized input before
//! any worker runs, so the input byte count is a genuinely terminal bound. *R10B —
//! parser quarantine (non-terminal):* the page and returned-text caps are useful
//! guardrails but only run **post-parse** — `lopdf` can allocate and decompress
//! object streams while loading, page traversal allocates before its count is
//! checked, and `pdf-extract` decompresses complete content streams into
//! intermediate buffers. So the offloaded parse is NOT terminally bounded: it
//! keeps the `hard_deadline` cleanup net (via `quarantined_compute`), not a
//! `finite_work` descriptor. Terminally bounding the parser needs subprocess (or
//! parser) isolation, which is deferred (see `docs/deferred.md`, R10B). The public
//! operations remain available and offloaded meanwhile. Cancellation discards an
//! eventual result; it does not interrupt an already-running worker.

use std::collections::BTreeMap;
use std::io::Read as _;

use sema_core::runtime::NativeOutcome;
use sema_core::{check_arity, Caps, SemaError, Value};

const PDF_INPUT_BYTE_CAP: u64 = 256 * 1024 * 1024;
const PDF_TEXT_OUTPUT_BYTE_CAP: u64 = 256 * 1024 * 1024;
const PDF_PAGE_CAP: usize = 10_000;

#[derive(Clone, Copy, Debug)]
struct PdfBounds {
    input_bytes: u64,
    output_bytes: u64,
    pages: usize,
}

const PDF_RUNTIME_BOUNDS: PdfBounds = PdfBounds {
    input_bytes: PDF_INPUT_BYTE_CAP,
    output_bytes: PDF_TEXT_OUTPUT_BYTE_CAP,
    pages: PDF_PAGE_CAP,
};

fn check_pdf_limit(op: &str, dimension: &str, actual: u64, limit: u64) -> Result<(), SemaError> {
    if actual > limit {
        return Err(SemaError::eval(format!(
            "{op}: {dimension} {actual} exceeds the quarantined limit {limit}"
        ))
        .with_hint("reduce or split the PDF input"));
    }
    Ok(())
}

#[derive(Debug)]
struct PdfRuntimeInput {
    file: std::fs::File,
    bounds: PdfBounds,
}

fn open_pdf_runtime_input(
    op: &str,
    path: &str,
    bounds: PdfBounds,
) -> Result<PdfRuntimeInput, SemaError> {
    let metadata =
        std::fs::metadata(path).map_err(|e| SemaError::Io(format!("{op} {path}: {e}")))?;
    if !metadata.is_file() {
        return Err(SemaError::eval(format!(
            "{op}: PDF input must be a regular file: {path}"
        )));
    }
    check_pdf_limit(op, "input bytes", metadata.len(), bounds.input_bytes)?;
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
    let opened = file
        .metadata()
        .map_err(|e| SemaError::Io(format!("{op} {path}: {e}")))?;
    if !opened.is_file() {
        return Err(SemaError::eval(format!(
            "{op}: PDF input must be a regular file: {path}"
        )));
    }
    check_pdf_limit(op, "input bytes", opened.len(), bounds.input_bytes)?;
    Ok(PdfRuntimeInput { file, bounds })
}

fn read_pdf_bounded(
    op: &str,
    path: &str,
    input: PdfRuntimeInput,
) -> Result<(Vec<u8>, PdfBounds), SemaError> {
    let PdfRuntimeInput { file, bounds } = input;
    let mut bytes = Vec::new();
    file.take(bounds.input_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|e| SemaError::Io(format!("{op} {path}: {e}")))?;
    check_pdf_limit(op, "input bytes", bytes.len() as u64, bounds.input_bytes)?;
    Ok((bytes, bounds))
}

fn check_pdf_pages(op: &str, pages: usize, bounds: PdfBounds) -> Result<(), SemaError> {
    check_pdf_limit(op, "pages", pages as u64, bounds.pages as u64)
}

fn check_pdf_text_output(
    op: &str,
    texts: impl IntoIterator<Item = usize>,
    bounds: PdfBounds,
) -> Result<(), SemaError> {
    let total = texts.into_iter().try_fold(0u64, |total, len| {
        total
            .checked_add(len as u64)
            .ok_or_else(|| SemaError::eval(format!("{op}: output byte count overflowed")))
    })?;
    check_pdf_limit(op, "output bytes", total, bounds.output_bytes)
}

/// Decode `pdf/extract-text-pages`'s off-thread result (per-page `String`s) into
/// a Sema list on the VM thread. A plain `fn` (no captures) so it fits the
/// `fn(T) -> Value` decoder slot of `quarantined_compute`.
fn pages_to_value(pages: Vec<String>) -> Value {
    Value::list(pages.iter().map(|s| Value::string(s)).collect())
}

/// `pdf/extract-text` work. Runtime bytes are capped, but the dependency's
/// internal stream decompression remains unbounded relative to that cap.
fn extract_text_work(path: &str, input: Option<PdfRuntimeInput>) -> Result<String, SemaError> {
    if let Some(input) = input {
        let (bytes, _bounds) = read_pdf_bounded("pdf/extract-text", path, input)?;
        return pdf_extract::extract_text_from_mem(&bytes)
            .map_err(|e| SemaError::eval(format!("pdf/extract-text {path}: {e}")));
    }
    pdf_extract::extract_text(path)
        .map_err(|e| SemaError::eval(format!("pdf/extract-text {path}: {e}")))
}

/// `pdf/extract-text-pages` work, with the same decompression limitation.
fn extract_text_pages_work(
    path: &str,
    input: Option<PdfRuntimeInput>,
) -> Result<Vec<String>, SemaError> {
    if let Some(input) = input {
        let (bytes, _bounds) = read_pdf_bounded("pdf/extract-text-pages", path, input)?;
        return pdf_extract::extract_text_from_mem_by_pages(&bytes)
            .map_err(|e| SemaError::eval(format!("pdf/extract-text-pages {path}: {e}")));
    }
    pdf_extract::extract_text_by_pages(path)
        .map_err(|e| SemaError::eval(format!("pdf/extract-text-pages {path}: {e}")))
}

/// `pdf/page-count`'s actual work, with optional runtime guardrails.
fn page_count_work(path: &str, input: Option<PdfRuntimeInput>) -> Result<i64, SemaError> {
    let (doc, bounds) = if let Some(input) = input {
        let (bytes, bounds) = read_pdf_bounded("pdf/page-count", path, input)?;
        (
            lopdf::Document::load_mem(&bytes)
                .map_err(|e| SemaError::Io(format!("pdf/page-count {path}: {e}")))?,
            Some(bounds),
        )
    } else {
        (
            lopdf::Document::load(path)
                .map_err(|e| SemaError::Io(format!("pdf/page-count {path}: {e}")))?,
            None,
        )
    };
    let pages = doc.get_pages().len();
    if let Some(bounds) = bounds {
        check_pdf_pages("pdf/page-count", pages, bounds)?;
    }
    Ok(pages as i64)
}

/// The plain, `Send` facts `pdf/metadata` extracts from a document — the
/// intermediate that crosses the offload's thread boundary; the `Value` map
/// is built from this on the VM thread in [`metadata_to_value`].
struct PdfMetadata {
    title: Option<String>,
    author: Option<String>,
    subject: Option<String>,
    creator: Option<String>,
    producer: Option<String>,
    pages: i64,
}

/// `pdf/metadata`'s actual work, with optional runtime guardrails.
fn metadata_work(path: &str, input: Option<PdfRuntimeInput>) -> Result<PdfMetadata, SemaError> {
    let (doc, bounds) = if let Some(input) = input {
        let (bytes, bounds) = read_pdf_bounded("pdf/metadata", path, input)?;
        (
            lopdf::Document::load_mem(&bytes)
                .map_err(|e| SemaError::Io(format!("pdf/metadata {path}: {e}")))?,
            Some(bounds),
        )
    } else {
        (
            lopdf::Document::load(path)
                .map_err(|e| SemaError::Io(format!("pdf/metadata {path}: {e}")))?,
            None,
        )
    };

    let extract_field = |dict: &lopdf::Dictionary, key: &[u8]| -> Option<String> {
        dict.get(key).ok().and_then(|val| {
            val.as_name()
                .map(|s| String::from_utf8_lossy(s).to_string())
                .ok()
                .or_else(|| {
                    val.as_str()
                        .map(|s| String::from_utf8_lossy(s).to_string())
                        .ok()
                })
        })
    };

    let mut meta = PdfMetadata {
        title: None,
        author: None,
        subject: None,
        creator: None,
        producer: None,
        pages: doc.get_pages().len() as i64,
    };

    if let Ok(info_ref) = doc.trailer.get(b"Info") {
        if let Ok(info_obj) = doc.dereference(info_ref) {
            if let Ok(dict) = info_obj.1.as_dict() {
                meta.title = extract_field(dict, b"Title");
                meta.author = extract_field(dict, b"Author");
                meta.subject = extract_field(dict, b"Subject");
                meta.creator = extract_field(dict, b"Creator");
                meta.producer = extract_field(dict, b"Producer");
            }
        }
    }

    if let Some(bounds) = bounds {
        check_pdf_pages("pdf/metadata", meta.pages as usize, bounds)?;
        check_pdf_text_output(
            "pdf/metadata",
            [
                meta.title.as_ref(),
                meta.author.as_ref(),
                meta.subject.as_ref(),
                meta.creator.as_ref(),
                meta.producer.as_ref(),
            ]
            .into_iter()
            .flatten()
            .map(String::len),
            bounds,
        )?;
    }
    Ok(meta)
}

/// Build `pdf/metadata`'s return map from the offload-safe intermediate.
fn metadata_to_value(m: PdfMetadata) -> Value {
    let mut map = BTreeMap::new();
    if let Some(v) = m.title {
        map.insert(Value::keyword("title"), Value::string(&v));
    }
    if let Some(v) = m.author {
        map.insert(Value::keyword("author"), Value::string(&v));
    }
    if let Some(v) = m.subject {
        map.insert(Value::keyword("subject"), Value::string(&v));
    }
    if let Some(v) = m.creator {
        map.insert(Value::keyword("creator"), Value::string(&v));
    }
    if let Some(v) = m.producer {
        map.insert(Value::keyword("producer"), Value::string(&v));
    }
    map.insert(Value::keyword("pages"), Value::int(m.pages));
    Value::map(map)
}

pub fn register(env: &sema_core::Env, sandbox: &sema_core::Sandbox) {
    crate::register_runtime_fn_path_gated(
        env,
        sandbox,
        Caps::FS_READ,
        "pdf/extract-text",
        &[0],
        |args| {
            check_arity!(args, "pdf/extract-text", 1);
            let path = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?
                .to_string();

            if sema_core::in_runtime_quantum() {
                let input = open_pdf_runtime_input("pdf/extract-text", &path, PDF_RUNTIME_BOUNDS)?;
                return crate::io::quarantined_compute(
                    "pdf/extract-text",
                    Value::string_owned,
                    move || extract_text_work(&path, Some(input)).map_err(|e| e.to_string()),
                );
            }
            Ok(NativeOutcome::Return(Value::string(&extract_text_work(
                &path, None,
            )?)))
        },
    );

    crate::register_runtime_fn_path_gated(
        env,
        sandbox,
        Caps::FS_READ,
        "pdf/extract-text-pages",
        &[0],
        |args| {
            check_arity!(args, "pdf/extract-text-pages", 1);
            let path = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?
                .to_string();

            if sema_core::in_runtime_quantum() {
                let input =
                    open_pdf_runtime_input("pdf/extract-text-pages", &path, PDF_RUNTIME_BOUNDS)?;
                return crate::io::quarantined_compute(
                    "pdf/extract-text-pages",
                    pages_to_value,
                    move || extract_text_pages_work(&path, Some(input)).map_err(|e| e.to_string()),
                );
            }
            Ok(NativeOutcome::Return(pages_to_value(
                extract_text_pages_work(&path, None)?,
            )))
        },
    );

    crate::register_runtime_fn_path_gated(
        env,
        sandbox,
        Caps::FS_READ,
        "pdf/page-count",
        &[0],
        |args| {
            check_arity!(args, "pdf/page-count", 1);
            let path = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?
                .to_string();

            if sema_core::in_runtime_quantum() {
                let bounds = PDF_RUNTIME_BOUNDS;
                let input = open_pdf_runtime_input("pdf/page-count", &path, bounds)?;
                return crate::io::quarantined_compute("pdf/page-count", Value::int, move || {
                    page_count_work(&path, Some(input)).map_err(|e| e.to_string())
                });
            }
            Ok(NativeOutcome::Return(Value::int(page_count_work(
                &path, None,
            )?)))
        },
    );

    crate::register_runtime_fn_path_gated(
        env,
        sandbox,
        Caps::FS_READ,
        "pdf/metadata",
        &[0],
        |args| {
            check_arity!(args, "pdf/metadata", 1);
            let path = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?
                .to_string();

            if sema_core::in_runtime_quantum() {
                let bounds = PDF_RUNTIME_BOUNDS;
                let input = open_pdf_runtime_input("pdf/metadata", &path, bounds)?;
                return crate::io::quarantined_compute(
                    "pdf/metadata",
                    metadata_to_value,
                    move || metadata_work(&path, Some(input)).map_err(|e| e.to_string()),
                );
            }
            Ok(NativeOutcome::Return(metadata_to_value(metadata_work(
                &path, None,
            )?)))
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_input_rejects_non_regular_files() {
        let path = std::env::temp_dir().join(format!("sema-pdf-special-{}", std::process::id()));
        std::fs::create_dir_all(&path).expect("create special-input directory");
        let error = open_pdf_runtime_input(
            "pdf/page-count",
            path.to_str().expect("utf-8 temp path"),
            PDF_RUNTIME_BOUNDS,
        )
        .expect_err("directory must not enter the worker queue");
        let _ = std::fs::remove_dir(&path);
        assert!(error.to_string().contains("regular file"));
    }

    #[cfg(unix)]
    #[test]
    fn runtime_input_rejects_fifo_without_blocking() {
        use std::os::unix::ffi::OsStrExt as _;

        let path = std::env::temp_dir().join(format!("sema-pdf-fifo-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let path_c = std::ffi::CString::new(path.as_os_str().as_bytes()).expect("FIFO path");
        assert_eq!(unsafe { libc::mkfifo(path_c.as_ptr(), 0o600) }, 0);
        let error = open_pdf_runtime_input(
            "pdf/page-count",
            path.to_str().expect("utf-8 temp path"),
            PDF_RUNTIME_BOUNDS,
        )
        .expect_err("FIFO must not enter the worker queue");
        let _ = std::fs::remove_file(&path);
        assert!(error.to_string().contains("regular file"));
    }

    #[test]
    fn quarantine_limit_accepts_boundary_and_rejects_one_over() {
        assert!(check_pdf_limit("pdf/extract-text", "pages", 8, 8).is_ok());
        let error = check_pdf_limit("pdf/extract-text", "pages", 9, 8)
            .expect_err("one page over the captured limit must fail");
        assert!(error.to_string().contains("9"));
        assert!(error.to_string().contains("8"));

        let bounds = PdfBounds {
            input_bytes: 8,
            output_bytes: 8,
            pages: 1,
        };
        assert!(check_pdf_text_output("pdf/extract-text", [8], bounds).is_ok());
        assert!(check_pdf_text_output("pdf/extract-text", [9], bounds).is_err());
        assert!(check_pdf_pages("pdf/extract-text", 1, bounds).is_ok());
        assert!(check_pdf_pages("pdf/extract-text", 2, bounds).is_err());
    }
}
