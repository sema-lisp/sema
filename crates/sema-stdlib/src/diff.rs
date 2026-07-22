//! Unified-diff generation and patch application builtins.
//!
//! The unified-diff text produced by `diff/unified` (via the `similar` crate) is
//! the canonical interchange format: `diff/stat`, `diff/hunks`, `diff/parse`, and
//! `diff/apply` all consume that same textual shape, so a round-trip
//! (`diff/unified` then `diff/apply`) reconstructs `new` from `old`.
//!
//! **Bounded / offloaded CPU (B8 R03 split).** `diff/unified` runs a super-linear
//! LCS (the `similar` diff), so during a runtime quantum (`in_runtime_quantum()`)
//! it captures a per-input byte cap BEFORE dispatch and offloads the diff onto the
//! I/O pool through `quarantined_compute` (`io.rs`) — the LCS runs over an owned
//! `String` snapshot (`Send`) on a worker, and the resulting diff string is
//! decoded back into a `Value` on the VM thread. `diff/stat`, `diff/hunks`,
//! `diff/parse`, and `diff/apply` are O(input) line walks, so they stay
//! SYNCHRONOUS but are still capped by a pre-dispatch input-byte and hunk-count
//! bound inside a quantum (an explicit synchronous split, not a fake async wrap):
//! bounded input ⇒ bounded VM-thread CPU. A direct native call outside the
//! cooperative runtime (e.g. the stdlib's own unit-test harness) keeps the
//! uncapped synchronous shape.
//!
//! `patch/apply-file` is the separate filesystem surface, gated behind
//! `Caps::FS_WRITE`. Its real work lives in `patch_apply_file_work`. A direct
//! native call outside the scheduler runs it synchronously; a runtime quantum
//! offloads it through `quarantined_compute` (`io.rs`). The runtime path captures
//! patch-byte, target-byte, output-byte, and hunk-count caps before dispatch,
//! then rechecks the target and output on the worker. Cancellation discards the
//! eventual result; it does not interrupt an already-running worker.

use std::collections::BTreeMap;

use sema_core::{check_arity, SemaError, Value};
use similar::TextDiff;

use crate::register_fn;
#[cfg(not(target_arch = "wasm32"))]
use std::cell::Cell;
#[cfg(not(target_arch = "wasm32"))]
use {
    crate::{register_runtime_fn, register_runtime_fn_gated},
    sema_core::runtime::NativeOutcome,
    sema_core::Caps,
};

/// Per-input byte cap for `diff/*` under a runtime quantum. `diff/unified`'s LCS
/// is super-linear, so the cap bounds both the offloaded worker's cost and the
/// synchronous line-walk ops' VM-thread cost. 64 MiB is far above any real diff.
#[cfg(not(target_arch = "wasm32"))]
const DIFF_INPUT_BYTE_CAP: u64 = 64 * 1024 * 1024;
/// Hunk-count cap for the synchronous patch-consuming ops under a quantum.
/// Shared ceiling with `patch/apply-file`.
#[cfg(not(target_arch = "wasm32"))]
const DIFF_HUNK_CAP: usize = PATCH_HUNK_CAP;

#[cfg(not(target_arch = "wasm32"))]
thread_local! {
    /// Optional per-call input-byte cap override (lowered, never raised above the
    /// hard ceiling). Read on the VM thread pre-dispatch; mirrors
    /// `git::GIT_MAX_OUTPUT_OVERRIDE`. `None` uses the module ceiling. The seam
    /// the regression suite drives to exercise the cap boundary without a
    /// multi-megabyte input string.
    static DIFF_INPUT_BYTE_CAP_OVERRIDE: Cell<Option<u64>> = const { Cell::new(None) };
}

/// The effective per-input byte cap for the current call: the module ceiling,
/// lowered by any per-call override (never raised above it).
#[cfg(not(target_arch = "wasm32"))]
fn effective_diff_input_byte_cap() -> u64 {
    DIFF_INPUT_BYTE_CAP_OVERRIDE
        .with(Cell::get)
        .map_or(DIFF_INPUT_BYTE_CAP, |over| over.min(DIFF_INPUT_BYTE_CAP))
}

