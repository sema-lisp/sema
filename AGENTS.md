# AGENTS.md — Sema (Lisp with LLM primitives, in Rust)

## Build & Test

- Build: `cargo build` | Lint: `make lint` (fmt-check + clippy -D warnings) | All tests: `cargo test`
- Single crate: `cargo test -p sema-reader` | Single test: `cargo test -p sema --test integration_test -- test_name`
- Run file: `cargo run -- examples/hello.sema` | REPL: `cargo run` | Eval: `cargo run -- -e "(+ 1 2)"`
- Integration tests: `crates/sema/tests/integration_test.rs`. Reader unit tests: `crates/sema-reader/src/reader.rs`.
- IntelliJ plugin: `editors/intellij/`. Unit tests: `./gradlew test` (116 tests, JUnit 4). Full IDE integration: `./gradlew buildPlugin integrationTest`.

## Architecture (Cargo workspace, 11 crates)

- **sema-core** → NaN-boxed `Value(u64)` struct, `Env` (Rc+RefCell+hashbrown::HashMap), `SemaError` (thiserror), eval/call callbacks (`set_eval_callback`/`set_call_callback`), thread-local VFS
- **sema-reader** → Lexer + s-expression parser → `Value` AST. Handles regex literals (`#"..."`), f-strings (`f"...${expr}..."`), short lambdas (`#(...)`), shebang lines
- **sema-eval** → Trampoline-based TCO evaluator, special forms, module system (`EvalContext` holds module cache, call stack, spans), `call_value` for stdlib callback dispatch, destructuring/pattern matching (`destructure.rs`), prelude macros (`->`, `->>`, `as->`, `some->`, `when-let`, `if-let`)
- **sema-vm** → Bytecode compiler (lowering → optimization → resolution → compilation), stack-based VM with intrinsic opcodes, NaN-boxed fast paths, debug hooks for DAP
- **sema-stdlib** → Native functions across many modules registered into `Env`. Higher-order fns (map, filter, fold) call through `sema_core::call_callback` — no mini-eval.
- **sema-llm** → LLM provider trait + Anthropic/OpenAI/Gemini/Ollama clients (tokio `block_on`), dynamic pricing from llm-prices.com with disk cache fallback
- **sema-lsp** → Language Server Protocol implementation (tower-lsp). Single-threaded backend via mpsc channel. Features: completions, hover, go-to-definition, references, rename, semantic tokens, folding ranges, inlay hints, document highlight, code lens (eval), workspace scanning, scope-aware symbol resolution.
- **sema-dap** → Debug Adapter Protocol server. Breakpoints, stepping (in/over/out), stack traces, variable inspection. Communicates with the bytecode VM via debug hooks.
- **sema-fmt** → Code formatter for Sema source files.
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
- Categories: `getting-started`, `functional`, `data`, `http`, `llm-tools`, `patterns`, `visuals`, `math-crypto`

## Website

- Hosted at **sema-lang.com**, deployed via `cd website && vercel --prod`
- VitePress site, URLs require `.html` suffix: e.g. `https://sema-lang.com/docs/internals/lisp-comparison.html`
- All docs pages are under `/docs/`: `https://sema-lang.com/docs/...`

## Bytecode File Format (.semac)

- Spec: `website/docs/internals/bytecode-format.md` — **this is the single source of truth**
- Serialization/deserialization lives in `crates/sema-vm/src/serialize.rs`
- **Any change to opcodes, Chunk, Function, ExceptionEntry, or UpvalueDesc MUST update both the format spec and the serializer**
- Format: 24-byte header (magic `\x00SEM` + version + flags), then sections (string table, function table, main chunk, optional debug sections)
- Spur remapping: global opcodes use string table indices in the file, remapped to process-local Spurs on load

## Testing — Dual Eval (Tree-walker + VM)

Sema has **two evaluators**: a tree-walking interpreter (`sema-eval`) and a bytecode VM (`sema-vm`). The VM is the default backend. Most language features must produce identical results across both backends. **Async features (async/await, channels) are VM-only** — tests go in `vm_async_test.rs`.

- **Dual-eval test file**: `crates/sema/tests/dual_eval_test.rs` — use `dual_eval_tests!` and `dual_eval_error_tests!` macros
- **Shared harness**: `crates/sema/tests/common/mod.rs` — provides `eval_tw()`, `eval_vm()`, `eval_both()`
- **VM-only async tests**: `crates/sema/tests/vm_async_test.rs` — async/channel tests (VM backend only)
- **Legacy files**: `integration_test.rs` (tree-walker only), `vm_integration_test.rs` (VM equivalence)
- **New tests go in `dual_eval_test.rs`** — the macros generate `_tw` and `_vm` variants automatically

### When to use dual eval
- Any new special form, destructuring, pattern, or evaluator change → `dual_eval_tests!`
- Pure stdlib functions (no I/O) → `dual_eval_tests!`
- I/O, LLM, sandbox, CLI, module/import, server tests → tree-walker only (`integration_test.rs`)
- Async features (async/await, channels) → VM-only (`vm_async_test.rs`)

### Adding VM support for a new special form
1. Add handler in `try_eval_special()` in `special_forms.rs` (tree-walker)
2. Add lowering in `lower_list()` dispatch in `crates/sema-vm/src/lower.rs` (VM)
3. If the form can desugar into existing CoreExpr nodes (If/Let/LetStar/Call), do that in lower.rs
4. If it needs runtime helpers, add `__vm-<name>` native functions in `register_vm_delegates()` in `eval.rs`
5. Add `dual_eval_tests!` in `dual_eval_test.rs`

## Adding Functionality

- **Builtin fn**: add to `crates/sema-stdlib/src/*.rs`, register in `register()`, add dual-eval test.
- **Special form**: add in `try_eval_special()` (tree-walker) AND `lower_list()` (VM), add dual-eval test.
- **Async feature**: implement in `async_ops.rs` using yield signal mechanism, add VM-only test in `vm_async_test.rs`. Async features do NOT need tree-walker implementation.
- **Prelude macro**: add to `crates/sema-eval/src/prelude.rs` (Sema code evaluated at startup).
