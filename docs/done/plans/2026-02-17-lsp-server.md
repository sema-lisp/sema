# Sema Developer Experience — Design Document (v3)

**Date:** 2026-02-18 (revised)
**Status:** Draft
**Supersedes:** `2026-02-16-lsp-server.v1.md`
**Implementation:** Phase 0a complete (special forms export)
**Goal:** World-class developer experience — tree-sitter grammar for instant structural editing + full LSP for semantic features.

## Overview

Two complementary systems:

1. **`tree-sitter-sema`** — A dedicated tree-sitter grammar for Sema. Currently Helix piggybacks on the Scheme grammar (`grammar = "scheme"`), which doesn't understand Sema-specific syntax (keywords `:foo`, hash maps `{}`, vectors `[]`, `#t`/`#f` booleans, block comments `#| |#`, etc.). A proper grammar gives every tree-sitter-native editor (Neovim, Helix, Zed, Emacs 29+) accurate highlighting, folding, indentation, and text objects — instantly, with no server.

2. **`sema-lsp`** — A Language Server Protocol server providing diagnostics, completions, go-to-definition, and hover. Uses `sema-reader` for parsing and `sema-eval::Interpreter` for semantic analysis under the sandbox system (`Sandbox::deny(Caps::ALL)`). The bytecode VM pipeline (`sema_vm::lower` → `resolve` → `compile`) can optionally be used for deeper static analysis (Phase 1b) — it catches unbound variables and arity errors without executing code. The VM pipeline now includes recursion depth limits (256) to safely handle malicious input.

## Prerequisites (Phase 0)

Before starting either system:

1. ~~**Unify the special-forms list.**~~ ✅ **Done.** `SPECIAL_FORM_NAMES: &[&str]` is now exported from `sema_eval::special_forms` (re-exported from `sema_eval`). The duplicate `SPECIAL_FORMS` in `main.rs` has been removed — the REPL now uses `sema_eval::SPECIAL_FORM_NAMES`. The canonical list has 38 entries matching `SpecialFormSpurs`. Note: the VM lowerer (`sema-vm/src/lower.rs`) still has its own ad-hoc `sf("name")` calls — these can't easily share the const since they produce `Spur` values, but they match the canonical list.

2. **Add end positions to `Span`** (recommended, not blocking Phase 1). Currently `Span { line, col }` is start-only. Adding `end_line`/`end_col` improves diagnostic underlines and is required for precise go-to-definition highlighting in Phase 3+. **Note:** Symbols are interned as `Spur` values and do not carry individual source spans. The `SpanMap` only tracks compound expressions (lists, vectors, maps) by `Rc` pointer address. This is a known limitation — it means top-level unbound variable errors lack line/col info for bare symbols. For the LSP, this affects Phase 3 (go-to-definition for symbol references) but not Phases 1–2.

---

## Tree-sitter Grammar (`tree-sitter-sema`)

### Why a Dedicated Grammar

Sema extends standard Scheme syntax with:
- Keywords: `:foo`, `:bar`
- Hash map literals: `{:key value}`
- Vector literals: `[1 2 3]`
- Block comments: `#| ... |#`
- String escapes: `\n`, `\t`, `\\`, `\"`
- Boolean literals: `#t`, `#f` (plus `true`/`false` as symbols)
- Dot notation in symbols: `record.field`

The Scheme tree-sitter grammar doesn't parse these correctly. A custom `tree-sitter-sema` grammar (~100-150 lines of `grammar.js`) handles all of them.

### Deliverables

- `tree-sitter-sema/` repository (or subdirectory under `editors/`)
- `grammar.js` defining: atoms (int, float, string, char, boolean, keyword, symbol), lists `()`, vectors `[]`, maps `{}`, quote/unquote/quasiquote/splice, comments (line `;` and block `#| |#`)
- Query files: `highlights.scm`, `indents.scm`, `textobjects.scm`, `folds.scm`
- Published to npm (for Neovim/Helix/Zed consumption) and as a Rust crate
- Updated editor configs: Helix switches from `grammar = "scheme"` to `grammar = "sema"`, Neovim gets tree-sitter config, Zed gets language extension

### What This Enables (No LSP Required)

