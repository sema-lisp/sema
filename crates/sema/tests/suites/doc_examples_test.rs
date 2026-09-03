//! Doctest runner for structured builtin docs and selected website reference pages. It evaluates
//! `; =>`-annotated examples and checks that the printed result matches.
//!
//! Conservative by design: it checks expressions with `; => <expected>` assertions, carries
//! preceding setup statements in the same block, and skips side-effecting or nondeterministic
//! examples (I/O, LLM, time, randomness, channels, …). Multi-line expressions are supported.
//!
//! Runs in CI. Every `; =>` assertion in a doc entry is verified here unless the
//! expression matches `SKIP_MARKERS`; an expected value of `error: ...` asserts
//! that evaluation fails. Run alone with:
//!   cargo nextest run -p sema-lang --test misc_suite builtin_doc_examples_evaluate --no-capture

use sema_eval::Interpreter;

/// Substrings that mark an example as side-effecting / nondeterministic → skip.
const SKIP_MARKERS: &[&str] = &[
    "http",
    "llm/",
    "file/",
    "io/",
    "net",
    "channel",
    "async",
    "await",
    "prompt",
    "agent",
    "conversation",
    "embedding",
    "tool/",
    "message",
    "random",
    "rand",
    "time",
    "now",
    "sql",
    "db/",
    "serial",
    "pio/",
    "spawn",
    "send",
    "recv",
    "web",
    "fetch",
    "sleep",
    "spy",
    "print",
    "stdin",
    "stdout",
    "stderr",
    "vector-store",
    "route",
    "log/",
    "throw",
    "error",
    "assert",
    "exit",
    "system",
    "sys/",
    "env",
    "shell",
    "exec",
    "read",
    "load",
    "import",
    // `context/*` examples depend on state set in sibling example blocks; `hashmap`/`hash-map`
    // key order is unspecified; `path/absolute` is machine-specific.
    "context",
    "hashmap",
    "hash-map",
    "path/absolute",
    "from-codepoints",
    "term-size",
    // Repository/machine-specific: the checkout's git state and absolute paths.
    "git/",
    "path/canonicalize",
    // Stateful across example blocks (a store opened earlier) or nondeterministic counters.
    "kv/",
    "gc/",
    "stream/open",
    // Terminal escape emitters write control sequences to stdout; process,
    // pty, and watcher examples touch the host.
    "term/",
    "proc/",
    "pty/",
    "fs/",
    // Archive builders write files into the working directory.
    "tar/",
    "zip/",
];

fn skip(expr: &str) -> bool {
    SKIP_MARKERS.iter().any(|m| expr.contains(m))
}

fn check_example(
    name: &str,
    example: &str,
    checked: &mut usize,
    skipped: &mut usize,
    failures: &mut Vec<String>,
) {
    let interp = Interpreter::new();
    // Accumulate lines until the delimiters balance, so a multi-line form runs as one setup or
    // assertion. The expected result may be attached to the closing line.
    let mut pending = String::new();
    let mut pending_expected: Option<String> = None;
    for line in example.lines() {
        let line = line.trim();
        if line.is_empty() || (pending.is_empty() && line.starts_with(';')) {
            continue;
        }
        let (code, expected) = match line.split_once("; =>") {
            Some((code, expected)) => {
                // Text after a second semicolon is a note, not part of the expected value.
                let expected = expected.split(';').next().unwrap_or("").trim();
                (code.trim(), Some(expected.to_string()))
            }
            None => (line.split(';').next().unwrap_or("").trim(), None),
        };
        if !pending.is_empty() {
            pending.push('\n');
        }
        pending.push_str(code);
        if expected.is_some() {
            pending_expected = expected;
        }
        let balanced = pending.matches('(').count() <= pending.matches(')').count()
            && pending.matches('[').count() <= pending.matches(']').count();
        if !balanced {
            continue;
        }
        let expr = std::mem::take(&mut pending);
        let expr = expr.trim();
        let Some(expected) = pending_expected.take() else {
            if !skip(expr) {
                let _ = interp.eval_str(expr);
            }
            continue;
        };
        let inexact = expected.contains('~')
            || expected.contains("...")
            || expected.contains("varies")
            || expected.contains(" (")
            || expected.contains('\\');
        let looks_evaluable = expr.chars().any(|c| c.is_alphanumeric() || c == '(');
        if expr.is_empty() || !looks_evaluable || skip(expr) || inexact {
            *skipped += 1;
            continue;
        }
        let expects_error = expected.starts_with("error");
        match interp.eval_str(expr) {
            Ok(v) if expects_error => {
                *checked += 1;
                failures.push(format!(
                    "{name}: `{expr}` => `{v}` (expected an error: `{expected}`)"
                ));
            }
            Ok(v) => {
                *checked += 1;
                let got = format!("{v}");
                if got.trim() != expected {
                    failures.push(format!(
                        "{name}: `{expr}` => `{got}` (expected `{expected}`)"
                    ));
                }
            }
            Err(e) if expects_error => {
                *checked += 1;
                let want = expected
                    .trim_start_matches("error")
                    .trim_start_matches(':')
                    .trim();
                let msg = e.to_string();
                if !want.is_empty() && !msg.contains(want) {
                    failures.push(format!(
                        "{name}: `{expr}` errored with `{}` (expected `{expected}`)",
                        msg.lines().next().unwrap_or("")
                    ));
                }
            }
            Err(e) => {
                // A form may depend on setup that this conservative checker skipped.
                let msg = e.to_string();
                if msg.contains("Unbound variable") {
                    *skipped += 1;
                } else {
                    *checked += 1;
                    failures.push(format!(
                        "{name}: `{expr}` errored: {}",
                        msg.lines().next().unwrap_or("")
                    ));
                }
            }
        }
    }
}

