use sema_core::{check_arity, ArgsExt, OptionsExt, SemaError, Value};

use crate::register_fn;

pub fn register(env: &sema_core::Env) {
    // (text/chunk text) or (text/chunk text {:size 1000 :overlap 200})
    register_fn(env, "text/chunk", |args| {
        check_arity!(args, "text/chunk", 1..=2);
        let text = args.str_at(0, "text/chunk")?;
        if text.is_empty() {
            return Ok(Value::list(vec![]));
        }

        let mut chunk_size: usize = 1000;
        let mut overlap: usize = 200;
        if let Some(opts) = args.get(1).and_then(|v| v.as_map_rc()) {
            if let Some(v) = opts.opt_int("size") {
                chunk_size = v.max(1) as usize;
            }
            if let Some(v) = opts.opt_int("overlap") {
                overlap = v.max(0) as usize;
            }
        }
        if overlap >= chunk_size {
            overlap = 0;
        }
        let chunks = recursive_chunk(text, chunk_size, overlap);
        Ok(Value::list(
            chunks.into_iter().map(|s| Value::string(&s)).collect(),
        ))
    });

    // (text/chunk-by-separator text separator)
    register_fn(env, "text/chunk-by-separator", |args| {
        check_arity!(args, "text/chunk-by-separator", 2);
        let text = args.str_at(0, "text/chunk-by-separator")?;
        let sep = args.str_at(1, "text/chunk-by-separator")?;
        if text.is_empty() {
            return Ok(Value::list(vec![]));
        }
        let chunks: Vec<Value> = text
            .split(sep)
            .filter(|s| !s.is_empty())
            .map(Value::string)
            .collect();
        Ok(Value::list(chunks))
    });

    // (text/split-sentences text)
    register_fn(env, "text/split-sentences", |args| {
        check_arity!(args, "text/split-sentences", 1);
        let text = args.str_at(0, "text/split-sentences")?;
        if text.is_empty() {
            return Ok(Value::list(vec![]));
        }
        let sentences = split_sentences(text);
        Ok(Value::list(
            sentences.into_iter().map(|s| Value::string(&s)).collect(),
        ))
    });

    // --- Task 5: Text Cleaning ---

    register_fn(env, "text/clean-whitespace", |args| {
        check_arity!(args, "text/clean-whitespace", 1);
        let text = args.str_at(0, "text/clean-whitespace")?;
        Ok(Value::string(
            &text.split_whitespace().collect::<Vec<_>>().join(" "),
        ))
    });

    register_fn(env, "text/strip-html", |args| {
        check_arity!(args, "text/strip-html", 1);
        let text = args.str_at(0, "text/strip-html")?;
        Ok(Value::string(&strip_html(text)))
    });

    register_fn(env, "text/truncate", |args| {
        check_arity!(args, "text/truncate", 2..=3);
        let text = args.str_at(0, "text/truncate")?;
        let max_len = args.int_at(1, "text/truncate")? as usize;
        let suffix = args
            .get(2)
            .and_then(|v| v.as_str())
            .unwrap_or("...")
            .to_string();
        let char_count = text.chars().count();
        if char_count <= max_len {
            return Ok(Value::string(text));
        }
        let suffix_len = suffix.chars().count();
        if max_len <= suffix_len {
            return Ok(Value::string(&suffix));
        }
        let take = max_len - suffix_len;
        let truncated: String = text.chars().take(take).collect();
        Ok(Value::string(&format!("{truncated}{suffix}")))
    });

    register_fn(env, "text/word-count", |args| {
        check_arity!(args, "text/word-count", 1);
        let text = args.str_at(0, "text/word-count")?;
        Ok(Value::int(text.split_whitespace().count() as i64))
    });

    register_fn(env, "text/trim-indent", |args| {
        check_arity!(args, "text/trim-indent", 1);
        let text = args.str_at(0, "text/trim-indent")?;
        Ok(Value::string(&trim_indent(text)))
    });

    // --- Task 6: Prompt Templates ---

    register_fn(env, "prompt/template", |args| {
        check_arity!(args, "prompt/template", 1);
        let text = args.str_at(0, "prompt/template")?;
        Ok(Value::string(text))
    });

    register_fn(env, "prompt/render", |args| {
        check_arity!(args, "prompt/render", 2);
        let template = args.str_at(0, "prompt/render")?;
        let vars = args.map_at(1, "prompt/render")?;
        Ok(Value::string(&sema_core::text_util::render_template(
            template, &vars,
        )))
    });

    // --- Task 15: Document Metadata ---

    register_fn(env, "document/create", |args| {
        check_arity!(args, "document/create", 2);
        let text = args.str_at(0, "document/create")?;
        let metadata = args.map_at(1, "document/create")?;
        let mut doc = std::collections::BTreeMap::new();
        doc.insert(Value::keyword("text"), Value::string(text));
        doc.insert(Value::keyword("metadata"), Value::map((*metadata).clone()));
        Ok(Value::map(doc))
    });

    register_fn(env, "document/text", |args| {
        check_arity!(args, "document/text", 1);
        let map = args[0]
            .as_map_rc()
            .ok_or_else(|| SemaError::type_error("map (document)", args[0].type_name()))?;
        map.opt("text")
            .ok_or_else(|| SemaError::eval("not a document: missing :text"))
    });

    register_fn(env, "document/metadata", |args| {
        check_arity!(args, "document/metadata", 1);
        let map = args[0]
            .as_map_rc()
            .ok_or_else(|| SemaError::type_error("map (document)", args[0].type_name()))?;
        map.opt("metadata")
            .ok_or_else(|| SemaError::eval("not a document: missing :metadata"))
    });

    register_fn(env, "document/chunk", |args| {
        check_arity!(args, "document/chunk", 1..=2);
        let doc = args[0]
            .as_map_rc()
            .ok_or_else(|| SemaError::type_error("map (document)", args[0].type_name()))?;
        let text = doc
            .opt_str("text")
            .ok_or_else(|| SemaError::eval("document/chunk: document missing :text"))?;
        let base_metadata = doc
            .opt_map("metadata")
            .map(|m| (*m).clone())
            .unwrap_or_default();

        let mut chunk_size: usize = 1000;
        let mut overlap: usize = 200;
        if let Some(opts) = args.get(1).and_then(|v| v.as_map_rc()) {
            if let Some(v) = opts.opt_int("size") {
                chunk_size = v.max(1) as usize;
            }
            if let Some(v) = opts.opt_int("overlap") {
                overlap = v.max(0) as usize;
            }
        }
        if overlap >= chunk_size {
            overlap = 0;
        }

        let chunks = recursive_chunk(&text, chunk_size, overlap);
        let total = chunks.len() as i64;
        let result: Vec<Value> = chunks
            .into_iter()
            .enumerate()
            .map(|(i, chunk_text)| {
                let mut meta = base_metadata.clone();
                meta.insert(Value::keyword("chunk-index"), Value::int(i as i64));
                meta.insert(Value::keyword("total-chunks"), Value::int(total));
                let mut doc_map = std::collections::BTreeMap::new();
                doc_map.insert(Value::keyword("text"), Value::string(&chunk_text));
                doc_map.insert(Value::keyword("metadata"), Value::map(meta));
                Value::map(doc_map)
            })
            .collect();

        Ok(Value::list(result))
    });

    // text/excerpt — extract a snippet around a match with omission markers
    register_fn(env, "text/excerpt", |args| {
        check_arity!(args, "text/excerpt", 2..=3);
        let text = args.str_at(0, "text/excerpt")?;
        let query = args.str_at(1, "text/excerpt")?;

        let mut radius: usize = 100;
        let mut omission = "...".to_string();
        if let Some(opts) = args.get(2).and_then(|v| v.as_map_rc()) {
            if let Some(v) = opts.opt_int("radius") {
                radius = v.max(0) as usize;
            }
            if let Some(v) = opts.opt_str("omission") {
                omission = v;
            }
        }

        let lower_text = text.to_lowercase();
        let lower_query = query.to_lowercase();
        match lower_text.find(&lower_query) {
            None => Ok(Value::nil()),
            Some(byte_idx) => {
                let chars: Vec<char> = text.chars().collect();
                let char_idx = text[..byte_idx].chars().count();
                let query_char_len = query.chars().count();

                let start = char_idx.saturating_sub(radius);
                let end = (char_idx + query_char_len + radius).min(chars.len());

                let snippet: String = chars[start..end].iter().collect();

                let mut result = String::new();
                if start > 0 {
                    result.push_str(&omission);
                }
                result.push_str(&snippet);
                if end < chars.len() {
                    result.push_str(&omission);
                }
                Ok(Value::string(&result))
            }
        }
    });

    // text/normalize-newlines — convert \r\n and \r to \n
    register_fn(env, "text/normalize-newlines", |args| {
        check_arity!(args, "text/normalize-newlines", 1);
        let text = args.str_at(0, "text/normalize-newlines")?;
        Ok(Value::string(
            &text.replace("\r\n", "\n").replace('\r', "\n"),
        ))
    });
}

