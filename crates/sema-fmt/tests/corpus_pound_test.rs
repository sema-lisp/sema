//! Corpus pounding harness: formats every `.sema` file it can find under
//! `SEMA_FMT_POUND_DIRS` (colon-separated; defaults to the repo's examples/)
//! and checks, for several option sets:
//!   1. valid input (per sema-reader) must format without error
//!   2. the output must still parse (sema-reader)
//!   3. the output must read to the SAME values as the input (semantics)
//!   4. formatting must be idempotent
//!   5. the comment count must not change (comments never dropped/invented)
//!
//! Run explicitly:
//!   SEMA_FMT_POUND_DIRS=/path/a:/path/b cargo test -p sema-fmt \
//!     --test corpus_pound_test -- --ignored --nocapture
use sema_fmt::{format_source, FormatOptions};
use std::collections::HashSet;
use std::path::PathBuf;

fn collect_sema_files(dir: &str, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            if name == "target" || name == ".git" || name == "node_modules" || name == ".worktrees"
            {
                continue;
            }
            collect_sema_files(path.to_str().unwrap(), out);
        } else if name.ends_with(".sema") {
            out.push(path);
        }
    }
}

/// Count line comments, skipping string-ish regions (plain/f-/regex strings).
/// Approximate — good enough to flag dropped comments for manual review.
fn count_comments(src: &str) -> usize {
    let mut count = 0;
    let mut chars = src.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                // string body: skip to unescaped closing quote
                while let Some(c) = chars.next() {
                    match c {
                        '\\' => {
                            chars.next();
                        }
                        '"' => break,
                        _ => {}
                    }
                }
            }
            ';' => {
                count += 1;
                for c in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    count
}

fn option_sets() -> Vec<(&'static str, FormatOptions)> {
    vec![
        ("default", FormatOptions::default()),
        (
            "align",
            FormatOptions {
                align: true,
                ..Default::default()
            },
        ),
        (
            "narrow",
            FormatOptions {
                width: 40,
                ..Default::default()
            },
        ),
        (
            "narrow+align",
            FormatOptions {
                width: 40,
                align: true,
                ..Default::default()
            },
        ),
    ]
}

#[test]
#[ignore = "corpus pounding — run explicitly with SEMA_FMT_POUND_DIRS"]
fn pound_corpus() {
    let dirs = std::env::var("SEMA_FMT_POUND_DIRS").unwrap_or_else(|_| {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        format!(
            "{}:{}",
            root.join("examples").display(),
            root.join("playground/examples").display()
        )
    });

    let mut files = Vec::new();
    for dir in dirs.split(':') {
        collect_sema_files(dir, &mut files);
    }
    files.sort();

    let mut seen_content: HashSet<u64> = HashSet::new();
    let mut checked = 0usize;
    let mut skipped_invalid = 0usize;
    let mut failures: Vec<String> = Vec::new();

    for path in &files {
        let Ok(source) = std::fs::read_to_string(path) else {
            continue;
        };
        // Dedupe identical files (worktrees, vendored copies)
        let hash = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            source.hash(&mut h);
            h.finish()
        };
        if !seen_content.insert(hash) {
            continue;
        }

        // Validity oracle: sema-reader is the ground truth
        let input_values = match sema_reader::read_many(&source) {
            Ok(v) => v,
            Err(_) => {
                skipped_invalid += 1;
                continue;
            }
        };
        let input_comments = count_comments(&source);
        checked += 1;

        for (opts_name, opts) in option_sets() {
            let tag = format!("{} [{}]", path.display(), opts_name);
            let first = match format_source(&source, &opts) {
                Ok(f) => f,
                Err(e) => {
                    failures.push(format!("FORMAT-ERR   {tag}: {e}"));
                    continue;
                }
            };
            match sema_reader::read_many(&first) {
                Ok(out_values) => {
                    if out_values != input_values {
                        failures.push(format!("SEMANTICS    {tag}: values changed"));
                    }
                }
                Err(e) => {
                    failures.push(format!("UNPARSEABLE  {tag}: {e}"));
                    continue;
                }
            }
            let out_comments = count_comments(&first);
            if out_comments != input_comments {
                failures.push(format!(
                    "COMMENTS     {tag}: {input_comments} -> {out_comments}"
                ));
            }
            match format_source(&first, &opts) {
                Ok(second) => {
                    if second != first {
                        let diff_line = first
                            .lines()
                            .zip(second.lines())
                            .position(|(a, b)| a != b)
                            .map(|i| i + 1)
                            .unwrap_or(0);
                        failures.push(format!(
                            "NON-IDEMPOT  {tag}: first diff at line {diff_line}"
                        ));
                    }
                }
                Err(e) => failures.push(format!("REFORMAT-ERR {tag}: {e}")),
            }
        }
    }

    println!(
        "pounded {checked} unique valid files ({} on disk, {skipped_invalid} not valid sema), {} failures",
        files.len(),
        failures.len()
    );
    for f in &failures {
        println!("{f}");
    }
    assert!(failures.is_empty(), "{} corpus failures", failures.len());
}