| Feature | Editor Support |
|---------|---------------|
| Accurate syntax highlighting | Neovim, Helix, Zed, Emacs 29+ |
| Code folding | All tree-sitter editors |
| Smart indentation | All tree-sitter editors |
| Structural text objects (`af` = around function, `if` = inside function) | Neovim, Helix |
| Incremental select (expand/shrink selection by AST node) | Neovim, Helix, Zed |
| Syntax-aware commenting | All tree-sitter editors |

### Complexity

**Easy–Medium.** S-expression grammars are among the simplest to write for tree-sitter. Existing `tree-sitter-sexp`, `tree-sitter-commonlisp`, and `tree-sitter-clojure` grammars serve as references. The query files can be adapted from the existing Helix `.scm` files (which are already well-structured with 300+ lines of Sema-specific patterns).

---

## Crate Structure

### New Crate: `crates/sema-lsp/`

```toml
[package]
name = "sema-lsp"
version = "0.1.0"
edition = "2021"

[dependencies]
sema-core.workspace = true
sema-reader.workspace = true
sema-eval.workspace = true
tower-lsp = "0.20"
tokio = { workspace = true, features = ["rt-multi-thread", "macros", "io-std"] }
dashmap = "6"
```

Note: **no direct dependency on `sema-stdlib` or `sema-vm`**. Builtins are accessed through `Interpreter::new_with_sandbox()` which registers stdlib internally. This avoids coupling to `register_stdlib`'s signature.

### Binary Entry Point

Add an `Lsp` variant to `Commands` in `crates/sema/src/main.rs`:

```rust
#[derive(Subcommand)]
enum Commands {
    Ast { /* ... */ },
    Completions { /* ... */ },
    /// Start the Language Server Protocol server
    Lsp,
}
```

The handler calls `sema_lsp::run_server().await`.

### Dependency Flow

```
sema-core ← sema-reader
    ↑           ↑
sema-vm    sema-eval ← sema-stdlib
    ↑           ↑
    └───────────┘
                ↑
            sema-lsp
                ↑
            sema (binary)

(sema-wasm is a separate WASM target, not involved)
```

`sema-lsp` depends on `sema-core`, `sema-reader`, and `sema-eval`. It does **not** depend on `sema-vm`, `sema-stdlib`, or `sema-llm`.

### Threading Model

`tower-lsp` is async (tokio). Sema's evaluator is single-threaded (`Rc`, not `Arc`). The LSP runs a **dedicated backend thread** that owns all `Rc` state:

- One `Interpreter` instance (sandboxed, no IO/network/shell/LLM)
- Cached builtin/special-form name lists
- Parsed ASTs and span maps for open documents

Async LSP handlers send requests to the backend via a channel and await responses on a oneshot. This is cleaner than scattered `spawn_blocking` calls once multiple features share state.

```
┌─────────────────────┐     channel      ┌──────────────────────┐
│  tower-lsp async    │ ──── Request ──→ │  Backend thread      │
│  handlers           │ ←── Response ─── │  (owns Interpreter,  │
│  (Send, tokio)      │     (oneshot)    │   Rc state, caches)  │
└─────────────────────┘                  └──────────────────────┘
```

Only `Send`-safe types cross the channel: `String`, `Url`, LSP structs, `Vec<Diagnostic>`, etc.

---

## Phase 1: Parse Diagnostics

**Complexity:** Easy
**Reuses:** `sema_reader::read_many_with_spans`, `SemaError` (spans, hints, notes)

### How It Works

1. On `textDocument/didOpen` and `textDocument/didChange`, send the full document text to the backend thread.
2. Backend calls `sema_reader::read_many_with_spans(&text)`.
3. On `Ok(...)` → return empty diagnostics.
4. On `Err(e)` → convert to LSP `Diagnostic` and return.

### Document State

Track open documents in a `DashMap<Url, String>` on the async side (for quick access) and forward text to the backend for analysis.

### Span Conversion

The reader's `Span { line, col }` is 1-indexed. Convert to LSP 0-indexed:

```rust
fn span_to_lsp_range(span: &Span) -> lsp_types::Range {
    let pos = Position {
        line: span.line.saturating_sub(1) as u32,
        character: span.col.saturating_sub(1) as u32,
    };
    Range { start: pos, end: pos } // point range until end spans exist
}
```

### Error Conversion

`SemaError` supports `.hint()` and `.note()` via `WithContext`. Append these to the diagnostic message:

