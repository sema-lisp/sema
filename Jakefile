# Jakefile — build automation for Sema (Lisp with LLM primitives, in Rust)
#
# The sole build entry point (the Makefile has been retired). Run `jake -l` for
# the grouped recipe list, `jake -s <recipe>` for details. CI installs the jake
# release binary and calls these recipes directly.
#
# The editor plugins and tree-sitter grammar now live in their own repos under
# the sema-lisp org (each carries its own Jakefile), so there is no editors
# module here — only the mono's own browser E2E stayed (in jake/examples.jake).
#
# Module layout (each file owns one area; imported below, some namespaced):
#   jake/rust.jake     — cargo build/test/lint/install/pgo  (UNnamespaced: the daily drivers)
#   jake/test.jake     — test variants (workspace/lsp/http/llm/e2e/providers), namespaced `test.*`
#   jake/docs.jake     — builtin-doc index, pricing, link check
#   jake/examples.jake — headless example + notebook + bytecode smoke runners + browser E2E
#   jake/wasm.jake     — WASM VM build + browser-runtime vendoring (file recipes = incremental)
#   jake/web.jake      — VitePress docs site (build/preview/deploy/OG cards)
#   jake/playground.jake — sema.run WASM playground (build/dev/deploy)
#   jake/bench.jake    — hyperfine benchmarks + samply profiling
#   jake/fuzz.jake     — cargo-fuzz + in-Sema grammar fuzzer
#   jake/release.jake  — coverage, mutation testing, publish-list guard
#   jake/mcpb.jake     — cross-platform MCP Bundle (.mcpb) packaging, namespaced `mcpb`
#   jake/sh.jake       — shellcheck + shfmt hygiene for scripts/*.sh, namespaced `sh`

# `@rooted` so the `sema-lisp/workspace` meta-repo can `@import "sema/Jakefile" as
# sema` and have these recipes' relative paths (jake/*.jake, scripts/, crates/)
# resolve against this dir. No-op when run standalone from the repo root.
@rooted

@import "jake/rust.jake"
@import "jake/test.jake" as test
@import "jake/docs.jake"
@import "jake/examples.jake"
@import "jake/wasm.jake" as wasm
@import "jake/web.jake" as site
@import "jake/playground.jake" as pg
@import "jake/bench.jake" as bench
@import "jake/fuzz.jake" as fuzz
@import "jake/release.jake" as release
@import "jake/mcpb.jake" as mcpb
@import "jake/sh.jake" as sh

# Load .env so LLM/provider tasks pick up API keys (ANTHROPIC_API_KEY, …)
# without polluting the shell. No-op when there's no .env.
@dotenv

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
task ci: [test.workspace, examples, smoke-bytecode, lint, docs-check]
    echo "CI gate green"

# Runs the full CI gate, then prints the manual release steps from AGENTS.md.
# The version-bump sed across Cargo.toml stays manual by design (too risky to
# automate); this just gets you to a verified-green tree and reminds the steps.
@group ci
@desc "Run the CI gate, then print the manual release checklist"
task release-preflight: [ci]
    @echo ""
    @echo "CI gate is green. To cut a release (see AGENTS.md 'Release Procedure'):"
    @echo "  1. Bump workspace version + every inter-crate =X.Y.Z pin in Cargo.toml"
    @echo "     sed -i '' -e 's/^version = \"OLD\"/version = \"NEW\"/' \\"
    @echo "               -e 's/version = \"=OLD\"/version = \"=NEW\"/g' Cargo.toml"
    @echo "     grep -c 'OLD' Cargo.toml   # must be 0"
    @echo "  2. Add a ## X.Y.Z section at the top of CHANGELOG.md"
    @echo "  3. cargo build --release && ./target/release/sema --version"
    @echo "  4. git commit -m 'release: X.Y.Z' && git tag vX.Y.Z"
    @echo "  5. git push origin main --tags   # then confirm 'gh run list' is green"

# One-shot "ship the web": build playground, gate on E2E, deploy site + playground.
@group deploy
@desc "Build + E2E-gate + deploy both docs site and playground to production"
task deploy-all: [pg.build]
    @confirm "Deploy site AND playground to production?"
    cd playground && npx playwright test
    jake test.notebook-e2e
    jake site.deploy
    jake pg.deploy
    echo "site + playground deployed"

@group deploy
@desc "Quick deploy (site + playground, skips the E2E gate)"
task deploy: [site.deploy, pg.deploy]
    echo "deployed site + playground"