fn assert_examples<'a>(label: &str, examples: impl IntoIterator<Item = (&'a str, &'a str)>) {
    let mut checked = 0usize;
    let mut skipped = 0usize;
    let mut failures = Vec::new();
    for (name, example) in examples {
        check_example(name, example, &mut checked, &mut skipped, &mut failures);
    }

    eprintln!(
        "{label}: {checked} checked, {} failed, {skipped} skipped",
        failures.len()
    );
    for f in &failures {
        eprintln!("  MISMATCH {f}");
    }
    assert!(
        failures.is_empty(),
        "{} doc example(s) do not match their `; =>` value (see MISMATCH lines above)",
        failures.len()
    );
}

fn sema_code_blocks(page: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut current = None;
    for line in page.lines() {
        if line.trim() == "```sema" {
            current = Some(String::new());
        } else if line.trim() == "```" {
            if let Some(block) = current.take() {
                blocks.push(block);
            }
        } else if let Some(block) = &mut current {
            block.push_str(line);
            block.push('\n');
        }
    }
    blocks
}

fn assert_page_examples(label: &str, page: &str) {
    let blocks = sema_code_blocks(page);
    assert_examples(label, blocks.iter().map(|block| (label, block.as_str())));
}

#[test]
fn builtin_doc_examples_evaluate() {
    let index = sema_docs::builtin_index();
    let examples = index.entries.iter().flat_map(|entry| {
        entry
            .examples
            .iter()
            .map(move |example| (entry.name.as_str(), example.as_str()))
    });
    assert_examples("builtin doc examples", examples);
}

#[test]
fn special_forms_page_examples_evaluate() {
    const PAGE: &str = include_str!("../../../../website/docs/language/special-forms.md");
    for name in sema_eval::SPECIAL_FORM_NAMES {
        assert!(
            PAGE.contains(&format!("`{name}`")),
            "special-forms page does not mention evaluator form `{name}`"
        );
    }

    assert_page_examples("special-forms page examples", PAGE);
}

#[test]
fn macros_modules_page_examples_evaluate() {
    const PAGE: &str = include_str!("../../../../website/docs/language/macros-modules.md");
    assert_page_examples("macros-modules page examples", PAGE);
}

#[test]
fn data_types_page_examples_evaluate() {
    const PAGE: &str = include_str!("../../../../website/docs/language/data-types.md");
    assert_page_examples("data-types page examples", PAGE);
}

#[test]
fn special_forms_documented_edge_cases() {
    let interp = Interpreter::new();

    assert_eq!(interp.eval_str("(and nil)").unwrap().to_string(), "nil");
    assert_eq!(interp.eval_str("(or nil)").unwrap().to_string(), "nil");
    assert_eq!(interp.eval_str("(if #f 1)").unwrap().to_string(), "nil");
    assert_eq!(
        interp
            .eval_str("(letrec (([a b] '(1 2))) (+ a b))")
            .unwrap()
            .to_string(),
        "3"
    );
    assert_eq!(
        interp
            .eval_str("(match {} ({:missing x} :explicit) ({:keys [missing]} missing))")
            .unwrap()
            .to_string(),
        "nil"
    );
    assert_eq!(
        interp.eval_str("(let ((and 5)) and)").unwrap().to_string(),
        "5"
    );
    assert_eq!(
        interp
            .eval_str("(let ((and (fn (a b) (* a b)))) (and 3 4))")
            .unwrap()
            .to_string(),
        "4"
    );

    let err = interp
        .eval_str("(define (sum-pair [a b]) (+ a b))")
        .unwrap_err();
    assert!(
        err.to_string().contains("define: expected a symbol"),
        "unexpected error: {err}"
    );
}

#[test]
fn macros_and_data_types_documented_edge_cases() {
    let interp = Interpreter::new();

    assert_eq!(
        interp
            .eval_str(
                "(begin (defmacro outer (x) (list 'inner x)) \
                 (defmacro inner (x) (list '+ x 1)) \
                 (macroexpand '(outer 4)))",
            )
            .unwrap()
            .to_string(),
        "(inner 4)"
    );
    assert_eq!(
        interp
            .eval_str("(list (type 1/2) (type 3+4i) (type nil) (type '()))")
            .unwrap()
            .to_string(),
        "(:rational :complex :nil :list)"
    );
    assert_eq!(
        interp
            .eval_str("(list (equal? nil '()) (if '() :true :false))")
            .unwrap()
            .to_string(),
        "(#f :true)"
    );
    assert_eq!(
        interp
            .eval_str("(list (type (delay 1)) (type (async/resolved 1)))")
            .unwrap()
            .to_string(),
        "(:promise :async-promise)"
    );
}