```rust
fn error_to_diagnostic(err: &SemaError) -> Diagnostic {
    let range = match err.span() {
        Some(span) => span_to_lsp_range(span),
        None => Range::default(),
    };
    let mut message = err.inner().to_string();
    if let Some(hint) = err.hint() {
        message.push_str(&format!("\nhint: {hint}"));
    }
    if let Some(note) = err.note() {
        message.push_str(&format!("\nnote: {note}"));
    }
    Diagnostic {
        range,
        severity: Some(DiagnosticSeverity::ERROR),
        source: Some("sema".into()),
        message,
        ..Default::default()
    }
}
```

### Error Recovery

`read_many_with_spans` stops at the first parse error. For Phase 1, this means one diagnostic at a time. Consider adding `read_many_with_spans_recover` to `sema-reader` that skips to the next top-level form on error, returning `(Vec<Value>, SpanMap, Vec<SemaError>)`. This is standard for LSP servers and not blocking for Phase 1.

### `didClose` Handling

Remove document from store and publish empty diagnostics to clear stale errors.

### Deliverables

- Real-time red squiggles for unclosed parens, unterminated strings, invalid tokens
- Error messages include hints ("Did you mean...?") and notes
- Pure parse — fast, no side effects

---

## Phase 2: Completion

**Complexity:** Medium
**Reuses:** `Interpreter` (global env bindings), `SPECIAL_FORM_NAMES` (from Phase 0 prerequisite)

### Completion Sources

1. **Special forms** — from the exported `SPECIAL_FORM_NAMES` const. Kind: `Keyword`.
2. **Stdlib builtins** — enumerate `interpreter.global_env` bindings where value is `NativeFn`. Kind: `Function`.
3. **User definitions** — walk the parsed AST for top-level `define`/`defun`/`defmacro` forms. Kind: `Variable`/`Function`/`Keyword` respectively.

### Builtin Collection

On backend thread startup:

```rust
let interpreter = Interpreter::new_with_sandbox(&Sandbox::deny(Caps::ALL));
let builtins: Vec<String> = interpreter.global_env
    .all_binding_names()  // or manual walk of bindings HashMap
    .collect();
```

This runs once. The builtin list is static for the server's lifetime.

### Trigger and Prefix Handling

Register `completion_provider` with trigger characters `(` and space. The prefix parser must treat `/` as part of the symbol name (Sema uses `/` for namespacing: `string/trim`, `map/get`).

### Scope Limitations

Phase 2 only handles top-level definitions. `let`/`let*`/`letrec` bindings are not visible. Scope-aware completion is Phase 3+.

---

## Phase 3: Go to Definition

**Complexity:** Hard
**Reuses:** `SpanMap`, module resolution logic, `read_many_with_spans`

### 3a. Import/Load Path Resolution (Easy)

