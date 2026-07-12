//! Unified-diff generation and patch application builtins.
//!
//! `diff/*` functions are read-only (pure string transforms) and registered with
//! plain `register_fn`. `patch/apply-file` touches the filesystem and is gated
//! behind `Caps::FS_WRITE`.
//!
//! The unified-diff text produced by `diff/unified` (via the `similar` crate) is
//! the canonical interchange format: `diff/stat`, `diff/hunks`, `diff/parse`, and
//! `diff/apply` all consume that same textual shape, so a round-trip
//! (`diff/unified` then `diff/apply`) reconstructs `new` from `old`.
//!
//! `patch/apply-file`'s real work (read + apply + write) lives in
//! `patch_apply_file_work`, called directly at top level or — inside
//! `async/spawn` (`in_async_context()`) — offloaded through `fs_offload`
//! (`io.rs`) so applying a patch to a large file doesn't block the VM thread
//! (and every sibling task). See `archive.rs`'s module doc for the full
//! offload rationale.

use std::collections::BTreeMap;

use sema_core::{check_arity, SemaError, Value};
use similar::TextDiff;

use crate::register_fn;
#[cfg(not(target_arch = "wasm32"))]
use {crate::register_fn_gated, sema_core::Caps};

/// A single parsed hunk header `@@ -old_start,old_count +new_start,new_count @@`
/// together with the body lines that follow it.
struct Hunk {
    header: String,
    old_start: i64,
    old_count: i64,
    new_start: i64,
    new_count: i64,
    /// Body lines including their leading marker (' ', '+', '-', or '\\').
    lines: Vec<String>,
}

/// Parse a `@@ -l,s +l,s @@` header. The counts are optional in the unified
/// format (a missing count means 1). Returns `(old_start, old_count, new_start,
/// new_count, full_header_text)`.
fn parse_hunk_header(line: &str) -> Option<(i64, i64, i64, i64, String)> {
    // Grammar: "@@ -" old "+" new " @@" [trailing section heading]
    // where old/new are "start" or "start,count".
    let rest = line.strip_prefix("@@ ")?;
    let at = rest.find(" @@")?;
    let ranges = &rest[..at];
    let header_text = {
        // Keep the canonical "@@ -.. +.. @@" portion (drop any trailing context
        // heading the producer may append after the closing `@@`).
        let end = line.find(" @@").map(|i| i + 3).unwrap_or(line.len());
        line[..end].to_string()
    };
    let mut parts = ranges.split(' ');
    let old_part = parts.next()?.strip_prefix('-')?;
    let new_part = parts.next()?.strip_prefix('+')?;

    let parse_range = |p: &str| -> Option<(i64, i64)> {
        let (s, c) = if let Some((s, c)) = p.split_once(',') {
            (s.parse().ok()?, c.parse().ok()?)
        } else {
            (p.parse().ok()?, 1)
        };
        // Unified-diff line numbers are 1-based (0 only for empty side); a
        // negative start is malformed.
        if s < 0 || c < 0 {
            return None;
        }
        Some((s, c))
    };
    let (old_start, old_count) = parse_range(old_part)?;
    let (new_start, new_count) = parse_range(new_part)?;
    Some((old_start, old_count, new_start, new_count, header_text))
}

/// Parse all hunks out of a unified-diff string, ignoring `---`/`+++` file
/// headers and any `diff`/`index` preamble lines that appear before a hunk.
fn parse_hunks(patch: &str) -> Vec<Hunk> {
    let mut hunks: Vec<Hunk> = Vec::new();
    for line in patch.lines() {
        if line.starts_with("@@") {
            if let Some((old_start, old_count, new_start, new_count, header_text)) =
                parse_hunk_header(line)
            {
                hunks.push(Hunk {
                    header: header_text,
                    old_start,
                    old_count,
                    new_start,
                    new_count,
                    lines: Vec::new(),
                });
            }
        } else if let Some(h) = hunks.last_mut() {
            // Lines belonging to the current hunk: context (' '), additions ('+'),
            // deletions ('-'), or "\ No newline at end of file" ('\').
            if line.starts_with(' ')
                || line.starts_with('+')
                || line.starts_with('-')
                || line.starts_with('\\')
            {
                h.lines.push(line.to_string());
            } else {
                // A non-diff line ends the current hunk run (e.g. start of a new
                // file section's `diff --git`). Stop attaching to this hunk.
            }
        }
    }
    hunks
}

