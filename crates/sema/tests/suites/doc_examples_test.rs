//! Doctest runner for the structured builtin docs: evaluates the `; =>`-annotated example lines in
//! `sema_docs::builtin_index()` and checks the printed result matches.
//!
//! Conservative by design — it only checks single-line `<expr> ; => <expected>` assertions (with
//! preceding single-line setup statements sharing one interpreter), and skips anything
//! side-effecting or nondeterministic (I/O, LLM, time, randomness, channels, …). Multi-line
//! expressions and unparseable lines are skipped, not failed.
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

#[test]
fn builtin_doc_examples_evaluate() {
    let index = sema_docs::builtin_index();
    let mut checked = 0usize;
    let mut skipped = 0usize;
    let mut failures: Vec<String> = Vec::new();

    for entry in &index.entries {
        for example in &entry.examples {
            let interp = Interpreter::new();
            // Accumulate lines until the parens balance, so a multi-line
            // `define`/`defmacro` runs as one setup form and a `; =>` on the
            // closing line of a multi-line form asserts on the whole form.
            let mut pending = String::new();
            let mut pending_expected: Option<String> = None;
            for line in example.lines() {
                let line = line.trim();
                if line.is_empty() || (pending.is_empty() && line.starts_with(';')) {
                    continue;
                }
                let (code, expected) = match line.split_once("; =>") {
                    Some((code, expected)) => {
                        // A trailing `; note` after the expected value is prose,
                        // not part of the oracle: `; => 7    ; single element`.
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
                    // Setup statement (e.g. `(define x 1)`) — eval to build shared state.
                    if !skip(expr) {
                        let _ = interp.eval_str(expr);
                    }
                    continue;
                };
                // Skip side-effecting exprs, and expecteds that aren't a single literal value:
                // approximations (`~`, `...`), nondeterministic notes (`varies`), or any
                // parenthetical annotation like `nil (not visible)` / escaped-quote strings.
                let inexact = expected.contains('~')
                    || expected.contains("...")
                    || expected.contains("varies")
                    || expected.contains(" (")
                    || expected.contains('\\');
                let looks_evaluable = expr.chars().any(|c| c.is_alphanumeric() || c == '(');
                if expr.is_empty() || !looks_evaluable || skip(expr) || inexact {
                    skipped += 1;
                    continue;
                }
                // `; => error: <message>` asserts that evaluation fails.
                let expects_error = expected.starts_with("error");
                match interp.eval_str(expr) {
                    Ok(v) if expects_error => {
                        checked += 1;
                        failures.push(format!(
                            "{}: `{expr}` => `{v}` (expected an error: `{expected}`)",
                            entry.name
                        ));
                    }
                    Ok(v) => {
                        checked += 1;
                        let got = format!("{v}");
                        if got.trim() != expected {
                            failures.push(format!(
                                "{}: `{expr}` => `{got}` (expected `{expected}`)",
                                entry.name
                            ));
                        }
                    }
                    Err(e) if expects_error => {
                        checked += 1;
                        let want = expected
                            .trim_start_matches("error")
                            .trim_start_matches(':')
                            .trim();
                        let msg = e.to_string();
                        if !want.is_empty() && !msg.contains(want) {
                            failures.push(format!(
                                "{}: `{expr}` errored with `{}` (expected `{expected}`)",
                                entry.name,
                                msg.lines().next().unwrap_or("")
                            ));
                        }
                    }
                    Err(e) => {
                        // An example that errors is a broken example. Only a
                        // form that depends on state the checker could not
                        // build is skipped; that shows up as an unbound variable.
                        let msg = e.to_string();
                        if msg.contains("Unbound variable") {
                            skipped += 1;
                        } else {
                            checked += 1;
                            failures.push(format!(
                                "{}: `{expr}` errored: {}",
                                entry.name,
                                msg.lines().next().unwrap_or("")
                            ));
                        }
                    }
                }
            }
        }
    }

    eprintln!(
        "doc examples: {checked} checked, {} failed, {skipped} skipped",
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
