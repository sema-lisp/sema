# Sema LSP Server — Design Document

**Date:** 2026-02-16
**Status:** Draft
**Implementation:** Not started

## Overview

Add Language Server Protocol support to Sema, providing real-time diagnostics, completions, go-to-definition, and hover documentation in editors. The LSP server reuses the existing `sema-reader`, `sema-core`, and `sema-eval` crates — the reader already tracks source spans and errors already carry hints/notes, so Phase 1 (diagnostics) is largely wiring.

## Crate Structure

### New Crate: `crates/sema-lsp/`

```toml
# crates/sema-lsp/Cargo.toml
[package]
name = "sema-lsp"
version = "0.1.0"
edition = "2021"

[dependencies]
sema-core.workspace = true
sema-reader.workspace = true
sema-eval.workspace = true
sema-stdlib.workspace = true
tower-lsp = "0.20"
tokio = { workspace = true, features = ["rt-multi-thread", "macros", "io-std"] }
ropey = "1"          # for efficient incremental text edits (Phase 1+)
dashmap = "6"        # concurrent document store for the server backend
```

Add `"crates/sema-lsp"` to the workspace `members` in the root `Cargo.toml`.

### Binary Entry Point

**`sema lsp` subcommand** — add an `Lsp` variant to the `Commands` enum in `crates/sema/src/main.rs`:

```rust
#[derive(Subcommand)]
enum Commands {
    Ast { /* ... */ },
    /// Start the Language Server Protocol server
    Lsp,
}
```

The handler calls into `sema_lsp::run_server().await`. The `sema` crate gains a dependency on `sema-lsp`.

### Dependency Flow

```
sema-core ← sema-reader ← sema-eval ← sema-stdlib
                ↑              ↑            ↑
                └──── sema-lsp ─────────────┘
                          ↑
                        sema (binary, `sema lsp` subcommand)
```

This follows the existing rule: `sema-lsp` depends on `sema-core`, `sema-reader`, `sema-eval`, and `sema-stdlib` — but **not** the other way around.

### Threading Model

`tower-lsp` is async (tokio). Sema's evaluator is single-threaded (`Rc`, not `Arc`). The LSP server must run parsing/eval on a dedicated thread (or `spawn_blocking`) and send results back to the async tower-lsp handler. For Phase 1 (parse-only diagnostics), `read_many_with_spans` is cheap enough to call inline via `spawn_blocking`.

**Important:** `Rc`-based `Value` is `!Send`, so parsed ASTs cannot cross the `spawn_blocking` boundary directly. The diagnostic extraction (spans, messages) must happen inside the blocking closure, returning only `Send`-safe types (Strings, LSP Diagnostic structs) back to the async handler.

---

## Phase 1: Diagnostics (Parse Errors)

**Complexity:** Easy
**Reuses:** `sema_reader::read_many_with_spans`, `SemaError` (spans, hints, notes)

### How It Works

1. On `textDocument/didOpen` and `textDocument/didChange`, extract the full document text.
2. Call `sema_reader::read_many_with_spans(&text)` inside `spawn_blocking`.
3. On `Ok(...)` → publish empty diagnostics (no parse errors).
4. On `Err(e)` → convert to LSP `Diagnostic` and publish.

### Document State Management

The server needs to track open document contents. Use a `DashMap<Url, String>` (or `Rope`) to store current text for each open file:

```rust
struct SemaLanguageServer {
    client: Client,
    documents: DashMap<Url, String>,
}
```

This is needed because `didChange` with `FULL` sync replaces the whole text, but future incremental sync or multi-file features (Phase 3) need access to other files' contents.

### Span Conversion

The reader's `SemaError::Reader { message, span }` already contains a `Span { line, col }` (1-indexed). Convert to LSP zero-indexed `Position`:

```rust
fn sema_span_to_lsp_range(span: &sema_core::Span) -> lsp_types::Range {
    let pos = lsp_types::Position {
        line: (span.line.saturating_sub(1)) as u32,
        character: (span.col.saturating_sub(1)) as u32,
    };
    // Point range; could expand to token end if we track end spans later
    lsp_types::Range { start: pos, end: pos }
}
```

### Hints and Notes as Related Information

`SemaError` supports `.hint()` and `.note()` via `WithContext`. Map these to `DiagnosticRelatedInformation` or append to the diagnostic message:

```rust
fn sema_error_to_diagnostic(err: &SemaError, uri: &Url) -> Diagnostic {
    let range = match err.inner() {
        SemaError::Reader { span, .. } => sema_span_to_lsp_range(span),
        _ => lsp_types::Range::default(),
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

### Error Recovery: Collecting Multiple Parse Errors

> **Current limitation:** `read_many_with_spans` returns `Result<..., SemaError>` — it stops at the **first** parse error. This means the LSP can only show one red squiggle at a time.
>
> **Recommended improvement for Phase 1:** Add a `read_many_with_spans_recover(input: &str) -> (Vec<Value>, SpanMap, Vec<SemaError>)` variant to `sema-reader` that continues parsing after an error (skipping to the next top-level form). This is standard for LSP servers and vastly improves the user experience — showing all errors at once instead of forcing a fix-one-reparse-fix-next cycle.
>
> **Minimal approach:** The tokenizer (`tokenize()`) also returns on first error. Recovery at the token level (skip to next `\n(` or matching `)`) would be sufficient for most cases.

### Minimal `tower-lsp` Skeleton

```rust
use dashmap::DashMap;
use tower_lsp::{Client, LanguageServer, LspService, Server};
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;

struct SemaLanguageServer {
    client: Client,
    documents: DashMap<Url, String>,
}

#[tower_lsp::async_trait]
impl LanguageServer for SemaLanguageServer {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                ..Default::default()
            },
            ..Default::default()
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client.log_message(MessageType::INFO, "Sema LSP initialized").await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        let text = params.text_document.text.clone();
        self.documents.insert(uri.clone(), text.clone());
        self.publish_diagnostics(uri, &text).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        if let Some(change) = params.content_changes.into_iter().last() {
            let uri = params.text_document.uri.clone();
            self.documents.insert(uri.clone(), change.text.clone());
            self.publish_diagnostics(uri, &change.text).await;
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        self.documents.remove(&params.text_document.uri);
        // Clear diagnostics on close
        self.client.publish_diagnostics(params.text_document.uri, vec![], None).await;
    }
}

impl SemaLanguageServer {
    async fn publish_diagnostics(&self, uri: Url, text: &str) {
        let text = text.to_string();
        // spawn_blocking because reader uses Rc (not Send)
        let diagnostics = tokio::task::spawn_blocking(move || {
            match sema_reader::read_many_with_spans(&text) {
                Ok(_) => vec![],
                Err(e) => vec![sema_error_to_diagnostic(&e, &uri)],
            }
        }).await.unwrap_or_default();
        // uri was moved into the closure; reconstruct or clone beforehand
        // (see implementation note below)
        self.client.publish_diagnostics(uri, diagnostics, None).await;
    }
}