/// Lower the per-input byte cap (clamped to the hard ceiling) for subsequent
/// `diff/*` calls on this thread, or clear the override with `None`. Test seam,
/// mirroring `set_git_max_output_bytes_override`.
#[cfg(not(target_arch = "wasm32"))]
pub fn set_diff_input_byte_cap_override(bytes: Option<u64>) {
    DIFF_INPUT_BYTE_CAP_OVERRIDE.with(|cell| cell.set(bytes));
}

#[cfg(not(target_arch = "wasm32"))]
fn check_diff_limit(op: &str, dimension: &str, actual: u64, limit: u64) -> Result<(), SemaError> {
    if actual > limit {
        return Err(SemaError::eval(format!(
            "{op}: {dimension} {actual} exceeds the quarantined limit {limit}"
        ))
        .with_hint("reduce or split the diff input"));
    }
    Ok(())
}

/// Pre-dispatch caps for a patch-consuming synchronous op (`diff/stat`,
/// `diff/hunks`, `diff/parse`). Enforced ONLY inside a runtime quantum so the
/// synchronous line walk that follows is bounded VM-thread CPU; a direct native
/// call keeps the uncapped shape. The `actual`/`limit` byte check reads
/// `patch.len()` — no snapshot is taken, so an over-cap input is rejected without
/// any excess allocation.
#[cfg(not(target_arch = "wasm32"))]
fn check_diff_patch_caps(op: &str, patch: &str) -> Result<(), SemaError> {
    if sema_core::in_runtime_quantum() {
        let cap = effective_diff_input_byte_cap();
        check_diff_limit(op, "input bytes", patch.len() as u64, cap)?;
        check_diff_limit(op, "hunks", patch_hunk_count(patch) as u64, DIFF_HUNK_CAP as u64)?;
    }
    Ok(())
}

/// Pre-dispatch caps for `diff/apply` (content + patch inputs), enforced only
/// inside a runtime quantum. See [`check_diff_patch_caps`].
#[cfg(not(target_arch = "wasm32"))]
fn check_diff_apply_caps(content: &str, patch: &str) -> Result<(), SemaError> {
    if sema_core::in_runtime_quantum() {
        let cap = effective_diff_input_byte_cap();
        check_diff_limit("diff/apply", "content bytes", content.len() as u64, cap)?;
        check_diff_limit("diff/apply", "patch bytes", patch.len() as u64, cap)?;
        check_diff_limit(
            "diff/apply",
            "hunks",
            patch_hunk_count(patch) as u64,
            DIFF_HUNK_CAP as u64,
        )?;
    }
    Ok(())
}

/// Decode an offloaded `diff/unified` result (an owned `String`) into a `Value`
/// on the VM thread. Non-capturing `fn` for `quarantined_compute`'s decoder slot.
#[cfg(not(target_arch = "wasm32"))]
fn diff_string_to_value(s: String) -> Value {
    Value::string(&s)
}

/// Parse `diff/unified`'s optional context-radius argument (default 3).
fn diff_context_arg(args: &[Value]) -> Result<usize, SemaError> {
    if args.len() == 3 {
        let n = args[2]
            .as_int()
            .ok_or_else(|| SemaError::type_error("int", args[2].type_name()))?;
        if n < 0 {
            return Err(SemaError::eval("diff/unified: context must be >= 0"));
        }
        Ok(n as usize)
    } else {
        Ok(3)
    }
}

/// Produce the unified-diff text for `old` → `new` at the given context radius.
/// Shared by the offloaded and synchronous `diff/unified` paths.
fn diff_unified_text(old: &str, new: &str, context: usize) -> String {
    TextDiff::from_lines(old, new)
        .unified_diff()
        .context_radius(context)
        .header("old", "new")
        .to_string()
}

#[cfg(not(target_arch = "wasm32"))]
const PATCH_INPUT_BYTE_CAP: u64 = 64 * 1024 * 1024;
#[cfg(not(target_arch = "wasm32"))]
const PATCH_TARGET_BYTE_CAP: u64 = 256 * 1024 * 1024;
#[cfg(not(target_arch = "wasm32"))]
const PATCH_OUTPUT_BYTE_CAP: u64 = 256 * 1024 * 1024;
#[cfg(not(target_arch = "wasm32"))]
const PATCH_HUNK_CAP: usize = 100_000;

#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy, Debug)]
struct PatchBounds {
    patch_bytes: u64,
    target_bytes: u64,
    output_bytes: u64,
    hunks: usize,
}

