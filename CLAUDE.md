# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```bash
make build                                               # dev build
make release                                             # optimized build
make test                                                # all tests (http tests ignored)
make test-http                                           # run HTTP integration tests (requires network)
make lint                                                # fmt-check + clippy -D warnings
make fmt                                                 # cargo fmt
make install                                             # install to ~/.cargo/bin
make all                                                 # lint + test + build
make run                                                 # start REPL
make example-notebook                                    # run demo notebook headlessly
make example-notebook-serve                              # serve demo notebook in browser
make test-notebook-e2e                                   # Playwright E2E tests for notebook

cargo test -p sema-reader                                # test single crate
cargo test -p sema --test integration_test -- test_name  # single integration test
cargo test -p sema --test dual_eval_test -- test_name    # single dual-eval test
cargo test -p sema -- --ignored                          # run any ignored tests
cargo run -- examples/hello.sema                         # run file (bytecode VM — the sole evaluator)
cargo run -- -e "(+ 1 2)"                                # eval expression
```

Integration tests are in `crates/sema/tests/integration_test.rs`. Dual-eval tests in `crates/sema/tests/dual_eval_test.rs`. Reader unit tests in `crates/sema-reader/src/reader.rs`.

## Architecture

Cargo workspace with 14 crates. Dependency flow (arrows = "depends on"):

```
sema-core  ←  sema-reader  ←  sema-vm  ←  sema-eval  ←  sema (binary)
    ↑                            ↑            ↑               ↑
    ├── sema-stdlib ─────────────┼────────────┘               │
    ├── sema-llm ────────────────┼────────────┘               │
    ├── sema-lsp (language server)             │               │
    ├── sema-dap (debug adapter)───────────────┘               │
    ├── sema-fmt (formatter)                                   │
    ├── sema-notebook (notebook UI + server) ──────────────────┘
    └── sema-wasm (browser playground)
```

- **sema-core** — NaN-boxed `Value(u64)` struct, `Env` (Rc + RefCell + hashbrown::HashMap), `SemaError`, `EvalContext`, thread-local VFS
- **sema-reader** — Lexer + parser producing `Value` AST. Handles regex literals (`#"..."`), f-strings (`f"...${expr}..."`), short lambdas (`#(...)`)
- **sema-vm** — Bytecode compiler (lowering → optimization → resolution → compilation), stack-based VM with intrinsic opcodes, debug hooks for DAP
- **sema-eval** — `Interpreter`, macro expansion (VM-native), module system (load/import drivers), prelude, eval/call callback wiring. The bytecode VM (`sema-vm`) is the sole evaluator.
- **sema-stdlib** — Native functions across many modules registered into `Env`
- **sema-llm** — LLM provider trait + Anthropic/OpenAI/Gemini/Ollama clients (tokio `block_on` for sync)
- **sema-notebook** — Jupyter-inspired notebook interface: `.sema-nb` JSON format, evaluation engine with shared cell environment, HTTP server with REST API, embedded browser UI, Markdown export
- **sema-lsp** — Language Server Protocol (tower-lsp). Single-threaded backend via mpsc channel. Completions, hover, go-to-definition, references, rename, semantic tokens, folding ranges, inlay hints, document highlight, code lens, workspace scanning.
- **sema-dap** — Debug Adapter Protocol server. Breakpoints, stepping, stack traces, variable inspection via VM debug hooks.
- **sema-fmt** — Code formatter for Sema source files.
- **sema-wasm** — WASM bindings for browser playground at sema.run
- **sema** — Binary: CLI (clap) + REPL (reedline) + `sema build` (standalone executables) + `sema compile`/`sema disasm` + `sema lsp` + `sema dap` + `sema fmt` + `sema notebook` + integration tests. REPL submodules live in `crates/sema/src/repl/` (editor, highlighter, hinter, validator, inspector, commands).

**Critical**: `sema-stdlib` and `sema-llm` depend on `sema-core` but NOT on `sema-eval` (avoids circular deps). Stdlib calls eval via thread-local callbacks registered by sema-eval.

## Key Design Patterns

### Trampoline TCO

`eval_step` returns `Trampoline::Value(v)` (done) or `Trampoline::Eval(expr, env)` (tail call). Special forms must return `Trampoline::Eval` for tail positions to enable proper tail-call optimization.

### Callback Architecture

Stdlib higher-order functions (map, filter, foldl, sort-by) call through `sema_core::call_callback` which dispatches to the real evaluator via a thread-local callback registered at interpreter startup. No mini-eval — all evaluation goes through the full evaluator.

### Module System (EvalContext)

`module_cache`, `current_file` (stack), `module_exports` are fields in `EvalContext` (defined in `sema-core/src/context.rs`), threaded through the evaluator as `ctx: &EvalContext`. Modules identified by canonical file path. Module env is child of root env (gets builtins, not caller bindings). Paths resolve relative to current file.

### Keywords as Functions

`(:name person)` works like `(get person :name)` — handled in `eval_step` when a `Value::Keyword` appears in head position.

## Code Conventions

### Rust

- Errors: use `SemaError::eval()`, `::type_error()`, `::arity()` constructors — never raw enum variants. Use `.with_hint()` for actionable guidance.
- Native fns: `NativeFn` takes `(&EvalContext, &[Value])`, returns `Result<Value, SemaError>`. Use `NativeFn::simple()` for fns that don't need context, `NativeFn::with_ctx()` for those that do
- Single-threaded: `Rc` everywhere, not `Arc`. `hashbrown::HashMap` for `Env` bindings, `BTreeMap` for user-facing sorted maps.

