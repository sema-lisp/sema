# Error UX Improvements — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Transform Sema's error reporting from plain-text output into a polished, diagnostic-quality developer experience with colors, source context, and actionable guidance.

**Architecture:** All rendering changes live in `print_error()` in `crates/sema/src/main.rs` (the only consumer of `SemaError` for display). Core error types in `sema-core` gain optional source context fields. No new crate dependencies — use raw ANSI escape codes (the terminal module already does this) and `std::io::IsTerminal` (stable since Rust 1.70).

**Tech Stack:** Rust std only. ANSI SGR codes for color. `std::io::IsTerminal` for TTY detection.

---

## Task 1: Colorized Error Output

**Goal:** Make `print_error()` use ANSI colors so errors have visual hierarchy.

**Files:**
- Modify: `crates/sema/src/main.rs` — `print_error()` function (line ~1361)

**Step 1: Add a color helper module at the top of main.rs**

After the imports (line ~10), add a small inline color helper:

```rust
mod colors {
    use std::io::IsTerminal;

    fn enabled() -> bool {
        std::io::stderr().is_terminal()
            && std::env::var_os("NO_COLOR").is_none()
    }

    pub fn red_bold(s: &str) -> String {
        if enabled() { format!("\x1b[1;31m{s}\x1b[0m") } else { s.to_string() }
    }
    pub fn yellow(s: &str) -> String {
        if enabled() { format!("\x1b[33m{s}\x1b[0m") } else { s.to_string() }
    }
    pub fn cyan(s: &str) -> String {
        if enabled() { format!("\x1b[36m{s}\x1b[0m") } else { s.to_string() }
    }
    pub fn dim(s: &str) -> String {
        if enabled() { format!("\x1b[2m{s}\x1b[0m") } else { s.to_string() }
    }
    pub fn bold(s: &str) -> String {
        if enabled() { format!("\x1b[1m{s}\x1b[0m") } else { s.to_string() }
    }
}
```

**Step 2: Rewrite `print_error()`**

Replace the current `print_error` function:

```rust
fn print_error(e: &SemaError) {
    eprintln!("{} {}", colors::red_bold("Error:"), e.inner());
    if let Some(trace) = e.stack_trace() {
        for frame in &trace.0 {
            let loc = match (&frame.file, &frame.span) {
                (Some(file), Some(span)) => format!("{}:{span}", file.display()),
                (Some(file), None) => format!("{}", file.display()),
                (None, Some(span)) => format!("<input>:{span}"),
                (None, None) => String::new(),
            };
            eprintln!("  {} {} {}", colors::dim("at"), frame.name, colors::dim(&format!("({loc})")));
        }
    }
    if let Some(hint) = e.hint() {
        eprintln!("  {} {hint}", colors::cyan("hint:"));
    }
    if let Some(note) = e.note() {
        eprintln!("  {} {note}", colors::yellow("note:"));
    }
}
```

**Step 3: Verify manually**

Run: `cargo run -- -e "(pritnln 42)"` — should show red "Error:" prefix and cyan "hint:" prefix.
Run: `NO_COLOR=1 cargo run -- -e "(pritnln 42)"` — should show plain text (no escape codes).

**Step 4: Commit**

```bash
git add crates/sema/src/main.rs
git commit -m "feat: colorized error output with TTY detection"
```

---

## Task 2: Show Actual Value in Type Errors

**Goal:** Change `Type error: expected string, got integer` to `Type error: expected string, got integer (42)`.

**Files:**
- Modify: `crates/sema-core/src/error.rs` — `SemaError::Type` variant and `type_error` constructor (lines ~141, ~297)
- Modify: Call sites that construct type errors where the value is available

**Step 1: Add an optional `got_value` field to the Type variant**

Change the `Type` variant in `SemaError` enum:

```rust
#[error("Type error: expected {expected}, got {got}")]
Type {
    expected: String,
    got: String,
    got_value: Option<String>,
},
```