#[cfg(not(target_arch = "wasm32"))]
const PATCH_RUNTIME_BOUNDS: PatchBounds = PatchBounds {
    patch_bytes: PATCH_INPUT_BYTE_CAP,
    target_bytes: PATCH_TARGET_BYTE_CAP,
    output_bytes: PATCH_OUTPUT_BYTE_CAP,
    hunks: PATCH_HUNK_CAP,
};

#[cfg(not(target_arch = "wasm32"))]
fn check_patch_limit(dimension: &str, actual: u64, limit: u64) -> Result<(), SemaError> {
    if actual > limit {
        return Err(SemaError::eval(format!(
            "patch/apply-file: {dimension} {actual} exceeds the quarantined limit {limit}"
        ))
        .with_hint("reduce or split the target and patch"));
    }
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn patch_hunk_count(patch: &str) -> usize {
    patch.lines().filter(|line| line.starts_with("@@")).count()
}

#[cfg(not(target_arch = "wasm32"))]
fn patch_added_bytes(patch: &str) -> Result<u64, SemaError> {
    patch
        .lines()
        // Counting file headers too is conservative. Excluding every `+++`
        // line would undercount a real added line whose content starts `++`.
        .filter(|line| line.starts_with('+'))
        .try_fold(0u64, |total, line| {
            total
                .checked_add(line.len() as u64)
                .and_then(|total| total.checked_add(1))
                .ok_or_else(|| SemaError::eval("patch/apply-file: output byte count overflowed"))
        })
}

#[cfg(not(target_arch = "wasm32"))]
fn check_patch_output_bound(
    target_bytes: u64,
    patch: &str,
    bounds: PatchBounds,
) -> Result<(), SemaError> {
    let output_upper_bound = target_bytes
        .checked_add(patch_added_bytes(patch)?)
        .ok_or_else(|| SemaError::eval("patch/apply-file: output byte count overflowed"))?;
    check_patch_limit("output bytes", output_upper_bound, bounds.output_bytes)
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug)]
struct PatchRuntimeInput {
    file: std::fs::File,
    bounds: PatchBounds,
}

