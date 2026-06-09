# Runnable Regions — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add a `sema eval` subcommand with `--stdin --json` flags that outputs a machine-readable JSON result envelope, enabling LSP/editor-driven code evaluation.

**Architecture:** New `Eval` variant in the `Commands` enum reads program text from stdin (or `--expr`), evaluates it with the existing `Interpreter`, captures stdout/stderr, and emits a single JSON object. This is the foundation that the LSP's CodeLens → executeCommand → subprocess flow will use.

**Tech Stack:** Rust (clap CLI), serde_json (already a dependency), existing `Interpreter` + `eval_with_mode`.

---

### Task 1: Add `Eval` subcommand to CLI

**Files:**
- Modify: `crates/sema/src/main.rs` (the `Commands` enum + dispatch in `main()`)

**Step 1: Add the `Eval` variant to `Commands` enum**

In `crates/sema/src/main.rs`, add after the `Fmt` variant (around line 377):

```rust
/// Evaluate code and return results (designed for machine consumption)
Eval {
    /// Read program from stdin instead of --expr
    #[arg(long)]
    stdin: bool,

    /// Expression to evaluate (alternative to --stdin)
    #[arg(long)]
    expr: Option<String>,

    /// Emit machine-readable JSON result envelope
    #[arg(long)]
    json: bool,

    /// Set file path for error spans and relative import resolution
    #[arg(long)]
    path: Option<String>,

    /// Kill evaluation after N milliseconds (default: 5000)
    #[arg(long, default_value = "5000")]
    timeout: u64,

    /// Sandbox mode (e.g., "strict", "all", or capabilities list)
    #[arg(long)]
    sandbox: Option<String>,

    /// Disable LLM features
    #[arg(long)]
    no_llm: bool,

    /// Use bytecode VM instead of tree-walker
    #[arg(long)]
    vm: bool,
},
```

**Step 2: Run build to verify it compiles**