pub async fn run_server() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let (service, socket) = LspService::new(|client| SemaLanguageServer {
        client,
        documents: DashMap::new(),
    });
    Server::new(stdin, stdout, socket).serve(service).await;
}
```

> **Implementation note:** The `uri` is moved into the `spawn_blocking` closure for the error diagnostic, but also needed afterward for `publish_diagnostics`. Clone `uri` before the closure. The skeleton above has this bug — fix during implementation.

### `didClose` Handling

The original plan omitted `didClose`. The server must:

- Remove the document from the store
- Clear diagnostics (publish empty `vec![]`) so stale errors don't linger

### What You Get

- Real-time red squiggles for unclosed parens, unterminated strings, invalid tokens
- Error messages include Sema's hints ("Did you mean...?") and notes
- Works with zero evaluation — pure parse, fast, no side effects

---

## Phase 2: Completion

**Complexity:** Medium
**Reuses:** `sema_stdlib::register_stdlib` (for builtin names), `SpecialFormSpurs` (for special form names), `Env` (for user bindings)

### Strategy

The REPL already has the infrastructure for listing available symbols: the global `Env` holds all stdlib bindings as `NativeFn` entries, and special forms are enumerated in `SpecialFormSpurs`. The LSP needs to:

1. **Build a static completion list** of all builtin function names and special form names at server startup.
2. **Dynamically extract user-defined names** by parsing the current document and walking for `define`/`defun` forms.
3. **On `textDocument/completion`**, find the partial word at the cursor and fuzzy-match against all known symbols.

### Builtin Names

Create a helper in `sema-stdlib` (or `sema-lsp`) that enumerates all registered names.

> **API mismatch:** `register_stdlib` now requires `(env: &Env, sandbox: &Sandbox)` — the plan's code omits the `Sandbox` argument. Use `Sandbox::default()` (which is `allow_all`):

```rust
fn collect_builtin_names() -> Vec<String> {
    let env = sema_core::Env::new();
    let sandbox = sema_core::Sandbox::default();
    sema_stdlib::register_stdlib(&env, &sandbox);
    env.bindings.borrow().keys()
        .map(|spur| sema_core::resolve(*spur))
        .collect()
}
```

> **Note:** This creates an `Env` with `Rc`-based values — must run on the same thread (inside `spawn_blocking` or at startup on a dedicated thread). Cache the resulting `Vec<String>` since it never changes.

### Special Form Names

The REPL already has a `SPECIAL_FORMS` constant in `main.rs` (lines 12–59) with ~40 names. Options:

1. **Reuse the REPL list:** Move the `SPECIAL_FORMS` array to `sema-eval` as a public constant, share it between REPL and LSP. This is the DRY approach and keeps the two in sync.
2. **Export from `SpecialFormSpurs`:** Add a `pub fn special_form_names()` to `sema-eval`. Cleaner but requires the struct to be kept in sync manually.
3. **Hard-code in `sema-lsp`:** Quick but creates a third copy.

> **Recommended:** Option 1. The REPL's list already includes extra names like `->`, `->>`, `as->`, `for-each`, `apply`, `map` that aren't in `SpecialFormSpurs` (they're stdlib functions used in head position, not true special forms). A shared constant avoids confusion.

### User-Defined Symbols

Walk the parsed AST looking for top-level `(define name ...)` and `(defun name ...)` forms.

> **Missing forms:** Also extract `(defmacro name ...)`, `(define-record-type name ...)`, `(deftool name ...)`, and `(defagent name ...)` — these are all binding forms users would want to complete on.

```rust
fn extract_definitions(exprs: &[Value]) -> Vec<(String, CompletionItemKind)> {
    let define = sema_core::intern("define");
    let defun = sema_core::intern("defun");
    let defmacro = sema_core::intern("defmacro");
    let deftool = sema_core::intern("deftool");
    let defagent = sema_core::intern("defagent");
    let mut names = Vec::new();
    for expr in exprs {
        if let Value::List(items) = expr {
            if items.len() >= 2 {
                if let Value::Symbol(head) = &items[0] {
                    let kind = if *head == define {
                        Some(CompletionItemKind::VARIABLE)
                    } else if *head == defun {
                        Some(CompletionItemKind::FUNCTION)
                    } else if *head == defmacro {
                        Some(CompletionItemKind::KEYWORD)
                    } else if *head == deftool || *head == defagent {
                        Some(CompletionItemKind::CLASS)
                    } else {
                        None
                    };
                    if let (Some(kind), Some(Value::Symbol(name))) = (kind, items.get(1)) {
                        names.push((sema_core::resolve(*name), kind));
                    }
                }
            }
        }
    }
    names
}
```

> **Nested scopes:** The above only handles top-level forms. `let`/`let*`/`letrec` bindings won't appear. This is acceptable for Phase 2 — scope-aware completion is a Phase 3+ concern.

### Completion Item Kinds

| Source                                            | `CompletionItemKind` |
| ------------------------------------------------- | -------------------- |
| Special forms (`if`, `define`, `lambda`, ...)     | `Keyword`            |
| Stdlib functions (`+`, `string/trim`, `map`, ...) | `Function`           |
| User `define`                                     | `Variable`           |
| User `defun`                                      | `Function`           |
| User `defmacro`                                   | `Keyword`            |
| User `deftool`/`defagent`                         | `Class`              |

### Trigger

Register `completion_provider` in `ServerCapabilities` with trigger characters `(` and a space. The completion handler extracts the word fragment left of the cursor and fuzzy-match against all known symbols.

> **Implementation detail:** Sema symbols use `/` for namespacing (e.g. `string/trim`). The completion prefix parser must not split on `/` — treat it as part of the symbol name.

---

## Phase 3: Go to Definition

**Complexity:** Hard
**Reuses:** `SpanMap`, `EvalContext` (module cache, file paths), `read_many_with_spans`

### Sub-features

#### 3a. `import`/`load` Path Resolution

When the cursor is on a string literal inside `(import "path")` or `(load "path")`:

- Resolve the path relative to the current file (same logic as the evaluator's module loader in `special_forms.rs`)
- Return a `Location` pointing to the resolved file, line 0

**Complexity:** Easy — mostly path manipulation.

#### 3b. User-Defined Symbol Definitions

For `(define x ...)` and `(defun f ...)`:

- Parse the current file with `read_many_with_spans`
- Walk top-level forms to find `define`/`defun` that binds the symbol under the cursor
- Use the `SpanMap` to look up the span of the definition's name symbol (keyed by `Rc` pointer address)

**Challenge:** The `SpanMap` keys on `Rc` pointer address (`usize`), which means you need to hold the parsed `Value` tree while doing the lookup. This is fine for single-file analysis.

> **Important subtlety:** The `SpanMap` stores the address of the `Rc`'s inner allocation (via `Rc::as_ptr` cast to `usize`). You must use the exact same `Value` pointers returned from parsing — no cloning, no re-interning. The lookup code must traverse the parsed AST and read the `Rc` pointer of each `Value` to find its span. This is fragile and worth documenting in code.

**Complexity:** Medium.

#### 3c. Cross-File Module Definitions

For symbols imported via `(import "module")`:

- Parse the target module file
- Find the exported symbol's definition span in that file
- Return a `Location` in the target file

**Complexity:** Hard — requires resolving module paths, parsing other files, and tracking exports.

### SpanMap Enhancement (Optional)

Currently `Span` only has `line` and `col` (start position). For precise definition highlighting, consider adding `end_line` and `end_col` to `Span`:

```rust
pub struct Span {
    pub line: usize,
    pub col: usize,
    pub end_line: usize,
    pub end_col: usize,
}
```

This is optional — point spans work fine for go-to-definition (the editor will highlight the word at the position).

> **Recommendation:** Do this as a prerequisite for Phase 3. End spans also improve Phase 1 diagnostics (underline the whole malformed token, not just a point). The reader already knows token boundaries during lexing — propagating `end` position is straightforward.

### Symbol-Under-Cursor Resolution

> **Missing from plan:** Phase 3 needs a function to determine "what symbol is the cursor on?" given a `(line, col)` and the source text. Options:
>
> 1. Re-lex the line at the cursor position to find token boundaries
> 2. Use the `SpanMap` to find the nearest span to the cursor position (requires iterating all spans — O(n) but fine for typical file sizes)
> 3. Simple regex/character-class scan backward and forward from cursor position to find symbol boundaries
>
> Option 3 is simplest and most robust for Lisp (symbols are delimited by whitespace and parens).

---

## Phase 4: Hover Documentation

**Complexity:** Medium–Hard
**Reuses:** `NativeFn` (name), `Lambda` (params), `SpanMap`

### 4a. Builtin Function Documentation

Currently `NativeFn` only stores `name` and `func`. To show documentation on hover, add an optional `doc` field:

```rust
pub struct NativeFn {
    pub name: String,
    pub func: Box<NativeFnInner>,
    pub doc: Option<String>,  // new field
}
```

Then update `register_fn` / `NativeFn::simple` / `NativeFn::with_ctx` in `sema-stdlib` to accept an optional doc string, and populate it for all ~350 builtins.

**Complexity:** The struct change is easy; writing 350 doc strings is a lot of work. Could start with just the most common functions and add docs incrementally.

**Alternative:** Ship a static `HashMap<&str, &str>` of name→doc in `sema-lsp` without modifying `NativeFn`. Less elegant but zero impact on core crates. Also easier to auto-generate from the website docs at `sema-lang.com/docs/`.

> **Recommended:** Start with the static `HashMap` approach. The website already has documentation for many functions — scrape or manually extract those. Adding `doc` to `NativeFn` is a nice-to-have but touches every registration call across 19 stdlib modules.

### 4b. User Function Signatures

For user-defined `(defun name (x y z) ...)`, show the parameter list on hover:

```rust
if let Value::List(params) = &items[2] {
    let param_names: Vec<String> = params.iter()
        .filter_map(|p| if let Value::Symbol(s) = p { Some(resolve(*s)) } else { None })
        .collect();
    format!("(defun {} ({}))", name, param_names.join(" "))
}
```

### 4c. Hover Response Format

Return Markdown-formatted hover content:

````markdown
```sema
(defun fibonacci (n))
```

Recursively computes the nth Fibonacci number.
````

> **Note:** Use a fenced code block with `sema` language ID so VS Code applies syntax highlighting in the hover popup (the TextMate grammar already handles this).

---

## VS Code Integration

**Complexity:** Easy (once the LSP server works)

### Changes to `editors/vscode/sema/package.json`

The current extension only provides syntax highlighting (TextMate grammar). Add an LSP client:

1. Add `"activationEvents": ["onLanguage:sema"]`
2. Add `"main": "./out/extension.js"` (need a small JS/TS extension entry point)
3. Add `vscode-languageclient` as a dependency

### Extension Entry Point (`extension.ts`)

```typescript
import { workspace, ExtensionContext } from "vscode";
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
} from "vscode-languageclient/node";

