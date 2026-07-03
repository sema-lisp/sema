# @sema/ui — Lit-based web component library. Built once, then VENDORED (copied)
# into the notebook crate (embedded via include_str!) and, later, the playground.
# There is no npm publish; consumers use the vendored bundle.
#
# The win over the Makefile: `file` recipes make vendoring INCREMENTAL — the
# bundle rebuilds and re-copies only when ui/src actually changed, instead of
# `cd ui && npm run build && cp` on every invocation (the notebook worktree's
# current `notebook-ui-vendor` target rebuilds unconditionally every time).

# Vendor destinations (single source of truth for the copy targets).
notebook_vendor = "crates/sema-notebook/src/ui/vendor/sema-ui.js"
playground_vendor = "playground/vendor/sema-ui.js"

# ── Build (the file recipe is the incremental core) ──────────────────

# Stage 1: build the bundle. Rebuilds only if any ui source/config changed.
file ui/dist/sema-ui.js: ui/src/**/* ui/package.json ui/vite.config.ts ui/tsconfig.json
    @needs npm
    @cd ui
    npm install
    npm run build

@group ui
@desc "Build the @sema/ui bundle (incremental; skips if ui/src unchanged)"
task build: [ui/dist/sema-ui.js]
    echo "@sema/ui bundle ready: ui/dist/sema-ui.js"

# ── Vendoring (stage 2: copy into each consumer, only if the bundle moved) ──

# Naive copy: only the main bundle is vendored; lazily-loaded Shiki grammar
# chunks aren't served, so non-`sema` code fences degrade to unhighlighted.
file crates/sema-notebook/src/ui/vendor/sema-ui.js: ui/dist/sema-ui.js
    mkdir -p {{dirname(notebook_vendor)}}
    cp ui/dist/sema-ui.js {{notebook_vendor}}
    echo "Vendored -> {{notebook_vendor}}"

# Provisional: the playground doesn't consume @sema/ui yet. The path is a
# placeholder for the notebook-ui-refactor follow-up; confirm it when the
# playground wires the components in.
file playground/vendor/sema-ui.js: ui/dist/sema-ui.js
    mkdir -p {{dirname(playground_vendor)}}
    cp ui/dist/sema-ui.js {{playground_vendor}}
    echo "Vendored -> {{playground_vendor}}"

@group ui
@desc "Build + vendor the @sema/ui bundle into the notebook crate"
task vendor-notebook: [crates/sema-notebook/src/ui/vendor/sema-ui.js]
    echo "notebook vendoring up to date"

@group ui
@desc "Build + vendor the @sema/ui bundle into the playground (provisional path)"
task vendor-playground: [playground/vendor/sema-ui.js]
    echo "playground vendoring up to date"

@group ui
@desc "Vendor @sema/ui into every consumer (notebook + playground)"
task vendor: [vendor-notebook, vendor-playground]
    echo "all @sema/ui consumers vendored"

# ── Dev / quality ────────────────────────────────────────────────────

@group ui
@desc "Start the @sema/ui vite dev server"
task dev:
    @needs npm
    @cd ui
    npm install
    npm run dev

@group ui
@desc "Run @sema/ui component tests (vitest)"
task test:
    @needs npm
    @cd ui
    npm install
    npm test

@group ui
@desc "Lint @sema/ui sources"
task lint:
    @needs npm
    @cd ui
    npm run lint

@group ui
@desc "Regenerate the custom-elements manifest"
task analyze:
    @cd ui
    npm run analyze

@group ui
@desc "Export the standalone <sema-code-typer> showcase bundle"
task export-typer:
    @cd ui
    npm run export:typer