// --- Chunking helpers ---

const SEPARATORS: &[&str] = &["\n\n", "\n", ". ", "! ", "? ", "; ", ", ", " "];

fn recursive_chunk(text: &str, max_size: usize, overlap: usize) -> Vec<String> {
    if text.len() <= max_size {
        return vec![text.to_string()];
    }
    for sep in SEPARATORS {
        let parts: Vec<&str> = text.split(sep).collect();
        if parts.len() > 1 {
            return merge_splits(&parts, sep, max_size, overlap);
        }
    }
    hard_chunk(text, max_size, overlap)
}

fn merge_splits(parts: &[&str], sep: &str, max_size: usize, overlap: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    for part in parts {
        let with_sep = if current.is_empty() {
            part.to_string()
        } else {
            format!("{}{}{}", current, sep, part)
        };
        if with_sep.len() <= max_size {
            current = with_sep;
        } else {
            if !current.is_empty() {
                chunks.push(current.clone());
            }
            if part.len() > max_size {
                chunks.extend(recursive_chunk(part, max_size, overlap));
                current = String::new();
            } else {
                current = part.to_string();
            }
        }
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    if overlap > 0 && chunks.len() > 1 {
        apply_overlap(&chunks, overlap)
    } else {
        chunks
    }
}

fn apply_overlap(chunks: &[String], overlap: usize) -> Vec<String> {
    let mut result = vec![chunks[0].clone()];
    for i in 1..chunks.len() {
        let prev = &chunks[i - 1];
        // `overlap` is a character count, so slice on a char boundary: take the
        // last `overlap` chars of `prev`. A byte-offset slice (`&prev[len-overlap..]`)
        // panics when the offset lands inside a multi-byte character.
        let ov = if overlap == 0 {
            ""
        } else {
            match prev.char_indices().rev().nth(overlap - 1) {
                Some((byte_idx, _)) => &prev[byte_idx..],
                None => prev.as_str(),
            }
        };
        result.push(format!("{}{}", ov, chunks[i]));
    }
    result
}

fn hard_chunk(text: &str, max_size: usize, overlap: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let step = if overlap < max_size {
        max_size - overlap
    } else {
        max_size
    };
    let mut i = 0;
    while i < chars.len() {
        let end = (i + max_size).min(chars.len());
        chunks.push(chars[i..end].iter().collect());
        i += step;
    }
    chunks
}

fn split_sentences(text: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut current = String::new();
    let chars: Vec<char> = text.chars().collect();
    for i in 0..chars.len() {
        current.push(chars[i]);
        if (chars[i] == '.' || chars[i] == '!' || chars[i] == '?')
            && (i + 1 >= chars.len() || chars[i + 1].is_whitespace())
        {
            let trimmed = current.trim().to_string();
            if !trimmed.is_empty() {
                sentences.push(trimmed);
            }
            current = String::new();
        }
    }
    let trimmed = current.trim().to_string();
    if !trimmed.is_empty() {
        sentences.push(trimmed);
    }
    sentences
}

// --- Text cleaning helpers ---

fn strip_html(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut in_tag = false;
    for ch in text.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => result.push(ch),
            _ => {}
        }
    }
    result
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ")
}

fn trim_indent(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    let lines: Vec<&str> = text.split('\n').collect();
    // Measure the common indent in CHARACTERS, not bytes. A byte count taken
    // from one line and sliced off another lands inside a character the moment
    // any line is indented with multi-byte whitespace — NBSP or an ideographic
    // space, both ordinary in text pasted from a browser or a word processor —
    // and `&line[min_indent..]` then aborts the process with "byte index is not
    // a char boundary", which `try`/`catch` cannot catch.
    let min_indent = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.chars().take_while(|c| c.is_whitespace()).count())
        .min()
        .unwrap_or(0);
    lines
        .iter()
        .map(|line| strip_leading_whitespace(line, min_indent))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Drop up to `count` leading whitespace CHARACTERS, stopping early at the
/// first non-whitespace one. Slices only on char boundaries by construction.
fn strip_leading_whitespace(line: &str, count: usize) -> &str {
    for (dropped, (offset, ch)) in line.char_indices().enumerate() {
        if dropped == count || !ch.is_whitespace() {
            return &line[offset..];
        }
    }
    // Every character was whitespace we were asked to drop.
    ""
}

// --- Template helpers ---
