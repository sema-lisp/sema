# AGENTS.md — Sema (Lisp with LLM primitives, in Rust)

Canonical instructions for any coding agent working in this repo. `CLAUDE.md` just points here.

## Build & Test

```bash
make build              # dev build
make release            # optimized build
make test               # all tests (http tests ignored)
make test-http          # HTTP integration tests (requires network)
make lint               # fmt-check + clippy -D warnings
make fmt                # cargo fmt
make install            # install to ~/.cargo/bin (LTO, no PGO)
make install-pgo        # PGO build + install (slower build, faster runtime)
make all                # lint + test + build
make run                # start REPL
make example-notebook   # run demo notebook headlessly
make test-notebook-e2e  # Playwright E2E tests for notebook
```

- Single crate: `cargo test -p sema-reader` | Single test: `cargo test -p sema --test integration_test -- test_name`
- Single eval test: `cargo test -p sema --test eval_test -- test_name` | Ignored tests: `cargo test -p sema -- --ignored`
- Run file: `cargo run -- examples/hello.sema` | REPL: `cargo run` | Eval: `cargo run -- -e "(+ 1 2)"`
- Integration tests: `crates/sema/tests/integration_test.rs`. Eval tests: `crates/sema/tests/eval_test.rs`. Reader unit tests: `crates/sema-reader/src/reader.rs`.
- IntelliJ plugin: `editors/intellij/`. Unit tests: `./gradlew test` (116 tests, JUnit 4). Full IDE integration: `./gradlew buildPlugin integrationTest`.

## Architecture (Cargo workspace, 16 crates)

Dependency flow (arrows = "depends on"): `sema-core ← sema-reader ← sema-vm ← sema-eval ← sema-stdlib/sema-llm ← sema`. **Critical**: `sema-stdlib` and `sema-llm` depend on `sema-core` but NOT on `sema-eval` (avoids circular deps). Stdlib calls eval via thread-local callbacks registered by sema-eval.

- **sema-core** → NaN-boxed `Value(u64)` struct, `Env` (Rc+RefCell+hashbrown::HashMap), `SemaError` (thiserror), `EvalContext`, eval/call callbacks (`set_eval_callback`/`set_call_callback`), thread-local VFS
- **sema-reader** → Lexer + s-expression parser → `Value` AST. Handles regex literals (`#"..."`), f-strings (`f"...${expr}..."`), short lambdas (`#(...)`), shebang lines
- **sema-vm** → Bytecode compiler (lowering → optimization → resolution → compilation), stack-based VM with intrinsic opcodes, NaN-boxed fast paths, debug hooks for DAP. **The sole evaluator** (the tree-walker has been retired).
- **sema-eval** → `Interpreter`, special forms, macro expansion (VM-native), module system (`EvalContext` holds module cache, call stack, spans), `call_value` for stdlib callback dispatch, destructuring/pattern matching (`destructure.rs`), prelude macros (`->`, `->>`, `as->`, `some->`, `when-let`, `if-let`)
- **sema-stdlib** → Native functions across many modules registered into `Env`. Higher-order fns (map, filter, fold) call through `sema_core::call_callback` — no mini-eval.
- **sema-llm** → LLM provider trait + Anthropic/OpenAI/Gemini/Ollama clients (tokio `block_on`), dynamic pricing from llm-prices.com with disk cache fallback
- **sema-lsp** → Language Server Protocol (tower-lsp). Single-threaded backend via mpsc channel. Completions, hover, go-to-definition, references, rename, semantic tokens, folding ranges, inlay hints, document highlight, code lens (eval), workspace scanning, scope-aware symbol resolution.
- **sema-dap** → Debug Adapter Protocol server. Breakpoints, stepping (in/over/out), stack traces, variable inspection via VM debug hooks.
- **sema-notebook** → Jupyter-inspired `.sema-nb` JSON notebook format, eval engine with shared cell environment, HTTP server + REST API, embedded browser UI, Markdown export
- **sema-mcp** → Model Context Protocol server exposing Sema eval/build/notebook tools to AI agents
- **sema-otel** → OpenTelemetry facade (spans/metrics); native-only, no-op on wasm32
- **sema-workflow** → Dynamic-workflow runtime: journals a frozen JSONL run-directory, bounded concurrency for leaves, `--resume` via memo sidecar. Leaf crate — depends only on sema-core + sema-otel.
- **sema-docs** → Builtin docs index generator. Each builtin is a markdown file in `crates/sema-docs/entries/`; `sema-docs gen` produces a JSON index consumed by LSP hover/completion and REPL apropos.
- **sema-fmt** → Code formatter for Sema source files
- **sema-wasm** → WASM bindings for the browser playground at sema.run
- **sema** → Binary: clap CLI + reedline REPL + `sema build` (standalone executables) + `sema compile`/`sema disasm` + `sema lsp` + `sema dap` + `sema fmt` + `sema notebook` + integration tests. REPL submodules live in `crates/sema/src/repl/` (editor, highlighter, hinter, validator, inspector, commands).