### Sema Language Naming (Decision #24)

- **Slash-namespaced** for all new functions: `file/read`, `path/join`, `regex/match?`, `http/get`, `json/encode`, `string/split`
- **Legacy Scheme** kept: `string-append`, `string-length`, `string-ref`, `substring`
- **Arrow conversions**: `string->symbol`, `keyword->string`
- **Predicates end in `?`**: `null?`, `list?`, `file/exists?`

## Bytecode File Format (.semac)

- Spec: `website/docs/internals/bytecode-format.md` — **this is the single source of truth**
- Serialization/deserialization lives in `crates/sema-vm/src/serialize.rs`
- **Any change to opcodes, Chunk, Function, ExceptionEntry, or UpvalueDesc MUST update both the format spec and the serializer**
- Format: 24-byte header (magic `\x00SEM` + version + flags), then sections (string table, function table, main chunk, optional debug sections)
- Spur remapping: global opcodes use string table indices in the file, remapped to process-local Spurs on load

## Testing

The bytecode VM is the **sole evaluator** (the tree-walker has been retired). All
tests run on the VM. The `dual_eval_tests!` / `dual_eval_error_tests!` macros
still exist (they currently emit `_tw` and `_vm` variants, both of which now run
on the VM via the same entry points) and stay useful for pinning a literal
expected value as the correctness oracle — the literal anchor matters more now
that there's no second backend to differentially compare against.

- **Dual-eval test file**: `crates/sema/tests/dual_eval_test.rs` — use `dual_eval_tests!` and `dual_eval_error_tests!` macros (literal `=> expected` value is the oracle)
- **Async tests**: `crates/sema/tests/vm_async_test.rs` — async/channel tests
- **VM equivalence / integration**: `vm_integration_test.rs`, `integration_test.rs`
- I/O, LLM, sandbox, CLI, module/import, server tests → `integration_test.rs`
- Notebook E2E tests: `crates/sema-notebook/tests/e2e/` (Playwright, run via `make test-notebook-e2e`)
- A few `#[ignore]`d tests in `integration_test.rs` are a ready acceptance suite for the deferred VM stack-trace parity work (see `docs/deferred.md`).

## Adding New Functionality

- **Builtin fn**: add to `crates/sema-stdlib/src/*.rs`, register in that module's `register()` fn, add dual-eval test.
- **Special form**: add it to the VM lowering in `lower_list()` in `crates/sema-vm/src/lower.rs` (+ compiler if needed), add dual-eval test.
- **Prelude macro**: add to `crates/sema-eval/src/prelude.rs` (Sema code evaluated at startup, expanded VM-natively).
- **Async feature**: implement in stdlib (`async_ops.rs`) using the yield signal mechanism, add an async test in `vm_async_test.rs`.

## Release Procedure

1. **Run tests**: `cargo test` — all must pass
2. **Bump versions** in workspace `Cargo.toml` (`workspace.package.version`) — all crate deps auto-inherit
3. **Update CHANGELOG.md** — add new `## X.Y.Z` section at top
4. **Build release**: `cargo build --release`
5. **Commit & tag**: `git commit`, `git tag vX.Y.Z`
6. **Push**: `git push origin main --tags` (triggers cargo-dist + crates.io publish)
7. **Deploy website**: `cd website && vercel --prod`

## Playground

- Hosted at **sema.run** (WASM)
- Examples live as `.sema` files in `playground/examples/<category>/` subdirectories
- `playground/build.mjs` auto-generates `playground/src/examples.js` from those files — **never edit `examples.js` by hand**
- To add a playground example: add the `.sema` file to the appropriate category dir, then run `cd playground && node build.mjs`
- Categories: `getting-started`, `functional`, `data`, `http`, `llm-tools`, `patterns`, `visuals`, `math-crypto`

## Website

- Hosted at **sema-lang.com**, deployed via `cd website && vercel --prod`
- VitePress site, URLs require `.html` suffix: e.g. `https://sema-lang.com/docs/internals/lisp-comparison.html`
- All docs pages are under `/docs/`: `https://sema-lang.com/docs/...`
- **Syntax highlighting**: Use `` ```sema `` for code blocks in website docs. The custom TextMate grammar is at `website/.vitepress/sema.tmLanguage.json` (copied from canonical source `editors/vscode/sema/syntaxes/sema.tmLanguage.json` — keep in sync). For GitHub markdown outside the website, `sema` won't be recognized — use `` ```scheme `` as fallback there.
- **OpenGraph cards**: per-page social images are generated from `website/og-template.html` (the single design source — homepage + docs variants, driven by URL query params) by `website/scripts/generate-og.mjs` (headless Chromium via Playwright). Run `make site-og` (or `cd website && npm run og`) after editing the template, logo, page titles, or version, then commit the regenerated `website/public/og/*.jpg` plus `playground/og-playground.jpg`. `config.ts` `transformHead` wires each page to its card; slug/category/dimension logic is shared via `website/.vitepress/og.shared.mjs`.

## Design Docs

- `docs/adr.md` — numbered design decisions with rationale
- `docs/wip.md` — open threads / work-in-progress with full context
- `docs/limitations.md` — known gaps and limitations
- `docs/deferred.md` — items parked with rationale (won't-fix or revisit-later)
- `docs/plans/` — individual implementation plans, named `YYYY-MM-DD-<slug>.md`
- `docs/vm-status.md`, `docs/performance-roadmap.md` — VM internals reference
- `docs/IDEAS.md` — feature tracker (consolidated from issues)
- `docs/bugs/` — short write-ups of specific known test/code issues
