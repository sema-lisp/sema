# AGENTS.md — Sema (Lisp with LLM primitives, in Rust)

## Build & Test

- Build: `cargo build` | Lint: `make lint` (fmt-check + clippy -D warnings) | All tests: `cargo test`
- Single crate: `cargo test -p sema-reader` | Single test: `cargo test -p sema --test integration_test -- test_name`
- Run file: `cargo run -- examples/hello.sema` | REPL: `cargo run` | Eval: `cargo run -- -e "(+ 1 2)"`
- Integration tests: `crates/sema/tests/integration_test.rs`. Reader unit tests: `crates/sema-reader/src/reader.rs`.
- IntelliJ plugin: `editors/intellij/`. Unit tests: `./gradlew test` (116 tests, JUnit 4). Full IDE integration: `./gradlew buildPlugin integrationTest`.

## Architecture (Cargo workspace)

- **sema-core** → NaN-boxed `Value(u64)` struct, `Env` (Rc+RefCell+hashbrown::HashMap), `SemaError` (thiserror), eval/call callbacks (`set_eval_callback`/`set_call_callback`), thread-local VFS
- **sema-reader** → Lexer + s-expression parser → `Value` AST. Handles regex literals (`#"..."`), f-strings (`f"...${expr}..."`), short lambdas (`#(...)`), shebang lines
- **sema-vm** → Bytecode compiler (lowering → optimization → resolution → compilation), stack-based VM with intrinsic opcodes, NaN-boxed fast paths, debug hooks for DAP
- **sema-eval** → Special forms, module system (`EvalContext` holds module cache, call stack, spans), `call_value` for stdlib callback dispatch, destructuring/pattern matching (`destructure.rs`), prelude macros (`->`, `->>`, `as->`, `some->`, `when-let`, `if-let`)
- **sema-stdlib** → Native functions across many modules registered into `Env`. Higher-order fns (map, filter, fold) call through `sema_core::call_callback` — no mini-eval.
- **sema-llm** → LLM provider trait + Anthropic/OpenAI/Gemini/Ollama clients (tokio `block_on`), dynamic pricing from llm-prices.com with disk cache fallback
- **sema-lsp** → Language Server Protocol implementation (tower-lsp). Single-threaded backend via mpsc channel. Features: completions, hover, go-to-definition, references, rename, semantic tokens, folding ranges, inlay hints, document highlight, code lens (eval), workspace scanning, scope-aware symbol resolution.
- **sema-dap** → Debug Adapter Protocol server. Breakpoints, stepping (in/over/out), stack traces, variable inspection. Communicates with the bytecode VM via debug hooks.
- **sema-notebook** → `.sema-nb` JSON notebook format, eval engine, HTTP server + REST API, embedded browser UI, Markdown export
- **sema-mcp** → Model Context Protocol server exposing Sema eval/build/notebook tools to AI agents
- **sema-otel** → OpenTelemetry facade (spans/metrics); native-only, no-op on wasm32
- **sema-workflow** → Dynamic-workflow runtime: journals a frozen JSONL run-directory, bounded concurrency for leaves, `--resume` via memo sidecar. Leaf crate — depends only on sema-core + sema-otel.
- **sema-docs** → Builtin docs index generator. Each builtin is a markdown file in `crates/sema-docs/entries/`; `sema-docs gen` produces a JSON index consumed by LSP hover/completion and REPL apropos.
- **sema-fmt** → Code formatter for Sema source files
- **sema-wasm** → WASM bindings for browser playground
- **sema** → Binary (clap CLI + reedline REPL) + `sema build` (standalone executables) + `sema compile`/`sema disasm` + `sema lsp` + `sema dap` + `sema fmt` + integration tests
- Dep flow: `sema-core ← sema-reader ← sema-vm ← sema-eval ← sema-stdlib/sema-llm ← sema`. **Critical**: stdlib/llm depend on core, NOT eval. Stdlib calls eval via thread-local callbacks registered by sema-eval.