## Key Design Patterns

- **Trampoline TCO** — `eval_step` returns `Trampoline::Value(v)` (done) or `Trampoline::Eval(expr, env)` (tail call). Special forms must return `Trampoline::Eval` for tail positions to enable proper tail-call optimization.
- **Callback architecture** — Stdlib higher-order functions (map, filter, foldl, sort-by) call through `sema_core::call_callback`, which dispatches to the real evaluator via a thread-local callback registered at interpreter startup. No mini-eval — all evaluation goes through the full evaluator.
- **Module system (EvalContext)** — `module_cache`, `current_file` (stack), `module_exports` are fields in `EvalContext` (`sema-core/src/context.rs`), threaded through the evaluator as `ctx: &EvalContext`. Modules identified by canonical file path. Module env is a child of the root env (gets builtins, not caller bindings). Paths resolve relative to the current file.
- **Keywords as functions** — `(:name person)` works like `(get person :name)`, handled in `eval_step` when a `Value::Keyword` appears in head position.
- **Invariant I2 (CORE-2 GC)** — a `NativeFn`'s boxed closure must not strongly capture anything that can transitively hold a `Value` or `Env` (that would form an uncollectable cycle the collector cannot trace). Traceable state belongs in `NativeFn.payload` (traced via a registered payload tracer); host infrastructure (e.g. the `__vm-*` delegates) captures `Weak` and upgrades at call time. See `docs/plans/2026-07-02-core2-gc.md` §2.

## Code Style