Update the `Display` impl (it's derived from thiserror, so update the `#[error(...)]` attribute):

```rust
#[error("Type error: expected {expected}, got {got}{}", got_value.as_ref().map(|v| format!(" ({v})")).unwrap_or_default())]
```

**Step 2: Update the `type_error` constructor**

The existing constructor stays unchanged (sets `got_value: None`):

```rust
pub fn type_error(expected: impl Into<String>, got: impl Into<String>) -> Self {
    SemaError::Type {
        expected: expected.into(),
        got: got.into(),
        got_value: None,
    }
}
```

Add a new constructor for when the value is available:

```rust
pub fn type_error_with_value(expected: impl Into<String>, got: impl Into<String>, value: &Value) -> Self {
    let display = format!("{value}");
    let truncated = if display.len() > 40 {
        format!("{}…", &display[..39])
    } else {
        display
    };
    SemaError::Type {
        expected: expected.into(),
        got: got.into(),
        got_value: Some(truncated),
    }
}
```

**Step 3: Update high-value call sites in the evaluator**

The most impactful call sites are in `crates/sema-eval/src/eval.rs` and `crates/sema-eval/src/special_forms.rs` where the offending `Value` is right there. Focus on the ones where the error message is most confusing without the value:

In `crates/sema-eval/src/eval.rs`, the keyword-as-function lookup (line ~337):
```rust
_ => Err(SemaError::type_error_with_value("map", args[0].type_name(), &args[0])),
```

And the "not callable" paths (lines ~341, ~639) — these already show the value via `format!`, so no change needed.

In `crates/sema-eval/src/special_forms.rs`, update the `eval_define_record_type` function's type errors where the pattern is `.ok_or_else(|| SemaError::type_error("symbol", args[0].type_name()))` and the value is `args[0]` — convert a few high-value ones:

```rust
.ok_or_else(|| SemaError::type_error_with_value("symbol", v.type_name(), v))
```

**Don't** update every single call site — just the ones in the evaluator hot path. The stdlib has hundreds of type_error calls; leave those for now.

**Step 4: Update tests**

In `crates/sema-core/src/error.rs` tests, add:

```rust
#[test]
fn type_error_with_value_display() {
    let e = SemaError::type_error_with_value("string", "integer", &Value::int(42));
    assert_eq!(e.to_string(), "Type error: expected string, got integer (42)");
}

#[test]
fn type_error_with_value_truncation() {
    let long_val = Value::string(&"x".repeat(100));
    let e = SemaError::type_error_with_value("int", "string", &long_val);
    assert!(e.to_string().contains("…"));
    assert!(e.to_string().len() < 120);
}
```

**Step 5: Verify & commit**

Run: `cargo test -p sema-core -- type_error`
Run: `cargo test` (full suite)

```bash
git add crates/sema-core/src/error.rs crates/sema-eval/src/eval.rs crates/sema-eval/src/special_forms.rs
git commit -m "feat: show actual value in type errors"
```

---

## Task 3: Source Line Display in Reader Errors

**Goal:** When a reader error occurs in a file, show the offending source line with a caret pointer. Example:

```
Error: Reader error at 3:15: unterminated string
  --> examples/app.sema:3:15
   |
 3 | (define name "hello
   |               ^ unterminated string
  hint: add a closing `"` to end the string
```

**Files:**
- Modify: `crates/sema/src/main.rs` — `print_error()` function
- No core changes needed — the `Reader` variant already has `span`, and the file path is available in the CLI context

**Step 1: Add a source-snippet helper to main.rs**

Add a function that reads a source line from a file or input string:

```rust
fn format_source_snippet(
    source: Option<&str>,
    file: Option<&std::path::Path>,
    span: &sema_core::Span,
) -> Option<String> {
    let lines: Vec<&str> = if let Some(src) = source {
        src.lines().collect()
    } else if let Some(path) = file {
        let content = std::fs::read_to_string(path).ok()?;
        return format_source_snippet(Some(&content), file, span);
    } else {
        return None;
    };

    let line_idx = span.line.checked_sub(1)?;
    let source_line = lines.get(line_idx)?;
    let col = span.col.saturating_sub(1);
    let line_num = span.line;
    let gutter_width = format!("{line_num}").len().max(2);
    let location = if let Some(path) = file {
        format!("{}:{line_num}:{}", path.display(), span.col)
    } else {
        format!("<input>:{line_num}:{}", span.col)
    };

    let mut out = String::new();
    out.push_str(&format!("  {} {}\n", colors::cyan("-->"), location));
    out.push_str(&format!("  {:>gutter_width$} {}\n", "", colors::cyan("|")));
    out.push_str(&format!(
        "  {:>gutter_width$} {} {}\n",
        colors::cyan(&line_num.to_string()),
        colors::cyan("|"),
        source_line
    ));
    out.push_str(&format!(
        "  {:>gutter_width$} {} {}{}",
        "",
        colors::cyan("|"),
        " ".repeat(col),
        colors::red_bold("^")
    ));
    Some(out)
}
```

**Step 2: Update `print_error()` to use snippets for Reader errors**

After printing the main error line, check if it's a `Reader` error and try to show the snippet:

```rust
fn print_error(e: &SemaError) {
    let inner = e.inner();
    eprintln!("{} {}", colors::red_bold("Error:"), inner);

    // For reader errors, show source snippet
    if let SemaError::Reader { span, .. } = inner {
        // Try to get source from current file (passed via a thread-local or global)
        if let Some(snippet) = format_source_snippet(None, CURRENT_FILE.get(), span) {
            eprintln!("{snippet}");
        }
    }
    // ... rest of trace/hint/note display
}
```

The challenge: `print_error` doesn't have access to the source code or file path. Two approaches:

**Approach A (simpler):** Store the last-evaluated source string and file path in a thread-local, set before eval, read in `print_error`. This is what we should do:

```rust
thread_local! {
    static LAST_SOURCE: RefCell<Option<String>> = RefCell::new(None);
    static LAST_FILE: RefCell<Option<PathBuf>> = RefCell::new(None);
}
```

Set these before calling `eval_with_mode` / `eval_str` in the file-execution and REPL paths.

**Approach B (cleaner but bigger):** Add an optional `source: Option<String>` to the `Reader` error variant. This would require changing every place that creates a `Reader` error. Skip this approach.

Go with Approach A. In the file-execution path (line ~457), before eval:
```rust
LAST_SOURCE.set(Some(content.clone()));
LAST_FILE.set(Some(PathBuf::from(file)));
```

In the REPL path (line ~1448), before eval:
```rust
LAST_SOURCE.set(Some(input.clone()));
LAST_FILE.set(None);
```

**Step 3: Test manually**

Create a test file with a syntax error:
```bash
echo '(define x "hello' > /tmp/test-err.sema
cargo run -- /tmp/test-err.sema
```

Should show the source line with caret.

**Step 4: Commit**

```bash
git add crates/sema/src/main.rs
git commit -m "feat: show source line with caret in reader errors"
```

---

## Task 4: Source Context in Eval Errors (Stack Trace Frames)

**Goal:** When a stack trace frame has a file + span, show the source line for the innermost frame. This extends Task 3's infrastructure to eval errors.

**Files:**
- Modify: `crates/sema/src/main.rs` — extend `print_error()` to show source for the first stack trace frame

**Step 1: Extend `print_error()` to show source for innermost frame**

After printing the error message and before the stack trace, check if the innermost (first) frame has a file + span:

```rust
if let Some(trace) = e.stack_trace() {
    if let Some(first_frame) = trace.0.first() {
        if let (Some(file), Some(span)) = (&first_frame.file, &first_frame.span) {
            if let Some(snippet) = format_source_snippet(None, Some(file), span) {
                eprintln!("{snippet}");
            }
        }
    }
    // Then print the rest of the trace as before
    for frame in &trace.0 { ... }
}
```

Also fall back to LAST_SOURCE/LAST_FILE thread-locals when the frame has a span but no file (REPL case).

**Step 2: Test manually**

```bash
cargo run -- -e '(define (foo x) (+ x "hello")) (foo 42)'
```

Should show the source line where the error occurs.

**Step 3: Commit**

```bash
git add crates/sema/src/main.rs
git commit -m "feat: show source context in eval error stack traces"
```

---

## Task 5: Better Stack Overflow Message

**Goal:** Change `maximum eval depth exceeded (1024)` to include a hint about infinite recursion and tail calls.

**Files:**
- Modify: `crates/sema-eval/src/eval.rs` — the depth check (line ~302-306)

**Step 1: Add hint to the depth error**

Change:
```rust
return Err(SemaError::eval(format!(
    "maximum eval depth exceeded ({MAX_EVAL_DEPTH})"
)));
```

To:
```rust
return Err(SemaError::eval(format!(
    "maximum eval depth exceeded ({MAX_EVAL_DEPTH})"
)).with_hint("this usually means infinite recursion; ensure recursive calls are in tail position for TCO, or use 'do' for iteration"));
```

**Step 2: Add test**

In `crates/sema/tests/integration_test.rs`, find the existing stack overflow test (or add one):

```rust
#[test]
fn test_stack_overflow_hint() {
    let interp = Interpreter::new();
    let err = interp.eval_str("(define (loop) (+ 1 (loop))) (loop)").unwrap_err();
    assert!(err.hint().is_some());
    assert!(err.hint().unwrap().contains("recursion"));
}
```

**Step 3: Verify & commit**

Run: `cargo test -p sema-lang --test integration_test -- stack_overflow`

```bash
git add crates/sema-eval/src/eval.rs crates/sema/tests/integration_test.rs
git commit -m "feat: add hint to stack overflow error"
```

---

## Task 6: REPL `,type` and `,time` Commands

**Goal:** Add `,type expr` (evaluate and show type) and `,time expr` (evaluate and show elapsed time) to the REPL.

**Files:**
- Modify: `crates/sema/src/main.rs` — REPL command handling (line ~1406) and `REPL_COMMANDS` constant (line ~16)

**Step 1: Add commands to REPL_COMMANDS**

```rust
const REPL_COMMANDS: &[&str] = &[",quit", ",exit", ",q", ",help", ",h", ",env", ",builtins", ",type", ",time"];
```

**Step 2: Add command handlers in the REPL match block**

After the `,builtins` handler (line ~1420), add:

```rust
_ if trimmed.starts_with(",type ") => {
    let expr = &trimmed[6..];
    match eval_with_mode(&interpreter, expr, use_vm) {
        Ok(val) => {
            let type_name = match val.view() {
                ValueView::Record(r) => format!(":{}", sema_core::resolve(r.type_tag)),
                _ => format!(":{}", val.type_name()),
            };
            println!("{}", colors::dim(&type_name));
        }
        Err(e) => print_error(&e),
    }
    continue;
}
_ if trimmed.starts_with(",time ") => {
    let expr = &trimmed[6..];
    let start = std::time::Instant::now();
    match eval_with_mode(&interpreter, expr, use_vm) {
        Ok(val) => {
            let elapsed = start.elapsed();
            if !val.is_nil() {
                println!("{}", pretty_print(&val, 80));
            }
            eprintln!("{} {elapsed:.3?}", colors::dim("elapsed:"));
        }
        Err(e) => {
            let elapsed = start.elapsed();
            print_error(&e);
            eprintln!("{} {elapsed:.3?}", colors::dim("elapsed:"));
        }
    }
    continue;
}
```

**Step 3: Update `,help` output**

In `print_help()`, add:
```rust
println!("  ,type EXPR    Show the type of a value");
println!("  ,time EXPR    Evaluate and show elapsed time");
```

**Step 4: Verify manually**

```bash
cargo run
sema> ,type 42
:integer
sema> ,type '(1 2 3)
:list
sema> ,time (foldl + 0 (range 10000))
49995000
elapsed: 12.345ms
```

**Step 5: Commit**

```bash
git add crates/sema/src/main.rs
git commit -m "feat: add ,type and ,time REPL commands"
```

---

## Task 7: Builtin Shadowing Warning

**Goal:** When `define` or `set!` assigns to a name that shadows a builtin, print a warning (not an error). Only in interactive/REPL mode.

**Files:**
- Modify: `crates/sema-eval/src/special_forms.rs` — `eval_define()` function
- Modify: `crates/sema-core/src/context.rs` — add a flag for interactive mode

**Step 1: Add an `interactive` flag to EvalContext**

In `crates/sema-core/src/context.rs`, add to `EvalContext`:

```rust
pub interactive: Cell<bool>,
```

Initialize to `false` in both `new()` and `new_with_sandbox()`.

**Step 2: Set the flag in the REPL**

In `crates/sema/src/main.rs`, at the start of `repl()`:

```rust
interpreter.ctx.interactive.set(true);
```

**Step 3: Add shadowing detection to `eval_define`**

In `crates/sema-eval/src/special_forms.rs`, in `eval_define()`, after determining the name to bind but before setting it, check if the name already exists as a native function in a parent env:

```rust
// Warn if shadowing a builtin (interactive mode only)
if ctx.interactive.get() {
    if let Some(existing) = env.get(name_spur) {
        if existing.as_native_fn_rc().is_some() {
            eprintln!("  {} redefining builtin '{}'", "warning:", resolve(name_spur));
        }
    }
}
```

This is a simple eprintln warning — not an error, doesn't change behavior. Uses the `colors` module if we want to make it yellow (but colors is in the `sema` crate, not `sema-eval`). For now, just use plain text since `sema-eval` doesn't have terminal awareness. Alternatively, use a callback pattern or just keep it simple.

**Step 4: Test**

In the integration tests:

```rust
#[test]
fn test_define_shadows_builtin_no_error() {
    // Shadowing should work fine, just with a warning
    let interp = Interpreter::new();
    assert_eq!(
        interp.eval_str("(define map 42) map").unwrap(),
        Value::int(42)
    );
}
```

**Step 5: Commit**

```bash
git add crates/sema-core/src/context.rs crates/sema-eval/src/special_forms.rs crates/sema/src/main.rs
git commit -m "feat: warn when redefining builtins in REPL"
```

---

## Task 8: Mismatched Bracket Detection

**Goal:** When you write `(define x [1 2 3)`, detect that `)` doesn't match `[` and give a specific error instead of a generic "expected `]`".

**Files:**
- Modify: `crates/sema-reader/src/reader.rs` — the list/vector/map parsing functions

**Step 1: Improve error messages in `parse_list`**

In `crates/sema-reader/src/reader.rs`, the `parse_list` function (which handles `(...)`) currently errors with "add a closing `)`" when it hits EOF. But when it hits a `]` or `}` instead of `)`, it should say:

```rust
Some(Token::RBracket) => {
    return Err(SemaError::Reader {
        message: "mismatched bracket: found `]` but expected `)`".to_string(),
        span: self.span(),
    }.with_hint("this list was opened with `(` — close it with `)`"));
}
Some(Token::RBrace) => {
    return Err(SemaError::Reader {
        message: "mismatched bracket: found `}` but expected `)`".to_string(),
        span: self.span(),
    }.with_hint("this list was opened with `(` — close it with `)`"));
}
```

Apply the same pattern for `parse_vector` (expects `]`, errors on `)` or `}`) and `parse_map` (expects `}`, errors on `)` or `]`).

Find the existing parse functions in `reader.rs`. They currently loop until they find the closing delimiter. Add checks at the point where they process tokens to detect wrong-type closers.

**Step 2: Add test**

In `crates/sema-reader/src/reader.rs` tests (or `crates/sema/tests/integration_test.rs`):

```rust
#[test]
fn test_mismatched_bracket() {
    let err = sema_reader::read("(define x [1 2 3)").unwrap_err();
    assert!(err.to_string().contains("mismatched"));
}
```

**Step 3: Commit**

```bash
git add crates/sema-reader/src/reader.rs
git commit -m "feat: detect mismatched bracket types in reader"
```

---

## Task 9: Arity Error Enhancement — Show the Call Form

**Goal:** For arity errors in the evaluator, include the call form in a note so the user can see what they actually wrote.

**Files:**
- Modify: `crates/sema-eval/src/eval.rs` — `apply_lambda()` (line ~650) and `call_value()` (line ~321)
- Modify: `crates/sema-core/src/error.rs` — optional `call_form` field on Arity variant

**Step 1: Add `call_form` to the Arity variant**

In `crates/sema-core/src/error.rs`:

```rust
#[error("Arity error: {name} expects {expected} args, got {got}{}",
    call_form.as_ref().map(|f| format!("\n  in: {f}")).unwrap_or_default())]
