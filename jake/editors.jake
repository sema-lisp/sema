# Editor tooling — tree-sitter grammar + browser E2E suites. Namespaced `ed`.

ts_dir = "editors/tree-sitter-sema"

@group editors
@desc "Install tree-sitter grammar deps"
task ts-setup:
    @cd editors/tree-sitter-sema
    npm install

# File recipe: regenerate the parser only when the grammar changes.
file editors/tree-sitter-sema/src/parser.c: editors/tree-sitter-sema/grammar.js
    @needs npx
    @cd editors/tree-sitter-sema
    npm install
    npx tree-sitter generate

@group editors
@desc "Generate the tree-sitter parser (incremental)"
task ts-generate: [editors/tree-sitter-sema/src/parser.c]
    echo "tree-sitter parser generated"

@group editors
@desc "Run tree-sitter corpus tests"
task ts-test: [ts-generate]
    @cd editors/tree-sitter-sema
    npx tree-sitter test

@group editors
@desc "Build the tree-sitter WASM + open the playground"
task ts-playground: [ts-generate]
    @cd editors/tree-sitter-sema
    npx tree-sitter build --wasm && npx tree-sitter playground

# ── Browser E2E ──────────────────────────────────────────────────────

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