#[cfg(not(target_arch = "wasm32"))]
fn prepare_patch_runtime_input(
    path: &str,
    patch: &str,
    bounds: PatchBounds,
) -> Result<PatchRuntimeInput, SemaError> {
    check_patch_limit("patch bytes", patch.len() as u64, bounds.patch_bytes)?;
    check_patch_limit("hunks", patch_hunk_count(patch) as u64, bounds.hunks as u64)?;
    let metadata = std::fs::metadata(path)
        .map_err(|e| SemaError::Io(format!("patch/apply-file {path}: {e}")))?;
    if !metadata.is_file() {
        return Err(SemaError::eval(format!(
            "patch/apply-file: target must be a regular file: {path}"
        )));
    }
    check_patch_limit("target bytes", metadata.len(), bounds.target_bytes)?;
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options
        .open(path)
        .map_err(|e| SemaError::Io(format!("patch/apply-file {path}: {e}")))?;
    let opened = file
        .metadata()
        .map_err(|e| SemaError::Io(format!("patch/apply-file {path}: {e}")))?;
    if !opened.is_file() {
        return Err(SemaError::eval(format!(
            "patch/apply-file: target must be a regular file: {path}"
        )));
    }
    check_patch_limit("target bytes", opened.len(), bounds.target_bytes)?;
    check_patch_output_bound(opened.len(), patch, bounds)?;
    Ok(PatchRuntimeInput { file, bounds })
}

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
    // (diff/unified old new [context]) -> unified-diff string. The LCS is
    // super-linear, so in a runtime quantum it is capped (per-input byte cap)
    // and offloaded onto the I/O pool; outside one it runs synchronously.
    #[cfg(not(target_arch = "wasm32"))]
    register_runtime_fn(env, "diff/unified", |args| {
        check_arity!(args, "diff/unified", 2..=3);
        let old = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let new = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        let context = diff_context_arg(args)?;
        if sema_core::in_runtime_quantum() {
            // Cap each input by byte length BEFORE snapshotting (no excess
            // allocation on the rejected path), then offload the LCS.
            let cap = effective_diff_input_byte_cap();
            check_diff_limit("diff/unified", "old bytes", old.len() as u64, cap)?;
            check_diff_limit("diff/unified", "new bytes", new.len() as u64, cap)?;
            let old = old.to_string();
            let new = new.to_string();
            return crate::io::quarantined_compute(
                "diff/unified",
                diff_string_to_value,
                move || Ok(diff_unified_text(&old, &new, context)),
            );
        }
        Ok(NativeOutcome::Return(Value::string(&diff_unified_text(
            old, new, context,
        ))))
    });
    #[cfg(target_arch = "wasm32")]
    register_fn(env, "diff/unified", |args| {
        check_arity!(args, "diff/unified", 2..=3);
        let old = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let new = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        let context = diff_context_arg(args)?;
        Ok(Value::string(&diff_unified_text(old, new, context)))
    });

    // (diff/stat patch) -> {:added <int> :removed <int> :hunks <int>}
    register_fn(env, "diff/stat", |args| {
        check_arity!(args, "diff/stat", 1);
        let patch = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        #[cfg(not(target_arch = "wasm32"))]
        check_diff_patch_caps("diff/stat", patch)?;
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
        #[cfg(not(target_arch = "wasm32"))]
        check_diff_patch_caps("diff/hunks", patch)?;
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
        #[cfg(not(target_arch = "wasm32"))]
        check_diff_patch_caps("diff/parse", patch)?;

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
        #[cfg(not(target_arch = "wasm32"))]
        check_diff_apply_caps(content, patch)?;
        let hunks = parse_hunks(patch);
        let patched = apply_hunks(content, &hunks)?;
        Ok(Value::string(&patched))
    });

    // (patch/apply-file path patch) -> number of hunks applied (int)
    // Touches the real filesystem (not the VFS), so it's native-only.
    #[cfg(not(target_arch = "wasm32"))]
    register_runtime_fn_gated(env, sandbox, Caps::FS_WRITE, "patch/apply-file", |args| {
        check_arity!(args, "patch/apply-file", 2);
        let path = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?
            .to_string();
        let patch = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?
            .to_string();

        if sema_core::in_runtime_quantum() {
            let bounds = PATCH_RUNTIME_BOUNDS;
            let input = prepare_patch_runtime_input(&path, &patch, bounds)?;
            return crate::io::quarantined_compute("patch/apply-file", Value::int, move || {
                patch_apply_file_work(&path, &patch, Some(input)).map_err(|e| e.to_string())
            });
        }
        Ok(NativeOutcome::Return(Value::int(patch_apply_file_work(
            &path, &patch, None,
        )?)))
    });
}

