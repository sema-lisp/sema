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

# ── UI library vendoring ─────────────────────────────────────────────

# Vendor the @sema-lang/ui bundle + design tokens into the notebook crate,
# where they are embedded via include_str! (single-binary, offline — like the
# bundled fonts). The library lives in its own repo (sema-lisp/ui) and ships
# to npm, so both files are pulled from the published package (pinned
# SEMA_UI_VERSION) rather than a local build. Re-run after bumping
# SEMA_UI_VERSION. Naive copy: only the main bundle is vendored; lazily-loaded
# Shiki grammar chunks aren't served, so non-`sema` code fences in markdown
# degrade to unhighlighted (the `sema` grammar is bundled).
SEMA_UI_VERSION = "0.2.0"

@group notebook
@desc "Vendor @sema-lang/ui bundle + tokens.css into the notebook crate (pinned SEMA_UI_VERSION)"
@needs curl
task notebook-ui-vendor:
    mkdir -p crates/sema-notebook/src/ui/vendor
    curl -fsSL https://unpkg.com/@sema-lang/ui@{{SEMA_UI_VERSION}}/dist/sema-ui.js -o crates/sema-notebook/src/ui/vendor/sema-ui.js
    curl -fsSL https://unpkg.com/@sema-lang/ui@{{SEMA_UI_VERSION}}/src/styles/tokens.css -o crates/sema-notebook/src/ui/vendor/tokens.css
    echo "Vendored @sema-lang/ui@{{SEMA_UI_VERSION}} -> crates/sema-notebook/src/ui/vendor/{sema-ui.js,tokens.css}"

# ── LLM / provider smokes (real spend / keys) ────────────────────────

# LIVE async/streaming stress against real provider APIs (real spend — cents).
# Manual gate for the true-async work; needs ANTHROPIC/OPENAI/GEMINI keys.
@group llm
@desc "LIVE async/streaming provider stress (real spend, needs API keys)"
@require ANTHROPIC_API_KEY OPENAI_API_KEY GEMINI_API_KEY
task llm-stress: [release]
    ./target/release/sema examples/llm/async-stress-live.sema

@group llm
@desc "LIVE RAG smoke over Sema docs (needs embed+rerank+chat keys)"
task rag-demo: [build]
    echo "=== RAG over Sema docs (embed -> search -> rerank -> answer) ==="
    cargo run --quiet -- examples/llm/rag-docs-search.sema

# Provider smokes and browser E2E (test.providers, test.notebook-e2e,
# test.web-e2e) live in jake/test.jake, namespaced as `test.*`.