/// Build the Sema map representation of a single hunk.
fn hunk_to_value(h: &Hunk) -> Value {
    let mut m = BTreeMap::new();
    m.insert(Value::keyword("header"), Value::string(&h.header));
    m.insert(Value::keyword("old-start"), Value::int(h.old_start));
    m.insert(Value::keyword("old-count"), Value::int(h.old_count));
    m.insert(Value::keyword("new-start"), Value::int(h.new_start));
    m.insert(Value::keyword("new-count"), Value::int(h.new_count));
    let lines: Vec<Value> = h.lines.iter().map(|l| Value::string(l)).collect();
    m.insert(Value::keyword("lines"), Value::list(lines));
    Value::map(m)
}

/// Apply a parsed set of hunks to `content`, returning the patched string.
///
/// Strategy: operate on `\n`-split lines. For each hunk, verify the context and
/// deleted lines match what the diff expects at the recorded 1-based position
/// (with a small search window to tolerate drift), then splice the new lines in.
fn apply_hunks(content: &str, hunks: &[Hunk]) -> Result<String, SemaError> {
    // Preserve whether the original ended with a trailing newline so the result
    // round-trips. `lines()` drops it; we re-add on join unless told otherwise.
    let had_trailing_newline = content.ends_with('\n');
    let mut lines: Vec<String> = if content.is_empty() {
        Vec::new()
    } else {
        content.split('\n').map(|s| s.to_string()).collect()
    };
    // `split('\n')` on text ending in '\n' yields a trailing empty element; drop
    // it so indices line up with diff line numbers, and re-add the newline later.
    if had_trailing_newline {
        lines.pop();
    }

    // Cumulative offset between original line numbers and the mutated buffer.
    let mut offset: i64 = 0;

    for h in hunks {
        // Expected old lines (context + deletions) in original order.
        let mut expected: Vec<&str> = Vec::new();
        let mut replacement: Vec<String> = Vec::new();
        for raw in &h.lines {
            let (marker, rest) = raw.split_at(1);
            match marker {
                " " => {
                    expected.push(rest);
                    replacement.push(rest.to_string());
                }
                "-" => {
                    expected.push(rest);
                }
                "+" => {
                    replacement.push(rest.to_string());
                }
                "\\" => {
                    // "\ No newline at end of file" — metadata, not content.
                }
                _ => {
                    return Err(SemaError::eval(format!(
                        "diff/apply: malformed hunk line: {raw:?}"
                    )));
                }
            }
        }

        // Locate the splice point. The diff records a 1-based old_start; adjust by
        // the running offset. For a pure-insertion hunk (old_count == 0) the new
        // lines go *after* old_start.
        let nominal = if h.old_count == 0 {
            // Insertion after line old_start (0 means before the first line).
            h.old_start + offset
        } else {
            h.old_start - 1 + offset
        };

        let matches_at = |start: i64| -> bool {
            if start < 0 {
                return false;
            }
            let start = start as usize;
            if start + expected.len() > lines.len() {
                return false;
            }
            expected
                .iter()
                .enumerate()
                .all(|(i, e)| lines[start + i] == *e)
        };

        let splice_at: usize = if expected.is_empty() {
            // Pure insertion: clamp the nominal position into range.
            nominal.max(0).min(lines.len() as i64) as usize
        } else {
            // Try the nominal position first, then search a SMALL window to
            // tolerate minor drift from earlier hunks. The window is bounded
            // (patch-style fuzz) so we don't latch onto a far-away coincidental
            // context match in repetitive content (blank lines, lone `}`), which
            // would silently splice the hunk in the wrong place.
            const MAX_DRIFT: i64 = 3;
            let mut found: Option<usize> = None;
            if matches_at(nominal) {
                found = Some(nominal.max(0) as usize);
            } else {
                let mut delta = 1;
                while delta <= MAX_DRIFT {
                    if matches_at(nominal - delta) {
                        found = Some((nominal - delta) as usize);
                        break;
                    }
                    if matches_at(nominal + delta) {
                        found = Some((nominal + delta) as usize);
                        break;
                    }
                    delta += 1;
                }
            }
            match found {
                Some(idx) => idx,
                None => {
                    return Err(SemaError::eval(format!(
                        "diff/apply: hunk does not apply (context mismatch near old line {})",
                        h.old_start
                    )))
                }
            }
        };

        // If the hunk applied at a drifted position (earlier edits shifted the
        // buffer by more than their length delta accounted for), fold that drift
        // into `offset` so later hunks — whose old_start is still in original
        // coordinates — stay aligned instead of compounding the misplacement.
        if !expected.is_empty() {
            offset += splice_at as i64 - nominal;
        }

        // Splice: remove the matched old lines, insert the replacement.
        let remove_count = expected.len();
        lines.splice(
            splice_at..splice_at + remove_count,
            replacement.iter().cloned(),
        );

        // Track how the buffer length changed for subsequent hunks.
        offset += h.new_count - h.old_count;
    }

    let mut result = lines.join("\n");
    if had_trailing_newline && !result.is_empty() {
        result.push('\n');
    } else if had_trailing_newline && result.is_empty() {
        // Original was just newlines that got fully consumed; keep a newline only
        // if some content remains. Empty result stays empty.
    }
    Ok(result)
}