Arity {
    name: String,
    expected: String,
    got: usize,
    call_form: Option<String>,
},
```

Update the `arity()` constructor to set `call_form: None`, and add:

```rust
pub fn arity_with_form(name: impl Into<String>, expected: impl Into<String>, got: usize, form: impl Into<String>) -> Self {
    SemaError::Arity {
        name: name.into(),
        expected: expected.into(),
        got,
        call_form: Some(form.into()),
    }
}
```

**Step 2: Use it in key call sites**

In `eval_value_inner` where list expressions are evaluated and arity is checked (the `apply_lambda` call sites), wrap the arity error to include the original expression:

This is optional and can be done incrementally. The key win is just having the infrastructure. Start with `apply_lambda` — when it returns an arity error, the caller in `eval_value_inner` can attach the call form via `.with_note(format!("in: {expr}"))`.

**Step 3: Update check_arity macro to set `call_form: None`**

In the `check_arity!` macro, add `call_form: None` to the `SemaError::Arity` construction.

**Step 4: Verify & commit**

Run: `cargo test`

```bash
git add crates/sema-core/src/error.rs crates/sema-eval/src/eval.rs
git commit -m "feat: arity errors can include call form context"
```

---

## Task 10: REPL `,doc` Command

**Goal:** Add `,doc fn-name` that shows the arity and a brief description for builtin functions.

**Files:**
- Modify: `crates/sema-core/src/value.rs` — add optional `doc` field to `NativeFn`
- Modify: `crates/sema/src/main.rs` — add `,doc` command handler

**Step 1: Add `doc` field to NativeFn**

In `crates/sema-core/src/value.rs`, find `NativeFn` struct and add:

```rust
pub doc: Option<&'static str>,
```

Update `NativeFn::simple()` and `NativeFn::with_ctx()` to set `doc: None`.

**Step 2: Add `,doc` command**

In the REPL, look up the name in the environment and display its type and doc:

```rust
_ if trimmed.starts_with(",doc ") => {
    let name = trimmed[5..].trim();
    let spur = sema_core::intern(name);
    match env.get(spur) {
        Some(val) => {
            println!("  {name} : {}", val.type_name());
            if let Some(native) = val.as_native_fn_rc() {
                if let Some(doc) = native.doc {
                    println!("  {doc}");
                }
            }
        }
        None => {
            if SPECIAL_FORM_NAMES.contains(&name) {
                println!("  {name} : special form");
            } else {
                eprintln!("  not found: {name}");
            }
        }
    }
    continue;
}
```

This is useful even without doc strings — it tells you the type (native-fn, lambda, integer, etc.) and confirms the binding exists.

Doc strings can be added to individual functions incrementally over time. Don't add doc strings to all 460+ functions now — that's a separate project.

**Step 3: Commit**

```bash
git add crates/sema-core/src/value.rs crates/sema/src/main.rs
git commit -m "feat: add ,doc REPL command"
```

---

## Execution Order & Dependencies

```
Task 1 (colors)          — independent, do first (all other tasks benefit)
Task 2 (type error vals) — independent of 1, can be parallel
Task 3 (source snippets) — depends on Task 1 (uses colors)
Task 4 (eval source ctx) — depends on Task 3 (extends snippet infra)
Task 5 (stack overflow)  — independent, trivial
Task 6 (REPL commands)   — depends on Task 1 (uses colors)
Task 7 (shadow warning)  — independent, can be parallel
Task 8 (bracket mismatch)— independent, can be parallel
Task 9 (arity call form) — independent, touches error.rs
Task 10 (,doc command)   — independent, can be done anytime
```

**Recommended parallel groups:**

1. **First:** Task 1 (colors) — everything else looks better with this
2. **Parallel batch:** Tasks 2, 5, 7, 8 (all independent, touch different files)
3. **Sequential:** Task 3, then Task 4 (source snippets build on each other)
4. **Final batch:** Tasks 6, 9, 10 (REPL polish + arity enhancement)