let client: LanguageClient;

export function activate(context: ExtensionContext) {
  const serverOptions: ServerOptions = {
    command: "sema",
    args: ["lsp"],
  };

  const clientOptions: LanguageClientOptions = {
    documentSelector: [{ scheme: "file", language: "sema" }],
  };

  client = new LanguageClient(
    "sema-lsp",
    "Sema Language Server",
    serverOptions,
    clientOptions,
  );
  client.start();
}

export function deactivate(): Thenable<void> | undefined {
  return client?.stop();
}
```

> **Extension packaging:** The extension currently has no `node_modules`, `tsconfig.json`, or build step. Adding the LSP client requires:
>
> - `npm init` + `npm install vscode-languageclient`
> - A `tsconfig.json` for compiling `extension.ts` → `out/extension.js`
> - Update `.vscodeignore` to exclude `node_modules` but include `out/`
> - Consider bundling with esbuild for smaller extension size

### Other Editors

- **Neovim:** Add config to `editors/vim/` for `nvim-lspconfig` pointing to `sema lsp`
- **Helix:** Add `[[language]]` entry to `editors/helix/languages.toml` with `command = "sema"`, `args = ["lsp"]`
- **Emacs:** Add `lsp-mode` or `eglot` config to `editors/emacs/`
- **Zed:** Add `settings.json` snippet for `lsp` configuration — Zed has growing adoption

---

## Testing Strategy

> **Missing from original plan.** LSP servers are notoriously hard to test manually. Plan for:

### Unit Tests (in `sema-lsp`)

- `sema_span_to_lsp_range` conversion (1-indexed to 0-indexed edge cases: line 1 col 1 → 0,0)
- `sema_error_to_diagnostic` with various error types (Reader, WithContext, nested WithTrace+WithContext)
- `extract_definitions` with various AST shapes
- `collect_builtin_names` returns non-empty list and includes known names

### Integration Tests

- Spin up the LSP server in-process using `tower_lsp::LspService::new()`, send `initialize` → `initialized` → `didOpen` with bad source → assert diagnostics
- Use the `tower-lsp` test utilities or the `lsp-types` crate to construct request/response pairs
- Test completion results for partial prefixes

### Manual Testing Checklist

- [ ] `sema lsp` starts and doesn't crash
- [ ] VS Code shows red squiggles for `(define x` (unclosed paren)
- [ ] Fixing the error clears the squiggles
- [ ] Completions appear when typing `str` → suggests `string/trim`, `string-append`, etc.
- [ ] Multiple files open simultaneously

---

## Summary

| Phase | Feature           | Complexity      | Sema Infrastructure Reused                            | New Code                                                         |
| ----- | ----------------- | --------------- | ----------------------------------------------------- | ---------------------------------------------------------------- |
| 1     | Parse diagnostics | **Easy**        | `read_many_with_spans`, `SemaError` spans/hints/notes | ~200 lines (tower-lsp boilerplate + span conversion + doc store) |
| 2     | Completion        | **Medium**      | `Env` bindings, `SPECIAL_FORMS`, AST walking          | ~250 lines (symbol collection + completion handler)              |
| 3     | Go to definition  | **Hard**        | `SpanMap`, module resolution logic, `EvalContext`     | ~400 lines (definition finder + cross-file resolution)           |
| 4     | Hover docs        | **Medium–Hard** | `NativeFn`, `Lambda` params                           | ~200 lines LSP side + doc string authoring effort                |

### Recommended Implementation Order

1. **Phase 1** first — delivers immediate value (red squiggles in editor), low effort, validates the tower-lsp plumbing.
2. **Phase 2** next — completions are the most-requested IDE feature, and the data sources already exist.
3. **Phase 3a** (import paths) is easy and useful, do it alongside Phase 2.
4. **Phase 3b/3c** and **Phase 4** can be done incrementally as the LSP matures.

### Prerequisites Before Starting

1. Decide on error recovery strategy: add `read_many_with_spans_recover` to `sema-reader` or accept single-error-at-a-time for Phase 1
2. Add end positions to `Span` (optional but recommended)
3. Move `SPECIAL_FORMS` from `main.rs` to a shared location

### Open Questions

- **Incremental parsing:** Phase 1 re-parses the entire file on every keystroke. For small Sema files this is fine (the reader is fast). If performance becomes an issue, consider debouncing or incremental text sync (`TextDocumentSyncKind::INCREMENTAL`). Debouncing (100–200ms) should be the first mitigation — it's trivial to implement with `tokio::time::sleep`.
- **Eval-level diagnostics:** Should the LSP also run the evaluator to catch runtime errors (unbound variables, type mismatches)? This would require sandboxing (no IO side effects) and adds complexity. Defer to a future phase. The `Sandbox` capability system already exists and could deny all IO caps for LSP analysis.
- **WASM support:** Could the LSP run in-browser for the playground at sema.run? `tower-lsp` is designed for stdio, but there are WASM LSP experiments. Low priority.
- **`NativeFn` takes `&EvalContext`:** The plan's Phase 2 `collect_builtin_names` creates an `Env` and registers stdlib, but several stdlib modules need a `Sandbox` argument. Needs to account for the full `register_stdlib(&env, &sandbox)` signature.