## Code Style

- Rust 2021, single-threaded (`Rc`, not `Arc`), `hashbrown::HashMap` for `Env` bindings, `BTreeMap` for user-facing sorted maps.
- Errors: `SemaError::eval()` / `::type_error()` / `::arity()` constructors — never raw enum variants. Use `.with_hint()` for actionable user guidance.
- Native fns: `NativeFn` takes `(&EvalContext, &[Value])` → `Result<Value, SemaError>`. Use `NativeFn::simple()` or `NativeFn::with_ctx()`. Special forms return `Trampoline`.
- Sema naming: slash-namespaced (`string/trim`, `file/read`), predicates end `?`, arrows for conversions (`string->symbol`). Legacy Scheme names kept (`string-append`, `substring`).

## Playground

- Hosted at **sema.run** (WASM)
- Examples live as `.sema` files in `playground/examples/<category>/` subdirectories
- `playground/build.mjs` auto-generates `playground/src/examples.js` from those files — **never edit `examples.js` by hand**
- To add a playground example: add the `.sema` file to the appropriate category dir, then run `cd playground && node build.mjs`

## Website

- Hosted at **sema-lang.com**, deployed via `cd website && vercel --prod`
- VitePress site, URLs require `.html` suffix: e.g. `https://sema-lang.com/docs/internals/lisp-comparison.html`

## Bytecode File Format (.semac)

- Spec: `website/docs/internals/bytecode-format.md` — **this is the single source of truth**
- Serialization/deserialization lives in `crates/sema-vm/src/serialize.rs`
- **Any change to opcodes, Chunk, Function, ExceptionEntry, or UpvalueDesc MUST update both the format spec and the serializer**

## Testing

The bytecode VM (`sema-vm`) is the **sole evaluator**. All tests run on the VM.
The `eval_tests!` / `eval_error_tests!` macros pin each case to a
literal expected value (`$input => $expected`) — that literal is the correctness
oracle (there's no second backend to differentially compare against).

- **Eval test file**: `crates/sema/tests/eval_test.rs` — use `eval_tests!` and `eval_error_tests!` (literal `=> expected` value is the oracle)
- **Async tests**: `crates/sema/tests/vm_async_test.rs` — async/channel tests
- **Integration / equivalence**: `integration_test.rs`, `vm_integration_test.rs`
- I/O, LLM, sandbox, CLI, module/import, server tests → `integration_test.rs`

### Adding a new special form
1. Add lowering in `lower_list()` dispatch in `crates/sema-vm/src/lower.rs`
2. If the form desugars into existing CoreExpr nodes (If/Let/LetStar/Call), do that in lower.rs
3. If it needs runtime helpers, add `__vm-<name>` native functions in `register_vm_delegates()` in `eval.rs`
4. Add `eval_tests!` in `eval_test.rs`

## Git Rules

- **NEVER use `git stash` without `--keep-index`.** Stashing without `--keep-index` silently destroys uncommitted work from other agents. If you need to inspect a clean tree, use `git stash --keep-index` so staged work is preserved, or work inside a separate worktree (`git worktree add`).
- **NEVER use `git stash` at all when not inside a dedicated worktree** — in the main checkout, stashing can clobber in-flight changes from parallel agents. Create a worktree instead.
- **NEVER use `git checkout -- <file>` to restore a file** if you don't own the only uncommitted changes to it — this destroys other agents' work. Use `git show <ref>:<path>` to inspect, or coordinate via branches.

## Adding Functionality

- **Builtin fn**: add to `crates/sema-stdlib/src/*.rs`, register in `register()`, add a test.
- **Special form**: add it to `lower_list()` in `crates/sema-vm/src/lower.rs` (+ compiler if needed), add a test.
- **Async feature**: implement in `async_ops.rs` using the yield signal mechanism, add a test in `vm_async_test.rs`.
- **Prelude macro**: add to `crates/sema-eval/src/prelude.rs` (Sema code evaluated at startup).