Run: `cargo build -p sema-lang 2>&1 | head -20`
Expected: Compiles with a warning about unreachable pattern (we haven't added the match arm yet).

**Step 3: Add the `run_eval` function and dispatch**

Add a new function `run_eval` and wire it into the `match command` block in `main()`. Insert the match arm after the `Commands::Fmt { .. }` arm (around line 598):

```rust
Commands::Eval {
    stdin,
    expr,
    json,
    path,
    timeout,
    sandbox,
    no_llm,
    vm,
} => {
    run_eval(stdin, expr, json, path, timeout, sandbox, no_llm, vm);
}
```

The `run_eval` function (add near the other `run_*` functions, e.g., after `eval_with_mode`):

```rust
fn run_eval(
    use_stdin: bool,
    expr: Option<String>,
    json: bool,
    path: Option<String>,
    timeout_ms: u64,
    sandbox_arg: Option<String>,
    no_llm: bool,
    use_vm: bool,
) {
    // Get the program text
    let program = if use_stdin {
        use std::io::Read;
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf).unwrap_or_else(|e| {
            if json {
                print_eval_json(false, None, "", "", Some(&format!("Failed to read stdin: {e}")), None, None, 0);
            } else {
                eprintln!("Error reading stdin: {e}");
            }
            std::process::exit(1);
        });
        buf
    } else if let Some(e) = expr {
        e
    } else {
        if json {
            print_eval_json(false, None, "", "", Some("Either --stdin or --expr is required"), None, None, 0);
        } else {
            eprintln!("Error: either --stdin or --expr is required");
        }
        std::process::exit(1);
    };

    // Set up sandbox
    let sandbox = match &sandbox_arg {
        Some(value) => sema_core::Sandbox::parse_cli(value).unwrap_or_else(|e| {
            if json {
                print_eval_json(false, None, "", "", Some(&format!("Invalid sandbox: {e}")), None, None, 0);
            } else {
                eprintln!("Error: {e}");
            }
            std::process::exit(1);
        }),
        None => sema_core::Sandbox::allow_all(),
    };

    let interpreter = Interpreter::new_with_sandbox(&sandbox);

    // Auto-configure LLM unless --no-llm
    if !no_llm {
        let _ = interpreter.eval_str("(llm/auto-configure)");
    }

    // Set file path for import resolution
    if let Some(ref p) = path {
        let file_path = std::path::Path::new(p);
        if let Ok(canonical) = file_path.canonicalize() {
            interpreter.ctx.push_file_path(canonical);
        }
    }

    // Capture stdout/stderr
    let captured_stdout = Rc::new(RefCell::new(String::new()));
    let captured_stderr = Rc::new(RefCell::new(String::new()));

    // Install stdout/stderr capture via VFS
    let out_clone = captured_stdout.clone();
    sema_core::vfs::set_stdout_handler(Box::new(move |s| {
        out_clone.borrow_mut().push_str(s);
    }));
    let err_clone = captured_stderr.clone();
    sema_core::vfs::set_stderr_handler(Box::new(move |s| {
        err_clone.borrow_mut().push_str(s);
    }));

    let start = std::time::Instant::now();
    let result = eval_with_mode(&interpreter, &program, use_vm);
    let elapsed_ms = start.elapsed().as_millis() as u64;

    let stdout_text = captured_stdout.borrow().clone();
    let stderr_text = captured_stderr.borrow().clone();

    // Clear VFS handlers
    sema_core::vfs::clear_stdout_handler();
    sema_core::vfs::clear_stderr_handler();

    match result {
        Ok(val) => {
            let val_str = pretty_print(&val, 120);
            if json {
                print_eval_json(true, Some(&val_str), &stdout_text, &stderr_text, None, None, None, elapsed_ms);
            } else {
                // Print captured stdout first (if any)
                if !stdout_text.is_empty() {
                    print!("{stdout_text}");
                }
                if !val.is_nil() {
                    println!("{val_str}");
                }
            }
        }
        Err(e) => {
            let msg = e.inner().to_string();
            let hint = e.hint().map(|s| s.to_string());
            let (line, col) = e.span().map(|s| (Some(s.line), Some(s.col))).unwrap_or((None, None));
            if json {
                print_eval_json(false, None, &stdout_text, &stderr_text, Some(&msg), hint.as_deref(), line, elapsed_ms);
            } else {
                if !stdout_text.is_empty() {
                    print!("{stdout_text}");
                }
                print_error(&e);
                std::process::exit(1);
            }
        }
    }
}

fn print_eval_json(
    ok: bool,
    value: Option<&str>,
    stdout: &str,
    stderr: &str,
    error_msg: Option<&str>,
    error_hint: Option<&str>,
    error_line: Option<usize>,
    elapsed_ms: u64,
) {
    let result = serde_json::json!({
        "ok": ok,
        "value": value,
        "stdout": stdout,
        "stderr": stderr,
        "error": error_msg.map(|msg| {
            let mut err = serde_json::json!({ "message": msg });
            if let Some(hint) = error_hint {
                err["hint"] = serde_json::json!(hint);
            }
            if let Some(line) = error_line {
                err["line"] = serde_json::json!(line);
            }
            err
        }),
        "elapsedMs": elapsed_ms,
    });
    println!("{}", serde_json::to_string(&result).unwrap());
}
```

**Step 4: Run build to verify it compiles**

Run: `cargo build -p sema-lang 2>&1 | tail -5`
Expected: Compiles successfully.

**Step 5: Commit**

```bash
git add crates/sema/src/main.rs
git commit -m "feat: add sema eval subcommand with --stdin --json --expr flags"
```

---

### Task 2: Handle stdout/stderr capture via VFS

The `run_eval` function above references `sema_core::vfs::set_stdout_handler` and friends. We need to check if the VFS already supports output capture. If not, we need a simpler approach.

**Step 1: Check VFS capabilities**

Run: `grep -n "stdout\|set_stdout\|print_handler\|display_handler\|write_handler" crates/sema-core/src/vfs.rs | head -20`

Depending on findings, either use the existing VFS capture mechanism, or fall back to a simpler approach: pipe the subprocess (which is the LSP's job anyway — for the CLI itself, stdout capture isn't strictly needed since we control the process). 

If VFS doesn't support capture, simplify `run_eval` to just redirect the eval output. The JSON envelope can set `stdout` and `stderr` to empty strings initially — the LSP subprocess runner captures process-level stdout/stderr anyway.

**Step 2: Adjust implementation based on findings**

If no VFS capture exists, simplify to:

```rust
// In run_eval, skip VFS capture, just eval and report
let start = std::time::Instant::now();
let result = eval_with_mode(&interpreter, &program, use_vm);
let elapsed_ms = start.elapsed().as_millis() as u64;

match result {
    Ok(val) => {
        let val_str = if val.is_nil() { String::new() } else { pretty_print(&val, 120) };
        if json {
            print_eval_json(true, if val.is_nil() { None } else { Some(&val_str) },
                            "", "", None, None, None, elapsed_ms);
        } else {
            if !val.is_nil() {
                println!("{val_str}");
            }
        }
    }
    Err(e) => { /* same as above */ }
}
```

**Step 3: Build and verify**

Run: `cargo build -p sema-lang 2>&1 | tail -3`
Expected: Clean compile.

**Step 4: Commit (if changes were needed)**

```bash
git add crates/sema/src/main.rs
git commit -m "feat(eval): simplify stdout capture in eval subcommand"
```

---

### Task 3: Write integration tests for `sema eval`

**Files:**
- Modify: `crates/sema/tests/integration_test.rs`

**Step 1: Add basic eval tests**

Add at the end of `integration_test.rs` (which already has a `sema_cmd()` helper):

```rust
// ── sema eval subcommand ──────────────────────────────────────────

#[test]
fn test_eval_expr_json() {
    let output = sema_cmd()
        .args(["eval", "--expr", "(+ 1 2)", "--json", "--no-llm"])
        .output()
        .expect("failed to run sema eval");
    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));
    let json: serde_json::Value = serde_json::from_slice(&output.stdout)
        .expect("invalid JSON output");
    assert_eq!(json["ok"], true);
    assert_eq!(json["value"], "3");
    assert!(json["elapsedMs"].as_u64().unwrap() < 10000);
}

#[test]
fn test_eval_stdin_json() {
    use std::io::Write;
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_sema"))
        .args(["eval", "--stdin", "--json", "--no-llm"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn sema eval");
    child.stdin.take().unwrap().write_all(b"(* 6 7)").unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["ok"], true);
    assert_eq!(json["value"], "42");
}

#[test]
fn test_eval_error_json() {
    let output = sema_cmd()
        .args(["eval", "--expr", "(/ 1 0)", "--json", "--no-llm"])
        .output()
        .expect("failed to run sema eval");
    // Should still succeed (exit 0) when --json, error is in the envelope
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["ok"], false);
    assert!(json["error"]["message"].as_str().unwrap().len() > 0);
}

#[test]
fn test_eval_expr_no_json() {
    let output = sema_cmd()
        .args(["eval", "--expr", "(+ 10 20)", "--no-llm"])
        .output()
        .expect("failed to run sema eval");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.trim().contains("30"), "expected 30, got: {stdout}");
}

#[test]
fn test_eval_nil_result() {
    let output = sema_cmd()
        .args(["eval", "--expr", "(define x 42)", "--json", "--no-llm"])
        .output()
        .expect("failed to run sema eval");
    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["ok"], true);
    // define returns nil, value should be null
    assert!(json["value"].is_null());
}

#[test]
fn test_eval_stdin_multi_form() {
    use std::io::Write;
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_sema"))
        .args(["eval", "--stdin", "--json", "--no-llm"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn sema eval");
    // Multiple forms: context + target. Last form's value is reported.
    child.stdin.take().unwrap().write_all(
        b"(define pi 3.14)\n(define (area r) (* pi r r))\n(area 10)"
    ).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["ok"], true);
    assert_eq!(json["value"], "314");
}

#[test]
fn test_eval_no_input_error() {
    let output = sema_cmd()
        .args(["eval", "--json", "--no-llm"])
        .output()
        .expect("failed to run sema eval");
    // No --stdin and no --expr: should fail
    assert!(!output.status.success());
}

#[test]
fn test_eval_sandbox() {
    let output = sema_cmd()
        .args(["eval", "--expr", "(shell \"echo hi\")", "--json", "--no-llm", "--sandbox", "strict"])
        .output()
        .expect("failed to run sema eval");
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["ok"], false);
    let msg = json["error"]["message"].as_str().unwrap();
    assert!(msg.contains("sandbox") || msg.contains("denied") || msg.contains("not permitted"),
            "expected sandbox error, got: {msg}");
}
```

**Step 2: Run the tests**

Run: `cargo test -p sema-lang --test integration_test -- test_eval_ 2>&1 | tail -30`
Expected: All `test_eval_*` tests pass.

**Step 3: Commit**

```bash
git add crates/sema/tests/integration_test.rs
git commit -m "test: add integration tests for sema eval subcommand"
```

---

### Task 4: Update Zed runnables and tasks for per-form evaluation

**Files:**
- Modify: `editors/zed/languages/sema/runnables.scm`
- Modify: `editors/zed/languages/sema/tasks.json`

**Step 1: Extend runnables.scm with per-form patterns**

Replace the current `runnables.scm` content:

```scheme
; Run the entire Sema source file.
(source_file) @run (#set! tag "sema-run")

; Run individual top-level definitions
(source_file
  (list
    . (symbol) @_f
    . (symbol) @run
    (#any-of? @_f "defun" "defn" "defmacro" "defagent" "deftool" "define")
  ) @_source (#set! tag "sema-run-form"))

; Run any top-level list expression (non-definition)
(source_file
  (list
    . (symbol) @run
    (#not-any-of? @run "defun" "defn" "defmacro" "defagent" "deftool" "define" "define-record-type" "module" "import" "load" "export")
  ) @_source (#set! tag "sema-run-form"))
```

**Step 2: Update tasks.json with the form-level task**

```jsonc
[
  {
    "label": "sema run",
    "command": "sema",
    "args": ["$ZED_FILE"],
    "tags": ["sema-run"]
  },
  {
    "label": "sema eval form",
    "command": "sema",
    "args": ["eval", "--expr", "$ZED_SELECTED_TEXT", "--no-llm"],
    "tags": ["sema-run-form"]
  }
]
```

**Step 3: Commit**

```bash
git add editors/zed/languages/sema/runnables.scm editors/zed/languages/sema/tasks.json
git commit -m "feat(zed): add per-form runnable regions"
```

---

### Task 5: Update the `defn`/`defun` outline for Zed

**Files:**
- Modify: `editors/zed/languages/sema/outline.scm`

**Step 1: Add `defn` to the outline patterns**

The outline.scm currently matches `defun` but not `defn`. Since `defn` is now the canonical form, add it:

In the first pattern, change:
```scheme
(#any-of? @_f "defun" "defmacro" "defagent" "deftool" "define-record-type")
```
to:
```scheme
(#any-of? @_f "defun" "defn" "defmacro" "defagent" "deftool" "define-record-type")
```

**Step 2: Commit**

```bash
git add editors/zed/languages/sema/outline.scm
git commit -m "feat(zed): add defn to outline queries"
```

---

### Task 6: Wire `--json` error exit code behavior

**Files:**
- Modify: `crates/sema/src/main.rs`

**Step 1: Ensure JSON mode always exits 0**

When `--json` is active, errors should be in the JSON envelope, not as exit code 1. This makes the subprocess easier for the LSP to handle (it always gets valid JSON). Verify that the `run_eval` implementation exits 0 on eval errors when `--json` is set, and exits 1 only for infrastructure errors (can't read stdin, invalid args).

Review the error path in `run_eval`: when an eval error occurs and `json` is true, make sure we do NOT call `std::process::exit(1)` — we just print the JSON envelope and return normally.

**Step 2: Add test for this behavior**

```rust
#[test]
fn test_eval_json_error_exit_zero() {
    let output = sema_cmd()
        .args(["eval", "--expr", "(undefined-fn)", "--json", "--no-llm"])
        .output()
        .expect("failed to run sema eval");
    // --json mode should exit 0 even on eval errors
    assert!(output.status.success(), "expected exit 0 for --json error, got: {}", output.status);
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["ok"], false);
}
```

**Step 3: Run tests**

Run: `cargo test -p sema-lang --test integration_test -- test_eval_ 2>&1 | tail -20`
Expected: All pass.

**Step 4: Commit**

```bash
git add crates/sema/src/main.rs crates/sema/tests/integration_test.rs
git commit -m "fix(eval): ensure --json mode exits 0 on eval errors"
```

---

### Task 7: Add `sema eval` to CLI documentation

**Files:**
- Modify: `website/docs/cli.md`

**Step 1: Add eval subcommand documentation**

Find the subcommands section and add:

```markdown
### `sema eval`

Evaluate Sema code and return results. Designed for machine consumption (editor/LSP integration).

```bash
# Evaluate an expression
sema eval --expr "(+ 1 2)"

# Read from stdin (avoids shell quoting issues)
echo '(* 6 7)' | sema eval --stdin

# JSON output for programmatic use
sema eval --expr "(+ 1 2)" --json

# Sandboxed evaluation (used by LSP)
echo '(define x 42) x' | sema eval --stdin --json --sandbox strict --no-llm
```

| Flag | Description |
|------|-------------|
| `--stdin` | Read program from stdin |
| `--expr <code>` | Evaluate a single expression |
| `--json` | Emit JSON result envelope |
| `--path <file>` | Set file context for imports and error spans |
| `--sandbox <mode>` | Sandbox mode (`strict`, `all`, or capability list) |
| `--no-llm` | Disable LLM features |
| `--timeout <ms>` | Kill evaluation after N ms (default: 5000) |
| `--vm` | Use bytecode VM instead of tree-walker |

The JSON envelope:

```json
{
  "ok": true,
  "value": "42",
  "stdout": "",
  "stderr": "",
  "error": null,
  "elapsedMs": 12
}
```
```

**Step 2: Commit**

```bash
git add website/docs/cli.md
git commit -m "docs: add sema eval subcommand to CLI reference"
```