- Rust 2021, single-threaded (`Rc`, not `Arc`), `hashbrown::HashMap` for `Env` bindings, `BTreeMap` for user-facing sorted maps.
- Errors: `SemaError::eval()` / `::type_error()` / `::arity()` constructors — never raw enum variants. Use `.with_hint()` for actionable user guidance.
- Native fns: `NativeFn` takes `(&EvalContext, &[Value])` → `Result<Value, SemaError>`. Use `NativeFn::simple()` (no context) or `NativeFn::with_ctx()`. Special forms return `Trampoline`.
- Comments describe the code as it stands, not what changed. Drop change-narration ("now uses X instead of Y", "switched from", "previously", "fixed to"); keep the *why* (rationale, invariants, gotchas). Git tracks the story; the source shouldn't. Contrasting with a sibling (`unlike string/lower`) is fine; contrasting with a past version is not.
- **Sema naming (Decision #24)**: slash-namespaced for all new functions (`file/read`, `path/join`, `regex/match?`, `http/get`, `json/encode`, `string/split`); legacy Scheme names kept (`string-append`, `string-length`, `string-ref`, `substring`); arrow conversions (`string->symbol`, `keyword->string`); predicates end in `?` (`null?`, `list?`, `file/exists?`).

## Bytecode File Format (.semac)

- Spec: `website/docs/internals/bytecode-format.md` — **this is the single source of truth**
- Serialization/deserialization lives in `crates/sema-vm/src/serialize.rs`
- **Any change to opcodes, Chunk, Function, ExceptionEntry, or UpvalueDesc MUST update both the format spec and the serializer**
- Format: 24-byte header (magic `\x00SEM` + version + flags), then sections (string table, function table, main chunk, optional debug sections)
- Spur remapping: global opcodes use string-table indices in the file, remapped to process-local Spurs on load

## Testing

The bytecode VM (`sema-vm`) is the **sole evaluator**. All tests run on the VM. The `eval_tests!` / `eval_error_tests!` macros emit one test per case and pin each case to a literal expected value (`$input => $expected`) — that literal is the correctness oracle (there's no second backend to differentially compare against). The `common::eval_tw`/`eval_vm` helpers are equivalent and kept only because many tests call them to turn an expected Sema literal into a `Value` (`=> common::eval_tw("'(2 4 6)")`).

- **Eval test file**: `crates/sema/tests/eval_test.rs` — use `eval_tests!` and `eval_error_tests!` (literal `=> expected` is the oracle)
- **Async tests**: `crates/sema/tests/vm_async_test.rs` — async/channel tests
- **Integration / equivalence**: `integration_test.rs`, `vm_integration_test.rs`
- I/O, LLM, sandbox, CLI, module/import, server tests → `integration_test.rs`
- **LLM / agent paths (keyless, deterministic)**: `crates/sema/tests/llm_fake_test.rs` uses `sema_llm::fake::FakeProvider` — a scripted provider (canned replies, tool calls, errors, streamed chunks) installed as the default via `sema_llm::builtins::register_test_provider`. It records every request (`FakeRecorder`) so tests can assert on the exact messages the runtime built (e.g. round-2 tool-result correlation). Test hooks: `set_retry_base_ms(0)` (no real sleeps) and `set_network_max_retries`. **Always add a FakeProvider test when changing the agent loop, retry, cache, budget, or provider serializers** — this is the CI regression oracle that runs without API keys.
- Notebook E2E tests: `crates/sema-notebook/tests/e2e/` (Playwright, run via `make test-notebook-e2e`)
- A few `#[ignore]`d tests in `integration_test.rs` are a ready acceptance suite for the deferred VM stack-trace parity work (see `docs/deferred.md`).

### Fuzzing

- **Byte-level (cargo-fuzz)**: `make fuzz` (nightly + cargo-fuzz). Mutates raw bytes to find reader/eval panics — targets in `crates/sema-reader/fuzz` and `crates/sema-eval/fuzz`.
- **Grammar-based (in Sema)**: `make fuzz-grammar` (release binary). Generates random *valid* Sema programs and checks a printer/reader round-trip oracle and a compiler/VM value oracle; detects VM panics. Written in Sema at `fuzz/grammar-fuzz.sema`, driven by `scripts/grammar-fuzz.sh`. Every finding reproduces from one integer seed; `make fuzz-grammar-emit` prints sample programs. See `fuzz/README.md`.

## Adding Functionality

- **Builtin fn**: add to `crates/sema-stdlib/src/*.rs`, register in that module's `register()` fn, add an eval test.
- **Special form**: add lowering in the `lower_list()` dispatch in `crates/sema-vm/src/lower.rs` (+ compiler if needed), add an eval test. If it desugars into existing CoreExpr nodes (If/Let/LetStar/Call), do that in lower.rs; if it needs runtime helpers, add `__vm-<name>` native functions in `register_vm_delegates()` in `eval.rs`.
- **Prelude macro**: add to `crates/sema-eval/src/prelude.rs` (Sema code evaluated at startup, expanded VM-natively).
- **Async feature**: implement in stdlib (`async_ops.rs`) using the yield signal mechanism, add an async test in `vm_async_test.rs`.

## LLM & Agentic Features (sema-llm)

Hard-won conventions — follow these or you will reintroduce shipped bugs (see `docs/llm-agentic-audit.md` and CHANGELOG 1.21.x):

- **One canonical request, per-provider translation.** `ChatRequest` (in `sema-llm/src/types.rs`) is the single source of truth that Sema code produces; each provider's `build_request_body` (anthropic/openai/gemini/ollama) translates it to that provider's wire format. **Never branch on provider in Sema code or in builtins** — add the field to `ChatRequest` and map it in each serializer. Example: `:reasoning-effort` → OpenAI `reasoning_effort`, Anthropic extended thinking (`budget_tokens` + max-tokens/temperature adjustments), Gemini `thinkingConfig`.
- **Tool-result correlation is mandatory.** The agent loop (`run_tool_loop`) must echo the assistant `tool_calls` turn and send results as correlated `ChatMessage::tool_result(id, name, content)`; each serializer maps that to its native shape (OpenAI `role:"tool"`+`tool_call_id`, Anthropic `tool_use`/`tool_result` blocks, Gemini `functionCall`/`functionResponse`). Plain user-text results silently break OpenAI-family providers.
- **Compat is self-healing + no-op, never user-facing.** Unsupported params degrade gracefully: OpenAI's `temperature` 400 on gpt-5.0/o-series is learned per-model (`DROP_TEMPERATURE`) and retried; `max_completion_tokens` is used on official OpenAI/Azure (`is_official_openai_url`); reasoning effort is ignored where unsupported. Tolerate unknown response blocks (e.g. Anthropic `thinking`) rather than failing to decode.
- **Accounting invariant:** a cache hit makes no provider call → it MUST report zero usage so `track_usage` doesn't recharge cost or burn budget. Network retry covers 429/5xx/network with backoff+jitter (`complete_with_retry`); 4xx-non-429 fail fast.
- **Verification flow for any LLM change:** (1) deterministic FakeProvider test (required, CI), then (2) live integration test against real providers when feasible — keys are in the env (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY`, `MISTRAL_API_KEY`). Use cheap models: `gpt-5.4-mini`, `claude-haiku-4-5-20251001`, `gemini-2.5-flash`, `mistral-small-latest`; **don't hammer `gpt-5.5`** (priciest). OpenAI model ids use dots (`gpt-5.4-mini`), not the dashed form in the pricing snapshot. Ollama-down is a handy hard-fail for testing `llm/with-fallback`.

## Git Rules

- **NEVER use `git stash` without `--keep-index`.** Stashing without `--keep-index` silently destroys uncommitted work from other agents. If you need to inspect a clean tree, use `git stash --keep-index` so staged work is preserved, or work inside a separate worktree (`git worktree add`).
- **NEVER use `git stash` at all when not inside a dedicated worktree** — in the main checkout, stashing can clobber in-flight changes from parallel agents. Create a worktree instead.
- **NEVER use `git checkout -- <file>` to restore a file** if you don't own the only uncommitted changes to it — this destroys other agents' work. Use `git show <ref>:<path>` to inspect, or coordinate via branches.

## Release Procedure

1. **Run the full CI-equivalent suite locally** (plain `cargo test` is NOT enough — it skips the example/bytecode smoke tests that run in CI): `cargo test --workspace && make examples && make smoke-bytecode && make lint && make docs-check` — all must pass. Skipping `make examples`/`make smoke-bytecode` locally is how a regression once shipped past four releases. The crates.io and npm publish workflows `needs:` a `verify` gate (`.github/workflows/verify.yml`) — a red suite blocks publishing. After pushing the tag, confirm `gh run list` shows CI **and** the publish gate green; never assume "started" == "passed". (The cargo-dist `Release`/binaries workflow is autogenerated and not gated — treat its result as advisory.)
2. **Bump versions** in `Cargo.toml`: the workspace version *and* the 12 inter-crate `=X.Y.Z` pins (13 lines total). One-shot: `sed -i '' -e 's/^version = "OLD"/version = "NEW"/' -e 's/version = "=OLD"/version = "=NEW"/g' Cargo.toml`, then `grep -c 'NEW' Cargo.toml` (expect 13) and `grep -c 'OLD' Cargo.toml` (expect 0).
3. **Update CHANGELOG.md** — add a new `## X.Y.Z` section at the top.
4. **Build release**: `cargo build --release` (also refreshes `Cargo.lock`); verify `./target/release/sema --version`.
5. **Commit & tag**: `git commit -m "release: X.Y.Z"`, `git tag vX.Y.Z`.
6. **Push**: `git push origin main --tags` — triggers 4 CI workflows (CI, Release/binaries, Publish to crates.io, Publish to npm); confirm with `gh run list`.
7. **Deploy website** *only if website content changed*: `cd website && vercel --prod` (see the deploy gotcha under Website).

## Playground

- Hosted at **sema.run** (WASM)
- Examples live as `.sema` files in `playground/examples/<category>/` subdirectories
- `playground/build.mjs` auto-generates `playground/src/examples.js` from those files — **never edit `examples.js` by hand**
- To add an example: add the `.sema` file to the appropriate category dir, then run `cd playground && node build.mjs`
- Categories: `getting-started`, `functional`, `data`, `http`, `llm-tools`, `patterns`, `visuals`, `math-crypto`

## Website

- Hosted at **sema-lang.com**, deployed via `cd website && vercel --prod`
- **Deploy gotcha (monorepo):** the Vercel project is CLI-deployed and **uploads only `website/`** — so any `import` in the site that reaches *outside* `website/` (e.g. `../../../ui/dist/...` or `../../../examples/...`) builds locally but **fails on Vercel** ("No such file or directory"). Keep all site imports inside `website/`. The `<sema-code-typer>` brand-page showcase (which imports the repo-root `@sema/ui` bundle) is currently **commented out** in `website/.vitepress/theme/BrandGuide.vue` for this reason; re-enabling it needs the proper monorepo deploy fix (git-integrate the Vercel project — `sourceFilesOutsideRootDirectory` is already on — or vendor the bundle into `website/public/`). Verify a deploy actually promoted: a failed build leaves the previous deploy live (production looks unchanged).
- The live homepage is the **`HomepageV2.vue`** component (via `website/index.md`), **not** any standalone `*.html` file.
- VitePress site uses **clean URLs** (`cleanUrls: true` in both `config.ts` and `vercel.json`): the canonical form is extensionless, e.g. `https://sema-lang.com/docs/internals/lisp-comparison`. The build still writes `*.html` files and Vercel 308-redirects the `.html` form to the clean URL, so don't hardcode `.html` in internal doc links — write `/docs/lsp`, not `/docs/lsp.html`. Per-page `<link rel="canonical">` + `og:url` are emitted extensionless via `transformHead`. All docs pages are under `/docs/`.
- **Syntax highlighting**: use `` ```sema `` for code blocks in website docs. The custom TextMate grammar is at `website/.vitepress/sema.tmLanguage.json` (copied from canonical source `editors/vscode/sema/syntaxes/sema.tmLanguage.json` — keep in sync). For GitHub markdown outside the website, `sema` won't be recognized — use `` ```scheme `` as fallback there.
- **OpenGraph cards**: per-page social images are generated from `website/og-template.html` (the single design source — homepage + docs variants, driven by URL query params) by `website/scripts/generate-og.mjs` (headless Chromium via Playwright). Run `make site-og` (or `cd website && npm run og`) after editing the template, logo, page titles, or version, then commit the regenerated `website/public/og/*.jpg` plus `playground/og-playground.jpg`. `config.ts` `transformHead` wires each page to its card; slug/category/dimension logic is shared via `website/.vitepress/og.shared.mjs`.

## Design Docs

- `docs/adr.md` — numbered design decisions with rationale
- `docs/wip.md` — open threads / work-in-progress (currently archived to `docs/plans/archive/wip.md`; recreate when new threads arise)
- `docs/limitations.md` — known gaps and limitations
- `docs/deferred.md` — items parked with rationale (won't-fix or revisit-later)
- `docs/plans/` — individual implementation plans, named `YYYY-MM-DD-<slug>.md` (completed plans move to `docs/plans/archive/`)
- `docs/vm-status.md`, `docs/performance-roadmap.md` — VM internals reference
- `docs/IDEAS.md` — feature tracker (consolidated from issues)
- `docs/bugs/` — short write-ups of specific known test/code issues
