//! PDF builtins (`pdf/*`).
//!
//! Every builtin reads a whole file plus runs a CPU-bound parse (page
//! extraction, xref/trailer walk), so — like `archive.rs` — each one's actual
//! work lives in a plain `*_work` function returning `Result<T, SemaError>`.
//! The registered native calls it directly at top level, or — inside
//! `async/spawn` (`in_async_context()`) — offloads it through `fs_offload`
//! (`io.rs`) so a large PDF doesn't block the VM thread (and every sibling
//! task) for the parse's whole duration. See `archive.rs`'s module doc for
//! the full rationale (never move `SemaError`/`Value` across the thread
//! boundary — only the plain `Send` result does).

use std::collections::BTreeMap;

use sema_core::{check_arity, in_async_context, Caps, SemaError, Value};

/// `pdf/extract-text`'s actual work. Shared verbatim by the sync and
/// offloaded-async paths.
fn extract_text_work(path: &str) -> Result<String, SemaError> {
    let bytes =
        std::fs::read(path).map_err(|e| SemaError::Io(format!("pdf/extract-text {path}: {e}")))?;
    pdf_extract::extract_text_from_mem(&bytes)
        .map_err(|e| SemaError::eval(format!("pdf/extract-text {path}: {e}")))
}

/// `pdf/extract-text-pages`'s actual work. Shared verbatim by the sync and
/// offloaded-async paths.
fn extract_text_pages_work(path: &str) -> Result<Vec<String>, SemaError> {
    pdf_extract::extract_text_by_pages(path)
        .map_err(|e| SemaError::eval(format!("pdf/extract-text-pages {path}: {e}")))
}

/// `pdf/page-count`'s actual work. Shared verbatim by the sync and
/// offloaded-async paths.
fn page_count_work(path: &str) -> Result<i64, SemaError> {
    let doc = lopdf::Document::load(path)
        .map_err(|e| SemaError::Io(format!("pdf/page-count {path}: {e}")))?;
    Ok(doc.get_pages().len() as i64)
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

/// `pdf/metadata`'s actual work. Shared verbatim by the sync and
/// offloaded-async paths.
fn metadata_work(path: &str) -> Result<PdfMetadata, SemaError> {
    let doc = lopdf::Document::load(path)
        .map_err(|e| SemaError::Io(format!("pdf/metadata {path}: {e}")))?;

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
    crate::register_fn_path_gated(
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

            if in_async_context() {
                return crate::io::fs_offload(
                    move || extract_text_work(&path).map_err(|e| e.to_string()),
                    Value::string_owned,
                );
            }
            let text = extract_text_work(&path)?;
            Ok(Value::string(&text))
        },
    );

    crate::register_fn_path_gated(
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

            if in_async_context() {
                return crate::io::fs_offload(
                    move || extract_text_pages_work(&path).map_err(|e| e.to_string()),
                    |pages: Vec<String>| {
                        Value::list(pages.iter().map(|s| Value::string(s)).collect())
                    },
                );
            }
            let pages = extract_text_pages_work(&path)?;
            let values: Vec<Value> = pages.iter().map(|s| Value::string(s)).collect();
            Ok(Value::list(values))
        },
    );

    crate::register_fn_path_gated(
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

            if in_async_context() {
                return crate::io::fs_offload(
                    move || page_count_work(&path).map_err(|e| e.to_string()),
                    Value::int,
                );
            }
            let count = page_count_work(&path)?;
            Ok(Value::int(count))
        },
    );

    crate::register_fn_path_gated(env, sandbox, Caps::FS_READ, "pdf/metadata", &[0], |args| {
        check_arity!(args, "pdf/metadata", 1);
        let path = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?
            .to_string();

        if in_async_context() {
            return crate::io::fs_offload(
                move || metadata_work(&path).map_err(|e| e.to_string()),
                metadata_to_value,
            );
        }
        let meta = metadata_work(&path)?;
        Ok(metadata_to_value(meta))
    });
}