When cursor is on a string in `(import "path")` or `(load "path")`:
- Resolve path relative to current file (same logic as evaluator's module loader)
- Return `Location` pointing to the resolved file, line 0

**Note:** Path resolution logic is currently duplicated between tree-walker (`special_forms.rs`) and VM delegates (`eval.rs`). Consider extracting a shared helper before implementing this.

### 3b. User-Defined Symbols (Medium)

For `define`/`defun` symbols:
- Parse file with `read_many_with_spans`
- Walk top-level forms to find the binding
- Use `SpanMap` to look up the span (keyed by `Rc` pointer address via `Rc::as_ptr as usize`)

**Important:** The `SpanMap` requires the exact `Value` pointers from parsing — no cloning. The lookup must traverse the parsed AST and read `Rc` pointers. This is fragile but functional for single-file analysis.

### 3c. Cross-File Module Definitions (Hard)

Resolve imported symbol to its definition in another file. Requires parsing target module and tracking exports.

### Symbol-Under-Cursor

Determine what symbol the cursor is on via character-class scan (backward/forward from cursor to find symbol boundaries delimited by whitespace and parens). Simple and robust for Lisp.

### End Spans

Phase 3 strongly benefits from `Span` having `end_line`/`end_col`. Without end spans, go-to-definition returns a point position (editors highlight the word at that position, which works but is imprecise).

---

## Phase 4: Hover Documentation

**Complexity:** Medium–Hard
**Reuses:** `NativeFn` (name), `Lambda` (params), `SpanMap`

### 4a. Builtin Documentation

Ship a static `HashMap<&str, &str>` of name→doc in `sema-lsp`. Source docs from the website at `sema-lang.com/docs/`. This avoids modifying `NativeFn` or touching all 350+ registration calls across 19 stdlib modules.

Future: add `doc: Option<String>` to `NativeFn` for inline docs.

### 4b. User Function Signatures

For `(defun name (x y z) ...)`, extract and display the parameter list from the parsed AST.

### 4c. Format

Return Markdown hover content with `sema` fenced code blocks (the TextMate grammar provides syntax highlighting in VS Code hover popups).

---

## Editor Integration

### VS Code (`editors/vscode/sema/`)

The extension currently only provides TextMate grammar. Add LSP client:

1. Add `vscode-languageclient` dependency
2. Create `extension.ts` entry point that spawns `sema lsp`
3. Add `tsconfig.json` and build step (`esbuild` recommended)
4. Update `package.json`: `activationEvents`, `main` field

### Other Editors

- **Neovim:** `nvim-lspconfig` entry pointing to `sema lsp`
- **Helix:** `[[language]]` in `languages.toml` with `command = "sema"`, `args = ["lsp"]`
- **Emacs:** `eglot` config
- **Zed:** `settings.json` LSP configuration

---

## Testing Strategy

### Unit Tests (in `sema-lsp`)

- `span_to_lsp_range` conversion edge cases (line 1 col 1 → 0,0)
- `error_to_diagnostic` with Reader errors, WithContext, WithTrace variants
- Builtin name collection returns non-empty list with known names
- Definition extraction from various AST shapes

### Integration Tests

- In-process LSP via `tower_lsp::LspService::new()`: initialize → didOpen with bad source → assert diagnostics
- Completion results for partial prefixes
- Go-to-definition for top-level `defun`

### Manual Checklist

- [ ] `sema lsp` starts without crash
- [ ] VS Code shows squiggles for `(define x` (unclosed paren)
- [ ] Fixing error clears squiggles
- [ ] Completions appear for `str` → `string/trim`, `string-append`, etc.
- [ ] Multiple files open simultaneously

---

## Phase R: Runnable Regions — Evaluate Code from the Editor

**Complexity:** Medium
**Depends on:** Phase 0b (end spans), Phase 1 (LSP plumbing)
**Reuses:** `sema` CLI subprocess, `read_many_with_spans`, doctest detection from Living Code

### Overview

Enable evaluating individual top-level forms, selections, and doctests directly from the editor with inline result display. The LSP handles code lens + command dispatch; actual evaluation is delegated to a `sema` subprocess for isolation.

### Architecture: LSP ≠ Evaluator

The LSP server **never evaluates user code**. It delegates to a `sema eval` subprocess:

```
┌──────────────┐  CodeLens / executeCommand  ┌──────────────┐
│   Editor     │ ◄──────────────────────────► │  sema-lsp    │
│  (VS Code,   │                              │  (analysis   │
│   Zed, etc.) │  sema/evalResult (notify)    │   only)      │
└──────────────┘ ◄──────────────────────────  └──────┬───────┘
                                                     │ spawn
                                                     ▼
                                              ┌──────────────┐
                                              │ sema eval    │
                                              │ --stdin      │
                                              │ --json       │
                                              │ (subprocess) │
                                              └──────────────┘
```

**Why not evaluate inside the LSP?**
- The LSP backend thread owns an `Interpreter` under `Sandbox::deny(Caps::ALL)` for analysis. Mixing in interactive evaluation would block diagnostics/completions, complicate cancellation, and create pressure to relax sandboxing.
- `Rc`-based values can't cross thread boundaries — subprocess isolation avoids this entirely.
- Subprocess model gives clean timeouts, cancellation, and capability control.

### New CLI: `sema eval`

Add a dedicated `eval` subcommand optimized for machine consumption:

```bash
# Read program from stdin, emit JSON result envelope
sema eval --stdin --json --path /abs/file.sema

# With sandbox + no LLM (default for LSP-triggered evals)
sema eval --stdin --json --sandbox strict --no-llm --path /abs/file.sema
```

**Arguments:**
- `--stdin` — read program text from stdin (avoids shell quoting issues)
- `--json` — emit machine-readable result envelope (single JSON object on stdout)
- `--path <file>` — set "current file" for error spans + relative import resolution
- `--sandbox <mode>` — sandbox mode (reuses existing `--sandbox` logic)
- `--no-llm` / `--no-init` — disable LLM features
- `--timeout <ms>` — kill after N milliseconds (default: 5000)

**JSON output envelope:**

```jsonc
{
  "ok": true,
  "value": "42",
  "stdout": "hello world\n",
  "stderr": "",
  "error": null,
  "elapsedMs": 12
}
```

On error:

```jsonc
{
  "ok": false,
  "value": null,
  "stdout": "",
  "stderr": "",
  "error": {
    "message": "Unbound variable: foo",
    "hint": "Did you mean 'for'?",
    "line": 3,
    "col": 5
  },
  "elapsedMs": 2
}
```

### LSP Features

#### CodeLens: `textDocument/codeLens`

Walk the parsed AST for each open document and return one lens stub per top-level form:

| Form type | Lens title | Command |
|-----------|-----------|---------|
| Any top-level form | `▶ Run` | `sema.runTopLevel` |
| `defn`/`defun` with doctests | `▶ Run Doctests` | `sema.runDoctests` |
| `defagent` | `▶ Run Agent` | `sema.runTopLevel` |

**Stub payload (in `data` field):**

```jsonc
{
  "uri": "file:///path/to/file.sema",
  "kind": "run",           // "run" | "doctests"
  "formIndex": 3,          // index into top-level forms
  "range": { "start": { "line": 5, "character": 0 }, "end": { "line": 12, "character": 1 } },
  "docVersion": 17         // document version for staleness check
}
```

**`codeLens/resolve`:** Attach the command title + arguments based on `data.kind`.

#### Execute Command: `workspace/executeCommand`

**Commands advertised during `initialize`:**

| Command | Arguments | Description |
|---------|-----------|-------------|
| `sema.runTopLevel` | `[LensData]` | Evaluate a top-level form with its context |
| `sema.runDoctests` | `[LensData]` | Run doctests for a function |
| `sema.evalRange` | `[uri, range]` | Evaluate arbitrary selected text |

**Execution flow for `sema.runTopLevel`:**

1. Look up document text from the open document store.
2. Parse with `read_many_with_spans` to get top-level forms.
3. Construct the evaluation program: **all prior forms `[0..i)` as context** + **target form `[i]`**.
4. Spawn `sema eval --stdin --json --sandbox strict --no-llm --path <file>`.
5. Pipe the constructed program to stdin, capture stdout.
6. Parse JSON result envelope.
7. Send `sema/evalResult` notification to client.

**Context construction (Phase R1 — simple, stateless):**

```
;; forms 0 through i-1 (establish definitions, imports)
(import "utils.sema")
(define pi 3.14159)
(defn area (r) (* pi r r))

;; target form (form i — its return value is the result)
(area 5)
```

This re-runs prefix forms on every evaluation. Acceptable for Phase R1; a long-lived REPL session (Phase R2) avoids this.

#### Custom Notification: `sema/evalResult`

**Method:** `"sema/evalResult"`

```jsonc
{
  "uri": "file:///path/to/file.sema",
  "range": { "start": {...}, "end": {...} },
  "kind": "run",
  "value": "78.53975",
  "stdout": "",
  "stderr": "",
  "ok": true,
  "elapsedMs": 15,
  "taskId": "uuid"
}
```

For doctests:

```jsonc
{
  "uri": "...",
  "range": {...},
  "kind": "doctests",
  "value": null,
  "ok": true,
  "stdout": "area: 3/3 ✓\n",
  "stderr": "",
  "elapsedMs": 42,
  "taskId": "uuid"
}
```

### VS Code Extension Changes

The current `editors/vscode/sema/` is grammar-only. Add a TypeScript extension:

**New file layout:**

```
editors/vscode/sema/
  package.json          # updated: add main, activationEvents, commands, keybindings
  tsconfig.json         # new
  src/
    extension.ts        # new: activation, LSP client setup, output channel
    lspClient.ts        # new: spawn `sema lsp`, register notification handlers
    evalDecorations.ts  # new: inline `=> value` decorations via VS Code API
    commands.ts         # new: evalForm, evalSelection, clearResults
```

**Keybindings:**

| Keybinding | Command | Description |
|-----------|---------|-------------|
| `Ctrl+Enter` | `sema.evalForm` | Evaluate current top-level form |
| `Shift+Enter` | `sema.evalSelection` | Evaluate selection (or form at cursor) |
| `Ctrl+Shift+Backspace` | `sema.clearResults` | Clear all inline result decorations |

**Inline result display:**

Uses VS Code's `TextEditorDecorationType` with `after:` render options:

```typescript
const resultDecoration = vscode.window.createTextEditorDecorationType({
  after: { textDecoration: 'none', fontStyle: 'italic' },
  rangeBehavior: vscode.DecorationRangeBehavior.ClosedOpen,
});

// Applied per-result:
{ renderOptions: { after: { contentText: ' => 78.53975', color: '#88c070' } } }
```

Results are truncated to ~120 chars inline; full output goes to the "Sema" output channel.

**Output channel:**

```
[file.sema:7] ▶ (area 5)
=> 78.53975 (15ms)

[file.sema:1-5] ▶ Run Doctests: area
area: 3/3 ✓ (42ms)
```

### Zed Integration

Extend `editors/zed/languages/sema/runnables.scm` to support per-form runs:

```scheme
; Run individual top-level definitions
(list
  . (symbol) @_f
  . (symbol) @run
  (#any-of? @_f "defun" "defn" "defmacro" "defagent" "deftool" "define")
) @_source (#set! tag "sema-run-form")

; Run any top-level expression
(source_file (list) @run (#set! tag "sema-run-form"))
```

Add matching task in `tasks.json`:

```jsonc
{
  "label": "sema run form",
  "command": "sema",
  "args": ["eval", "--stdin", "--path", "$ZED_FILE"],
  "tags": ["sema-run-form"]
}
```

### Safety & Guardrails

1. **Sandbox by default** — LSP-triggered evals use `--sandbox strict --no-llm` unless the user opts in via LSP `initializationOptions`.
2. **Timeouts** — subprocess killed after 5s (configurable). Inline result shows `⏱ timeout` on expiry.
3. **Result truncation** — inline display truncated to 120 chars. Full output in the output channel.
4. **Cancellation** — if user triggers a new eval on the same form before the previous completes, kill the old subprocess.
5. **UTF-16 ↔ UTF-8** — LSP positions are UTF-16. Document text store must handle offset conversion correctly.
6. **Staleness** — check `docVersion` before applying results; discard if document has changed since eval was requested.

### Implementation Checklist

| Task | Location | Effort |
|------|----------|--------|
| `sema eval` subcommand (--stdin, --json, --timeout) | `crates/sema/src/main.rs` | M |
| JSON result envelope serialization | `crates/sema/src/main.rs` | S |
| CodeLens provider (top-level form detection) | `crates/sema-lsp/src/features/codelens.rs` | M |
| Doctest detection in CodeLens | `crates/sema-lsp/src/features/codelens.rs` | S |
| Execute command handler + subprocess runner | `crates/sema-lsp/src/features/execute_command.rs` | M |
| `sema/evalResult` notification type | `crates/sema-lsp/src/protocol.rs` | S |
| Context construction (prefix forms) | `crates/sema-lsp/src/eval/subprocess.rs` | M |
| VS Code extension entry point + LSP client | `editors/vscode/sema/src/extension.ts` | M |
| Inline result decorations | `editors/vscode/sema/src/evalDecorations.ts` | M |
| Eval keybindings + commands | `editors/vscode/sema/src/commands.ts` | S |
| Zed runnables.scm update | `editors/zed/languages/sema/runnables.scm` | S |

### Future: Phase R2 — Long-Lived REPL Session

When stateless eval becomes a bottleneck (large files, slow prefix, stateful workflows):

- Add `sema repl --machine --json` mode — line-delimited JSON request/response protocol.
- LSP manages one REPL session per workspace folder.
- Track "loaded up to formIndex/version" per document to avoid re-running prefix.
- Protocol: `{ "id": 1, "op": "eval", "code": "...", "path": "..." }` → `{ "id": 1, "ok": true, "value": "..." }`
- Reset session on config change or explicit user command.

---

## Summary

| Phase | Feature | Complexity | Status |
|-------|---------|------------|--------|
| 0a | Special forms export | Easy | ✅ Done |
| 0b | End positions on `Span` | Easy | ✅ Done |
| T | Tree-sitter grammar + queries + editor configs | Easy–Medium | ✅ Done |
| 1 | LSP: Parse diagnostics | Easy | ✅ Done |
| 1b | LSP: Compile-time diagnostics (via sema-vm pipeline) | Medium | ✅ Done |
| 2 | LSP: Completion | Medium | ✅ Done |
| R | LSP: Runnable regions + eval bridge | Medium | ✅ Done (CLI + CodeLens + executeCommand) |
| 3a | LSP: Import/load path resolution | Easy | ✅ Done |
| 3b | LSP: Go to definition (user symbols) | Medium | ✅ Done |
| 3c | LSP: Cross-file module definitions | Hard | ✅ Done |
| 4 | LSP: Hover docs | Medium–Hard | ✅ Done |

### Implementation Order

1. ~~**Phase 0a**~~ ✅ — special forms list unified
2. ~~**Phase T**~~ ✅ — tree-sitter grammar (`editors/tree-sitter-sema/`)
3. ~~**Phase 0b**~~ ✅ — end spans (`Span` has `end_line`/`end_col`)
4. ~~**Phase 1**~~ ✅ — LSP parse diagnostics (`crates/sema-lsp/`, `sema lsp` subcommand)
5. ~~**Phase 2**~~ ✅ — LSP completion (special forms, builtins, user defs)
6. ~~**Phase R (CLI)**~~ ✅ — `sema eval` subcommand with JSON envelope
7. ~~**Phase 1b**~~ ✅ — compile-time diagnostics via `sema_vm::compile_program` (catches unbound vars, arity errors)
8. ~~**Phase R (LSP)**~~ ✅ — CodeLens, executeCommand, `sema/evalResult` notifications
9. ~~**Phase 3a**~~ ✅ — import/load path resolution (go-to-definition for import strings)
10. ~~**Phase 3b**~~ ✅ — go-to-definition for user-defined symbols (via SpanMap)
11. ~~**Phase 4**~~ ✅ — hover documentation (builtin docs from website + user function signatures)
12. ~~**Phase 3c**~~ ✅ — cross-file module definitions (go-to-definition + hover across imports)

---

## Future Considerations

- **Phase 1b — Compile-time diagnostics:** The VM compiler (`sema_vm::compile_program`) can now detect unbound variables, arity mismatches, and invalid forms without executing code. The pipeline is: parse → lower (`sema_vm::lower`) → resolve (`sema_vm::resolve`) → compile (`sema_vm::compile`). The resolver catches unbound variables, the lowerer validates special form syntax, and the compiler catches structural issues. This would be extremely valuable for the LSP — catching errors the reader alone can't see. **Current limitation:** The VM `Chunk.spans` table is not yet populated during normal compilation (only during serialization), so compiler errors may lack precise source locations. Also, macro expansion happens in the tree-walker (`sema-eval`), so the LSP would need to macroexpand under sandbox before handing to the VM pipeline. The recursion depth limit (256) added to lower/resolve/compile passes prevents stack overflow from malicious input.

- **Eval-level diagnostics:** Running the tree-walker for deeper analysis (type mismatches, arity errors at call sites). Must use `Sandbox::deny(Caps::ALL)` and add timeouts. High complexity, low priority.

- **Incremental parsing:** Phase 1 re-parses the full file on every keystroke. The reader is fast enough for typical Sema files. If performance becomes an issue, add debouncing (100–200ms via `tokio::time::sleep`) first, then consider `TextDocumentSyncKind::INCREMENTAL`.

- **WASM LSP:** `sema-wasm` exists but `tower-lsp` is stdio-oriented. In-browser LSP would need a different transport (WebSocket/worker). Out of scope.

- **Tree-sitter in LSP:** Once `tree-sitter-sema` exists, the LSP could optionally use it for faster incremental parsing instead of re-running `sema-reader` on every keystroke. Low priority — `sema-reader` is already fast for typical file sizes.

- **Symbol span tracking:** To give bare symbols source locations (needed for precise go-to-definition of variable references), the reader/lexer would need to track span information per-atom, not just per-compound-expression. Options: (a) extend `SpanMap` to key on symbol `Spur` + occurrence index, (b) wrap `Value::Symbol(Spur)` to carry an optional span, or (c) use the tree-sitter CST for position lookups instead. Option (c) is cleanest long-term once `tree-sitter-sema` exists.
