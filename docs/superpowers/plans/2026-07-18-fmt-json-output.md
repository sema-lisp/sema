# Formatter JSON Output Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make file-mode `sema fmt --json` emit valid read-only NDJSON with an explicit `changed` field and correct option exit semantics.

**Architecture:** Keep formatting and change detection shared with normal file mode, then branch only for output and file mutation. JSON mode emits one object per resolved file and bypasses human summaries and writes; normal formatting retains its current behavior.

**Tech Stack:** Rust 2021, Clap, serde_json, Cargo integration tests

## Global Constraints

- JSON file mode is read-only and never rewrites source files.
- Preserve NDJSON and the existing `file`, `formatted`, `source`, and `error` fields.
- Add `changed` only to successful file results.
- `--check --json` exits 1 when any file would change.
- `--diff --json` is rejected by Clap.
- Standard output contains no human-readable text in JSON mode.
- Do not change stdin formatting behavior.

---

### Task 1: Lock down the file-mode JSON contract

**Files:**
- Modify: `crates/sema/tests/integration_test.rs`
- Modify: `crates/sema/src/main.rs:265-300`
- Modify: `crates/sema/src/main.rs:3058-3263`

**Interfaces:**
- Consumes: the `sema fmt [OPTIONS] [FILES...]` CLI and `run_fmt` file loop.
- Produces: successful NDJSON records shaped as `{file, formatted: true, changed, source}`; existing error records; exit status 1 for `--check --json` changes.

- [ ] **Step 1: Add failing real-binary integration tests**

Add focused `test_fmt_json_*` tests to `crates/sema/tests/integration_test.rs`. Use the existing `unique_temp_dir` helper, `env!("CARGO_BIN_EXE_sema")`, and `serde_json::Value`. Cover these assertions:

```rust
const FMT_JSON_UGLY: &str = "(define   x   1)\n";
const FMT_JSON_PRETTY: &str = "(define x 1)\n";

fn run_fmt_command(dir: &std::path::Path, args: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_sema"))
        .arg("fmt")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("failed to run sema fmt")
}

fn parse_ndjson(output: &[u8]) -> Vec<serde_json::Value> {
    String::from_utf8_lossy(output)
        .lines()
        .map(|line| serde_json::from_str(line).expect("stdout line must be JSON"))
        .collect()
}
```

The tests must verify:

```rust
// Changed input: one valid record, changed=true, canonical source, original file untouched.
// Unchanged input: one valid record with changed=false.
// Two inputs: exactly two parseable lines and no aggregate summary.
// --check --json: changed input returns status 1 and still emits one valid record.
// --diff --json: Clap rejects the combination and names both flags in stderr.
// An unmatched glob: status 0 and empty stdout.
// A missing explicit file: status 1 and exactly one parseable formatted=false record.
```

Remove each temporary directory at the end of its test.

- [ ] **Step 2: Run the focused tests and verify RED**

Run:

```bash
cargo test -p sema-lang --test integration_test -- test_fmt_json --nocapture
```

Expected: the contract tests fail because output contains `0 file(s) already formatted`, successful records lack `changed`, `--check --json` exits 0, and Clap accepts `--diff --json`.

- [ ] **Step 3: Implement shared change detection and JSON-only output**

In the `Fmt` Clap fields, reject diff plus JSON:

```rust
/// Output result as JSON (useful for editor integrations)
#[arg(long, conflicts_with = "diff")]
json: bool,
```

In `run_fmt`, suppress the no-file human message for JSON mode:

```rust
if files.is_empty() {
    if !json {
        println!("No .sema files found");
    }
    return;
}
```

Compute change state before the JSON branch and include it in successful records:

```rust
checked += 1;
let file_changed = source != formatted;
if file_changed {
    changed += 1;
}

if json {
    println!(
        "{}",
        serde_json::json!({
            "file": file,
            "formatted": true,
            "changed": file_changed,
            "source": formatted
        })
    );
    continue;
}

if file_changed {
    // Preserve the existing check/diff/write branches.
}
```

Guard the existing human summary with `if !json`. Preserve its messages and normal-mode early exit, then add the JSON check exit after error handling:

```rust
if !json {
    // Existing check/diff/format summary logic.
}

if errors > 0 {
    eprintln!("{errors} error(s)");
    std::process::exit(1);
}

if check && changed > 0 {
    std::process::exit(1);
}
```

- [ ] **Step 4: Run the focused tests and verify GREEN**

Run:

```bash
cargo test -p sema-lang --test integration_test -- test_fmt_json --nocapture
```

Expected: all `test_fmt_json_*` tests pass with no warnings.

- [ ] **Step 5: Format and lint the changed Rust code**

Run:

```bash
cargo fmt --all -- --check
cargo clippy -p sema-lang --all-targets -- -D warnings
```

Expected: both commands exit 0 without warnings.

- [ ] **Step 6: Commit the tested formatter fix**

```bash
git add crates/sema/src/main.rs crates/sema/tests/integration_test.rs
git commit -m "fix(fmt): emit valid JSON output"
```

### Task 2: Document and verify the public CLI contract

**Files:**
- Modify: `website/docs/formatter.md`
- Modify: `website/docs/cli.md`

**Interfaces:**
- Consumes: the JSON schema and option interactions implemented in Task 1.
- Produces: user-facing reference text for read-only NDJSON output, `changed`, `--check`, and the `--diff` conflict.

- [ ] **Step 1: Update formatter reference documentation**

Add `--json` to the options table in `website/docs/formatter.md` and add a concise reference section containing this example:

```bash
sema fmt --json src/main.sema
```

```json
{"file":"src/main.sema","formatted":true,"changed":true,"source":"(define x 1)\n"}
```

State that JSON mode is read-only NDJSON, one record per file; `formatted` reports success, `changed` compares input with `source`; `--check --json` exits 1 when changes are needed; and `--diff` cannot be combined with `--json`.

- [ ] **Step 2: Tighten the CLI reference entry**

Change the `website/docs/cli.md` option description from `Output result as JSON (useful for editor integrations)` to `Emit read-only NDJSON results for editor integrations`, and include `sema fmt --check --json` in its examples.

- [ ] **Step 3: Verify documentation and the focused CLI tests**

Run:

```bash
cargo test -p sema-lang --test integration_test -- test_fmt_json
jake docs-check
git diff --check
```

Expected: all commands exit 0.

- [ ] **Step 4: Commit the documentation**

```bash
git add website/docs/formatter.md website/docs/cli.md
git commit -m "docs(fmt): describe JSON output contract"
```

### Task 3: Final regression verification

**Files:**
- Verify only; no planned file changes.

**Interfaces:**
- Consumes: Tasks 1 and 2.
- Produces: evidence that formatter CLI behavior and the `sema-lang` test target remain healthy.

- [ ] **Step 1: Run formatter and CLI regression tests**

```bash
cargo test -p sema-lang --test integration_test -- test_fmt_json
cargo test -p sema-fmt
```

Expected: all tests pass.

- [ ] **Step 2: Run final static checks**

```bash
cargo fmt --all -- --check
cargo clippy -p sema-lang --all-targets -- -D warnings
git status --short -- crates/sema/src/main.rs crates/sema/tests/integration_test.rs website/docs/formatter.md website/docs/cli.md
```

Expected: formatting and Clippy pass; the formatter files are clean after the planned commits. Preserve unrelated worktree changes.
