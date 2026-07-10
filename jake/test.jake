# Test variants beyond the default `jake test` (which stays bare — it's the
# daily driver). Namespaced as `test`, so: `jake test.http`, `jake test.lsp`,
# `jake test.notebook-e2e`, etc.

@group test
@desc "Run the full workspace test suite"
task workspace:
    cargo test --workspace

@group test
@desc "LSP unit + e2e (pytest) tests"
task lsp: [release]
    @needs uv
    cargo test -p sema-lsp
    cd crates/sema-lsp/tests/e2e && uv run pytest -v

@group test
@desc "Embedding benchmark (ignored) test"
task embedding-bench:
    cargo test -p sema-lang --test embedding_bench -- --ignored --nocapture

@group test
@desc "HTTP integration tests (requires network)"
task http:
    cargo test -p sema-lang --test http_test -- --ignored --nocapture

@group test
@desc "LLM integration tests (requires API keys)"
@require ANTHROPIC_API_KEY OPENAI_API_KEY
task llm:
    cargo test -p sema-lang --test llm_test -- --ignored --nocapture

# ── LLM provider smokes (real spend / keys) ──────────────────────────

@group test
@desc "Exercise every configured LLM provider"
task providers: [build]
    echo "=== Testing all LLM providers ==="
    cargo run --quiet -- examples/providers/test-all.sema

# Run a single provider smoke: `jake test.provider anthropic`
@group test
@desc "Test a single provider by name (arg 1)"
task provider: [build]
    cargo run --quiet -- examples/providers/test-{{$1}}.sema

# ── Browser E2E (Playwright) ─────────────────────────────────────────
# The editor plugins live in their own repos; these exercise the mono's own
# browser surfaces (notebook crate, sema-web dev server, and the playground).

@group test
@desc "Notebook browser E2E (Playwright)"
task notebook-e2e: [build]
    @needs npx
    echo "=== Running notebook E2E tests ==="
    @cd crates/sema-notebook/tests/e2e
    npx playwright test

# Vendor the browser runtime, build the release binary (embeds it), then drive
# the real `sema web` dev server in a browser.
@group test
@desc "sema web dev-server browser E2E (Playwright)"
task web-e2e: [wasm.web-runtime]
    @needs npx
    cargo build --release -p sema-lang
    echo "=== Running sema web dev-server E2E tests ==="
    @cd packages/sema-web
    npx playwright test --config playwright.dev-server.config.ts

# playground/ isn't an npm workspace member (unlike packages/sema-web, whose
# deps a root `npm ci` already covers), so this installs its own deps before
# driving the specs. Depends on pg.build so the WASM VM + examples bundle the
# specs exercise are current.
@group test
@desc "Playground browser E2E (Playwright)"
task playground-e2e: [pg.build]
    @needs npx npm
    echo "=== Running playground E2E tests ==="
    @cd playground
    npm install
    npx playwright test
