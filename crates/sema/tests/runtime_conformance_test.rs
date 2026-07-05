//! Architectural conformance gate for ADR #69 (one I/O pool behind one seam).
//!
//! Scans every `crates/*/src/**/*.rs` for tokio-runtime creation and for bypasses
//! of the sanctioned `sema_io::io_*` wrappers, and fails on any hit outside an
//! explicit allowlist. This is what makes "no ad-hoc runtimes" an invariant CI
//! enforces rather than a review convention: adding a runtime anywhere new — or
//! calling the raw sema-core seam directly from a consumer crate — fails this test
//! with the offending file:line, and the fix is to go through `sema-io` or to add
//! an allowlist entry WITH a written reason in the same commit.
//!
//! Matching is comment-stripped and path-free (`Runtime::new(`,
//! `new_multi_thread(`, …) so a `use tokio::runtime::Builder;` import doesn't
//! evade the scan. Test code is exempt: `crates/*/tests/**` isn't scanned, and
//! in-file `#[cfg(test)]` fixtures are carried as counted allowlist entries.

use std::fs;
use std::path::{Path, PathBuf};

/// Forbidden tokens (post comment-stripping). Path-free so imports can't evade.
const RUNTIME_TOKENS: &[&str] = &[
    "Runtime::new(",
    "new_multi_thread(",
    "new_current_thread(",
    "#[tokio::main]",
];

/// Raw-seam bypasses: consumer crates must go through the sanctioned
/// `sema_io::io_*` install-then-delegate wrappers, never the raw sema-core seam.
const RAW_SEAM_TOKENS: &[&str] = &[
    "sema_core::io_spawn(",
    "sema_core::io_spawn_blocking(",
    "sema_core::io_block_on(",
];

/// (path suffix, max allowed hits, reason). Anything not listed allows 0.
const ALLOWLIST: &[(&str, usize, &str)] = &[
    (
        "sema-io/src/lib.rs",
        1,
        "the blessed backend: THE one pool builder lives here",
    ),
    (
        "sema-otel/src/imp.rs",
        1,
        "isolated OTLP export reactor — telemetry must not contend with or tear \
         down with user I/O (ADR #69 scope)",
    ),
    (
        "sema/src/main.rs",
        7,
        "subcommand entry-point drivers (lsp/dap/mcp/workflow/notebook) ARE main()",
    ),
    (
        "sema-mcp/src/builtins.rs",
        1,
        "out of Slice A: private current-thread reactor with progress-only-during-\
         block_on semantics; consolidation is a behavior change — own follow-up slice",
    ),
    (
        "sema-mcp/src/client_auth.rs",
        1,
        "out of Slice A: same as sema-mcp builtins",
    ),
    (
        "sema-notebook/src/bridge.rs",
        1,
        "out of Slice A: blocking-recv shim; right fix is std::sync::mpsc, no seam",
    ),
    (
        "sema-stdlib/src/server.rs",
        2,
        "in-file #[cfg(test)] fixtures asserting inside-runtime behavior",
    ),
];

/// Strip `//` line comments, tracking double-quote string state per line so a
/// `//` inside a string (e.g. `"https://…"`) survives. Deliberately does NOT
/// strip `/* */` block comments: a string literal containing `/*` (server.rs has
/// them) would otherwise swallow real code and silently UNDER-report — the one
/// failure direction this gate must never have. A commented-out forbidden token
/// inside a block comment therefore false-POSITIVES, which fails loudly and is
/// resolved by deleting the dead code. Line-scoped state also bounds any
/// char-literal (`'"'`) mis-parse to a single line.
fn strip_line_comments(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut in_string = false;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if in_string => i += 1, // skip escaped char inside a string
            b'"' => in_string = !in_string,
            b'/' if !in_string && i + 1 < bytes.len() && bytes[i + 1] == b'/' => {
                return &line[..i];
            }
            _ => {}
        }
        i += 1;
    }
    line
}

fn rs_files_under(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            // Skip build artifacts and fuzz corpora.
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name == "target" || name == "fuzz" {
                continue;
            }
            rs_files_under(&p, out);
        } else if p.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(p);
        }
    }
}

#[test]
fn no_adhoc_tokio_runtimes_outside_allowlist() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let crates_dir = root.join("crates");
    assert!(crates_dir.is_dir(), "workspace crates/ dir not found");

    // Collect every crates/*/src tree.
    let mut files = Vec::new();
    for entry in fs::read_dir(&crates_dir).expect("read crates/").flatten() {
        let src = entry.path().join("src");
        if src.is_dir() {
            rs_files_under(&src, &mut files);
        }
    }
    assert!(
        files.len() > 50,
        "scan looks broken: only {} source files found",
        files.len()
    );

    let mut violations: Vec<String> = Vec::new();
    for file in &files {
        let rel = file
            .strip_prefix(&root)
            .unwrap_or(file)
            .to_string_lossy()
            .replace('\\', "/");
        let Ok(src) = fs::read_to_string(file) else {
            continue;
        };

        let in_sema_io = rel.contains("sema-io/src/");
        let mut hits = 0usize;
        let mut hit_lines: Vec<String> = Vec::new();
        for (lineno, raw_line) in src.lines().enumerate() {
            let line = strip_line_comments(raw_line);
            for tok in RUNTIME_TOKENS {
                if line.contains(tok) {
                    hits += 1;
                    hit_lines.push(format!("{}:{}: {}", rel, lineno + 1, tok));
                }
            }
            if !in_sema_io {
                for tok in RAW_SEAM_TOKENS {
                    if line.contains(tok) {
                        hits += 1;
                        hit_lines.push(format!("{}:{}: raw-seam bypass {}", rel, lineno + 1, tok));
                    }
                }
            }
        }
        if hits == 0 {
            continue;
        }
        let allowed = ALLOWLIST
            .iter()
            .find(|(suffix, _, _)| rel.ends_with(suffix))
            .map(|(_, max, _)| *max)
            .unwrap_or(0);
        if hits > allowed {
            violations.push(format!(
                "{rel}: {hits} runtime-creation/bypass hit(s), {allowed} allowed:\n    {}",
                hit_lines.join("\n    ")
            ));
        }
    }

    assert!(
        violations.is_empty(),
        "ad-hoc tokio runtimes (or raw-seam bypasses) outside the ADR #69 allowlist \
         — route through sema-io, or add an allowlist entry WITH a reason:\n\n{}",
        violations.join("\n\n")
    );
}