#[cfg_attr(target_arch = "wasm32", allow(unused_variables))]
pub fn register(env: &sema_core::Env, sandbox: &sema_core::Sandbox) {
    // (diff/unified old new [context]) -> unified-diff string
    register_fn(env, "diff/unified", |args| {
        check_arity!(args, "diff/unified", 2..=3);
        let old = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let new = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        let context: usize = if args.len() == 3 {
            let n = args[2]
                .as_int()
                .ok_or_else(|| SemaError::type_error("int", args[2].type_name()))?;
            if n < 0 {
                return Err(SemaError::eval("diff/unified: context must be >= 0"));
            }
            n as usize
        } else {
            3
        };
        let diff = TextDiff::from_lines(old, new);
        let text = diff
            .unified_diff()
            .context_radius(context)
            .header("old", "new")
            .to_string();
        Ok(Value::string(&text))
    });

    // (diff/stat patch) -> {:added <int> :removed <int> :hunks <int>}
    register_fn(env, "diff/stat", |args| {
        check_arity!(args, "diff/stat", 1);
        let patch = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let mut added = 0i64;
        let mut removed = 0i64;
        let mut hunks = 0i64;
        // Track hunk-body state so a content line that happens to start with
        // `---`/`+++` (e.g. a removed `--` line renders as `---`) is counted,
        // not mistaken for a file header. File headers only appear in the
        // preamble before a hunk; `diff ` (git) starts a new file's preamble.
        let mut in_hunk = false;
        for line in patch.lines() {
            if line.starts_with("diff ") {
                in_hunk = false;
                continue;
            }
            if line.starts_with("@@") {
                hunks += 1;
                in_hunk = true;
                continue;
            }
            if !in_hunk {
                continue; // preamble, incl. ---/+++ file headers and index lines
            }
            match line.as_bytes().first() {
                Some(b'+') => added += 1,
                Some(b'-') => removed += 1,
                _ => {} // context (space), "\ No newline", blank
            }
        }
        let mut m = BTreeMap::new();
        m.insert(Value::keyword("added"), Value::int(added));
        m.insert(Value::keyword("removed"), Value::int(removed));
        m.insert(Value::keyword("hunks"), Value::int(hunks));
        Ok(Value::map(m))
    });

    // (diff/hunks patch) -> list of hunk maps
    register_fn(env, "diff/hunks", |args| {
        check_arity!(args, "diff/hunks", 1);
        let patch = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let hunks = parse_hunks(patch);
        let values: Vec<Value> = hunks.iter().map(hunk_to_value).collect();
        Ok(Value::list(values))
    });

    // (diff/parse patch) -> {:files [ {:old-path :new-path :hunks [...]} ... ]}
    register_fn(env, "diff/parse", |args| {
        check_arity!(args, "diff/parse", 1);
        let patch = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;

        // Walk the patch line-by-line, opening a new file section on each `---`
        // header and attaching subsequent hunks to it. Hunks before any file
        // header (a bare hunk-only patch) attach to a single anonymous file.
        struct FileSection {
            old_path: Option<String>,
            new_path: Option<String>,
            hunks: Vec<Hunk>,
        }
        let mut files: Vec<FileSection> = Vec::new();

        let strip_path = |s: &str| -> String {
            // Drop a leading "a/" or "b/" prefix and any trailing tab-delimited
            // timestamp the producer may append.
            let s = s.split('\t').next().unwrap_or(s);
            s.to_string()
        };

        for line in patch.lines() {
            if let Some(rest) = line.strip_prefix("--- ") {
                files.push(FileSection {
                    old_path: Some(strip_path(rest)),
                    new_path: None,
                    hunks: Vec::new(),
                });
            } else if let Some(rest) = line.strip_prefix("+++ ") {
                if let Some(f) = files.last_mut() {
                    f.new_path = Some(strip_path(rest));
                } else {
                    files.push(FileSection {
                        old_path: None,
                        new_path: Some(strip_path(rest)),
                        hunks: Vec::new(),
                    });
                }
            } else if line.starts_with("@@") {
                if let Some((old_start, old_count, new_start, new_count, header_text)) =
                    parse_hunk_header(line)
                {
                    if files.is_empty() {
                        files.push(FileSection {
                            old_path: None,
                            new_path: None,
                            hunks: Vec::new(),
                        });
                    }
                    files.last_mut().unwrap().hunks.push(Hunk {
                        header: header_text,
                        old_start,
                        old_count,
                        new_start,
                        new_count,
                        lines: Vec::new(),
                    });
                }
            } else if line.starts_with(' ')
                || line.starts_with('+')
                || line.starts_with('-')
                || line.starts_with('\\')
            {
                if let Some(f) = files.last_mut() {
                    if let Some(h) = f.hunks.last_mut() {
                        h.lines.push(line.to_string());
                    }
                }
            }
        }

        let file_values: Vec<Value> = files
            .iter()
            .map(|f| {
                let mut fm = BTreeMap::new();
                fm.insert(
                    Value::keyword("old-path"),
                    f.old_path
                        .as_deref()
                        .map(Value::string)
                        .unwrap_or_else(Value::nil),
                );
                fm.insert(
                    Value::keyword("new-path"),
                    f.new_path
                        .as_deref()
                        .map(Value::string)
                        .unwrap_or_else(Value::nil),
                );
                let hv: Vec<Value> = f.hunks.iter().map(hunk_to_value).collect();
                fm.insert(Value::keyword("hunks"), Value::list(hv));
                Value::map(fm)
            })
            .collect();

        let mut m = BTreeMap::new();
        m.insert(Value::keyword("files"), Value::list(file_values));
        Ok(Value::map(m))
    });

    // (diff/apply content patch) -> patched string (errors if a hunk doesn't apply)
    register_fn(env, "diff/apply", |args| {
        check_arity!(args, "diff/apply", 2);
        let content = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let patch = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        let hunks = parse_hunks(patch);
        let patched = apply_hunks(content, &hunks)?;
        Ok(Value::string(&patched))
    });

    // (patch/apply-file path patch) -> number of hunks applied (int)
    // Touches the real filesystem (not the VFS), so it's native-only.
    #[cfg(not(target_arch = "wasm32"))]
    register_fn_gated(env, sandbox, Caps::FS_WRITE, "patch/apply-file", |args| {
        check_arity!(args, "patch/apply-file", 2);
        let path = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?
            .to_string();
        let patch = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?
            .to_string();

        if sema_core::in_async_context() {
            return crate::io::fs_offload(
                move || patch_apply_file_work(&path, &patch).map_err(|e| e.to_string()),
                Value::int,
            );
        }
        let count = patch_apply_file_work(&path, &patch)?;
        Ok(Value::int(count))
    });
}

