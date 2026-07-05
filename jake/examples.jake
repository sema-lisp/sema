# Examples, notebooks, and smoke tests. Imported UNnamespaced (`jake examples`,
# `jake smoke-bytecode`) — these are CI-critical and referenced by the `ci` gate.

# Run every runnable example headless; report pass/skip/fail. Interactive,
# server, and hardware examples are skipped (see scripts/run-examples.sh).
@group examples
@desc "Run all runnable examples headless (per-example timeout)"
task examples: [release]
    @export EXAMPLE_TIMEOUT=30
    ./scripts/run-examples.sh

# Compile every runnable example into a standalone binary with `sema build` and
# execute it — exercises the whole release/portability path.
@group examples
@desc "Compile every example to a standalone binary and run it"
task examples-build: [release]
    @export EXAMPLE_TIMEOUT=30
    ./scripts/build-examples.sh

# Compile -> disasm -> run every example; the ONLY check that catches a desynced
# disassembler after an opcode add (see AGENTS.md bytecode format rules).
@group examples
@desc "Bytecode smoke: compile + disasm + run every example"
task smoke-bytecode: [build]
    ./scripts/smoke-bytecode.sh ./target/debug/sema

# ── Notebooks ────────────────────────────────────────────────────────

@group notebook
@desc "Run the demo notebook headless"
task example-notebook: [build]
    echo "=== Running example notebook ==="
    @ignore
    cargo run --quiet -- notebook run examples/notebook/demo.sema-nb

@group notebook
@desc "Headless smoke for the deterministic async notebook pair"
task example-notebooks-async: [release]
    ./target/release/sema notebook run examples/notebook/async-basics.sema-nb
    ./target/release/sema notebook run examples/notebook/realtime-monitor.sema-nb

@group notebook
@desc "Serve the demo notebook in a browser"
task example-notebook-serve: [build]
    cargo run --quiet -- notebook serve examples/notebook/demo.sema-nb

# ── LLM / provider smokes (real spend / keys) ────────────────────────

# LIVE async/streaming stress against real provider APIs (real spend — cents).
# Manual gate for the true-async work; needs ANTHROPIC/OPENAI/GEMINI keys.
@group llm
@desc "LIVE async/streaming provider stress (real spend, needs API keys)"
@require ANTHROPIC_API_KEY OPENAI_API_KEY GEMINI_API_KEY
task llm-stress: [release]
    ./target/release/sema examples/llm/async-stress-live.sema

@group llm
@desc "Exercise every configured LLM provider"
task test-providers: [build]
    echo "=== Testing all LLM providers ==="
    cargo run --quiet -- examples/providers/test-all.sema

# Run a single provider smoke: `jake test-provider anthropic`
@group llm
@desc "Test a single provider by name (arg 1)"
task test-provider: [build]
    cargo run --quiet -- examples/providers/test-{{$1}}.sema

@group llm
@desc "LIVE RAG smoke over Sema docs (needs embed+rerank+chat keys)"
task rag-demo: [build]
    echo "=== RAG over Sema docs (embed -> search -> rerank -> answer) ==="
    cargo run --quiet -- examples/llm/rag-docs-search.sema

# ── Browser E2E ──────────────────────────────────────────────────────
# The editor plugins now live in their own repos; these E2E suites exercise
# the mono's own browser surfaces (notebook crate + sema-web dev server).

@group e2e
@desc "Notebook browser E2E (Playwright)"
task test-notebook-e2e: [build]
    @needs npx
    echo "=== Running notebook E2E tests ==="
    @cd crates/sema-notebook/tests/e2e
    npx playwright test

# Vendor the browser runtime, build the release binary (embeds it), then drive
# the real `sema web` dev server in a browser.
@group e2e
@desc "sema web dev-server browser E2E (Playwright)"
task test-web-e2e: [wasm.web-runtime]
    @needs npx
    cargo build --release -p sema-lang
    echo "=== Running sema web dev-server E2E tests ==="
    @cd packages/sema-web
    npx playwright test --config playwright.dev-server.config.ts
