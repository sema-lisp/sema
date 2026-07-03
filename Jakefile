# Jakefile — build automation for Sema (Lisp with LLM primitives, in Rust)
#
# A spike replacing the 393-line Makefile with modular Jake recipes.
# Run `jake -l` for the grouped recipe list, `jake -s <recipe>` for details.
#
# Module layout (each file owns one area; imported below, some namespaced):
#   jake/rust.jake     — cargo build/test/lint/install/pgo  (UNnamespaced: the daily drivers)
#   jake/docs.jake     — builtin-doc index, pricing, link check
#   jake/examples.jake — headless example + notebook + bytecode smoke runners
#   jake/wasm.jake     — WASM VM build + browser-runtime vendoring (file recipes = incremental)
#   jake/ui.jake       — @sema/ui component library build + vendoring into notebook/playground
#   jake/web.jake      — VitePress docs site (build/preview/deploy/OG cards)
#   jake/playground.jake — sema.run WASM playground (build/dev/deploy)
#   jake/bench.jake    — hyperfine benchmarks + samply profiling
#   jake/fuzz.jake     — cargo-fuzz + in-Sema grammar fuzzer
#   jake/editors.jake  — tree-sitter grammar, LSP e2e
#   jake/release.jake  — coverage, mutation testing, publish-list guard

@import "jake/rust.jake"
@import "jake/docs.jake"
@import "jake/examples.jake"
@import "jake/wasm.jake" as wasm
@import "jake/ui.jake" as ui
@import "jake/web.jake" as site
@import "jake/playground.jake" as pg
@import "jake/bench.jake" as bench
@import "jake/fuzz.jake" as fuzz
@import "jake/editors.jake" as ed
@import "jake/release.jake" as release

# ── Aggregate pipelines ──────────────────────────────────────────────

@group ci
@desc "Lint + test + build (the `make all` equivalent)"
task all: [lint, test, build]
    echo "all: lint + test + build complete"

# Full CI-equivalent gate (mirrors AGENTS.md release step 1). Runs the checks
# plain `cargo test` skips: example + bytecode smoke suites. Dependencies run
# in listed order (run serial, i.e. without -j, so the gate reads top-to-bottom).
@group ci
@desc "Full local CI gate: workspace tests + examples + bytecode smoke + lint + docs"
task ci: [test-workspace, examples, smoke-bytecode, lint, docs-check]
    echo "CI gate green"

# One-shot "ship the web": build playground, gate on E2E, deploy site + playground.
@group deploy
@desc "Build + E2E-gate + deploy both docs site and playground to production"
task deploy-all: [pg.build]
    @confirm "Deploy site AND playground to production?"
    cd playground && npx playwright test
    jake ed.test-notebook-e2e
    jake site.deploy
    jake pg.deploy
    echo "site + playground deployed"

@group deploy
@desc "Quick deploy (site + playground, skips the E2E gate)"
task deploy: [site.deploy, pg.deploy]
    echo "deployed site + playground"