/// `patch/apply-file`'s actual work: read `path`, apply `patch`'s hunks, write
/// the patched content back, returning the hunk count. Shared verbatim by the
/// sync and offloaded-async paths (see `archive.rs`'s module doc for the
/// offload rationale — `SemaError` never crosses the thread boundary, only
/// its `.to_string()` rendering does).
#[cfg(not(target_arch = "wasm32"))]
fn patch_apply_file_work(path: &str, patch: &str) -> Result<i64, SemaError> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| SemaError::Io(format!("patch/apply-file {path}: {e}")))?;
    let hunks = parse_hunks(patch);
    let count = hunks.len() as i64;
    let patched = apply_hunks(&content, &hunks)?;
    std::fs::write(path, patched)
        .map_err(|e| SemaError::Io(format!("patch/apply-file {path}: {e}")))?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sema_core::{Env, Sandbox};

    fn make_env() -> Env {
        let env = Env::new();
        register(&env, &Sandbox::allow_all());
        env
    }

    fn try_call(env: &Env, name: &str, args: &[Value]) -> Result<Value, SemaError> {
        let f = env.get_str(name).expect("fn registered");
        let nf = f.as_native_fn_ref().expect("native fn");
        let ctx = sema_core::EvalContext::default();
        (nf.func)(&ctx, args)
    }

    fn call(env: &Env, name: &str, args: &[Value]) -> Value {
        try_call(env, name, args).expect("call ok")
    }

    fn map_get(m: &Value, key: &str) -> Value {
        let bt = m.as_map_ref().expect("map");
        bt.get(&Value::keyword(key))
            .cloned()
            .unwrap_or(Value::nil())
    }

    #[test]
    fn unified_then_apply_round_trips() {
        let env = make_env();
        let old = "a\nb\nc\n";
        let new = "a\nc\nd\n";
        let patch = call(
            &env,
            "diff/unified",
            &[Value::string(old), Value::string(new)],
        );
        let patch_s = patch.as_str().unwrap().to_string();
        assert!(patch_s.contains("@@"));
        let applied = call(
            &env,
            "diff/apply",
            &[Value::string(old), Value::string(patch_s.as_str())],
        );
        assert_eq!(applied.as_str().unwrap(), new);
    }

    #[test]
    fn round_trip_insertion_only() {
        let env = make_env();
        let old = "line1\nline2\n";
        let new = "line1\ninserted\nline2\n";
        let patch = call(
            &env,
            "diff/unified",
            &[Value::string(old), Value::string(new)],
        );
        let applied = call(&env, "diff/apply", &[Value::string(old), patch.clone()]);
        assert_eq!(applied.as_str().unwrap(), new);
    }

    #[test]
    fn round_trip_deletion_only() {
        let env = make_env();
        let old = "x\ny\nz\n";
        let new = "x\nz\n";
        let patch = call(
            &env,
            "diff/unified",
            &[Value::string(old), Value::string(new)],
        );
        let applied = call(&env, "diff/apply", &[Value::string(old), patch.clone()]);
        assert_eq!(applied.as_str().unwrap(), new);
    }

    #[test]
    fn stat_counts_match_known_patch() {
        let env = make_env();
        let patch = "--- old\n+++ new\n@@ -1,3 +1,3 @@\n a\n-b\n+B\n c\n";
        let stat = call(&env, "diff/stat", &[Value::string(patch)]);
        assert_eq!(map_get(&stat, "added").as_int(), Some(1));
        assert_eq!(map_get(&stat, "removed").as_int(), Some(1));
        assert_eq!(map_get(&stat, "hunks").as_int(), Some(1));
    }

    #[test]
    fn hunks_parses_header_and_lines() {
        let env = make_env();
        let patch = "--- old\n+++ new\n@@ -1,3 +1,3 @@\n a\n-b\n+B\n c\n";
        let hunks = call(&env, "diff/hunks", &[Value::string(patch)]);
        let list = hunks.as_list().unwrap();
        assert_eq!(list.len(), 1);
        let h = &list[0];
        assert_eq!(map_get(h, "old-start").as_int(), Some(1));
        assert_eq!(map_get(h, "old-count").as_int(), Some(3));
        assert_eq!(map_get(h, "new-start").as_int(), Some(1));
        assert_eq!(map_get(h, "new-count").as_int(), Some(3));
        let header = map_get(h, "header");
        assert_eq!(header.as_str(), Some("@@ -1,3 +1,3 @@"));
        let lines = map_get(h, "lines");
        assert_eq!(lines.as_list().unwrap().len(), 4);
    }

    #[test]
    fn parse_extracts_file_paths() {
        let env = make_env();
        let patch = "--- a/foo.txt\n+++ b/foo.txt\n@@ -1 +1 @@\n-old\n+new\n";
        let parsed = call(&env, "diff/parse", &[Value::string(patch)]);
        let files = map_get(&parsed, "files");
        let list = files.as_list().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(map_get(&list[0], "old-path").as_str(), Some("a/foo.txt"));
        assert_eq!(map_get(&list[0], "new-path").as_str(), Some("b/foo.txt"));
        assert_eq!(map_get(&list[0], "hunks").as_list().unwrap().len(), 1);
    }

    #[test]
    fn apply_fails_on_context_mismatch() {
        let env = make_env();
        // Patch expects "b" at line 2 but content has "X".
        let content = "a\nX\nc\n";
        let patch = "--- old\n+++ new\n@@ -1,3 +1,3 @@\n a\n-b\n+B\n c\n";
        let r = try_call(
            &env,
            "diff/apply",
            &[Value::string(content), Value::string(patch)],
        );
        assert!(r.is_err());
    }

    #[test]
    fn apply_no_trailing_newline() {
        let env = make_env();
        let old = "a\nb\nc";
        let new = "a\nB\nc";
        let patch = call(
            &env,
            "diff/unified",
            &[Value::string(old), Value::string(new)],
        );
        let applied = call(&env, "diff/apply", &[Value::string(old), patch.clone()]);
        assert_eq!(applied.as_str().unwrap(), new);
    }
}