/// `patch/apply-file`'s actual work: read `path`, apply `patch`'s hunks, write
/// the patched content back, returning the hunk count. Runtime callers provide
/// the bounds captured before dispatch; `SemaError` never crosses the thread
/// boundary, only its `.to_string()` rendering does.
#[cfg(not(target_arch = "wasm32"))]
fn patch_apply_file_work(
    path: &str,
    patch: &str,
    input: Option<PatchRuntimeInput>,
) -> Result<i64, SemaError> {
    let (content, runtime_file) = if let Some(input) = input {
        use std::io::Read as _;

        let PatchRuntimeInput { file, bounds } = input;
        let mut content = String::new();
        (&file)
            .take(bounds.target_bytes.saturating_add(1))
            .read_to_string(&mut content)
            .map_err(|e| SemaError::Io(format!("patch/apply-file {path}: {e}")))?;
        check_patch_limit("target bytes", content.len() as u64, bounds.target_bytes)?;
        check_patch_output_bound(content.len() as u64, patch, bounds)?;
        (content, Some((file, bounds)))
    } else {
        (
            std::fs::read_to_string(path)
                .map_err(|e| SemaError::Io(format!("patch/apply-file {path}: {e}")))?,
            None,
        )
    };
    let hunks = parse_hunks(patch);
    let count = hunks.len() as i64;
    let patched = apply_hunks(&content, &hunks)?;
    if let Some((mut file, bounds)) = runtime_file {
        use std::io::{Seek as _, Write as _};

        check_patch_limit("output bytes", patched.len() as u64, bounds.output_bytes)?;
        file.set_len(0)
            .map_err(|e| SemaError::Io(format!("patch/apply-file {path}: {e}")))?;
        file.seek(std::io::SeekFrom::Start(0))
            .map_err(|e| SemaError::Io(format!("patch/apply-file {path}: {e}")))?;
        file.write_all(patched.as_bytes())
            .map_err(|e| SemaError::Io(format!("patch/apply-file {path}: {e}")))?;
        return Ok(count);
    }
    std::fs::write(path, patched)
        .map_err(|e| SemaError::Io(format!("patch/apply-file {path}: {e}")))?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sema_core::{Env, Sandbox};

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn diff_limit_accepts_boundary_and_rejects_one_over() {
        assert!(check_diff_limit("diff/unified", "old bytes", 8, 8).is_ok());
        let error = check_diff_limit("diff/unified", "old bytes", 9, 8)
            .expect_err("one byte over the captured limit must fail");
        assert!(error.to_string().contains('9'));
        assert!(error.to_string().contains('8'));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn diff_input_byte_cap_is_finite_and_clamps_overrides() {
        assert_eq!(effective_diff_input_byte_cap(), DIFF_INPUT_BYTE_CAP);
        set_diff_input_byte_cap_override(Some(16));
        assert_eq!(effective_diff_input_byte_cap(), 16);
        // An override above the hard ceiling is clamped down, never raised.
        set_diff_input_byte_cap_override(Some(u64::MAX));
        assert_eq!(effective_diff_input_byte_cap(), DIFF_INPUT_BYTE_CAP);
        set_diff_input_byte_cap_override(None);
        assert_eq!(effective_diff_input_byte_cap(), DIFF_INPUT_BYTE_CAP);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn patch_quarantine_limit_accepts_boundary_and_rejects_one_over() {
        assert!(check_patch_limit("patch bytes", 8, 8).is_ok());
        let error = check_patch_limit("patch bytes", 9, 8)
            .expect_err("one byte over the captured limit must fail");
        assert!(error.to_string().contains("9"));
        assert!(error.to_string().contains("8"));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn patch_runtime_input_rejects_non_regular_files() {
        let path = std::env::temp_dir().join(format!("sema-patch-special-{}", std::process::id()));
        std::fs::create_dir_all(&path).expect("create special-input directory");
        let error = prepare_patch_runtime_input(
            path.to_str().expect("utf-8 temp path"),
            "@@ -1 +1 @@\n-old\n+new\n",
            PATCH_RUNTIME_BOUNDS,
        )
        .expect_err("directory must not enter the worker queue");
        let _ = std::fs::remove_dir(&path);
        assert!(error.to_string().contains("regular file"));
    }

    #[cfg(all(unix, not(target_arch = "wasm32")))]
    #[test]
    fn patch_runtime_input_rejects_fifo_without_blocking() {
        use std::os::unix::ffi::OsStrExt as _;

        let path = std::env::temp_dir().join(format!("sema-patch-fifo-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let path_c = std::ffi::CString::new(path.as_os_str().as_bytes()).expect("FIFO path");
        assert_eq!(unsafe { libc::mkfifo(path_c.as_ptr(), 0o600) }, 0);
        let error = prepare_patch_runtime_input(
            path.to_str().expect("utf-8 temp path"),
            "@@ -1 +1 @@\n-old\n+new\n",
            PATCH_RUNTIME_BOUNDS,
        )
        .expect_err("FIFO must not enter the worker queue");
        let _ = std::fs::remove_file(&path);
        assert!(error.to_string().contains("regular file"));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn patch_worker_rechecks_output_bound_after_open() {
        let path = std::env::temp_dir().join(format!(
            "sema-patch-grow-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::write(&path, b"").expect("create target");
        let patch = "@@ -1 +1 @@\n-old\n+new\n";
        let bounds = PatchBounds {
            patch_bytes: 64,
            target_bytes: 8,
            output_bytes: 8,
            hunks: 1,
        };
        let input =
            prepare_patch_runtime_input(path.to_str().expect("utf-8 temp path"), patch, bounds)
                .expect("empty target passes preflight");
        std::fs::write(&path, b"12345678").expect("grow target after descriptor capture");

        let error =
            patch_apply_file_work(path.to_str().expect("utf-8 temp path"), patch, Some(input))
                .expect_err("grown target plus additions must fail before patch construction");
        let _ = std::fs::remove_file(&path);
        assert!(error.to_string().contains("output bytes"), "{error}");
    }

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
