# WASM VM build + browser-runtime vendoring.
#
# Two consumers embed a WASM-compiled Sema VM:
#   1. the `sema web` dev server (crates/sema/src/web/assets, embedded via build.rs)
#   2. the sema-web-example demo app
# Both need the same ~8 runtime files. The Makefile duplicated the copy list
# across `web-runtime` and `sema-web-example-build`; here it's a shared recipe.

web_runtime_dir = "crates/sema/src/web/assets"
example_dir = "examples/sema-web-app"

# ── WASM + JS package build (file recipe = incremental) ──────────────

# Compile the WASM VM. Depends on the wasm crate + every workspace crate it
# pulls in; the broad glob is correct (sema-wasm re-exports the whole stack).
file packages/sema-wasm/pkg/sema_wasm_bg.wasm: crates/**/*.rs Cargo.toml Cargo.lock
    @needs wasm-pack "cargo install wasm-pack"
    npm run build:wasm

@group wasm
@desc "Build the WASM VM package (incremental; skips if no crate changed)"
task build: [packages/sema-wasm/pkg/sema_wasm_bg.wasm]
    echo "WASM VM built: packages/sema-wasm/pkg/"

@group wasm
@desc "Build WASM VM + the JS embedding packages"
task js-lib-build: [build]
    @needs wasm-pack npm
    wasm-pack build crates/sema-wasm --target web --release --scope sema-lang \
        --out-dir ../../packages/sema-wasm/pkg -- --config 'profile.release.package.sema-wasm.opt-level="s"'
    cd packages/sema && npm install && npm run build

@group wasm
@desc "Fast (non-optimized) WASM VM build for iteration"
task js-lib-dev:
    @needs wasm-pack
    wasm-pack build crates/sema-wasm --target web --scope sema-lang --out-dir ../../packages/sema-wasm/pkg

# ── Shared runtime vendoring ─────────────────────────────────────────

# Copy the ~8 browser-runtime files into a destination dir (arg 1). Private
# helper shared by `web-runtime` and the example build so the list lives once.
@group wasm
task _vendor-runtime: [build]
    @needs npm
    npm run build
    mkdir -p {{$1}}/sema/backends
    cp packages/sema-web/dist/index.js {{$1}}/sema-web.js
    cp packages/sema/dist/index.js {{$1}}/sema/index.js
    cp packages/sema/dist/vfs.js {{$1}}/sema/vfs.js
    cp packages/sema/dist/backends/*.js {{$1}}/sema/backends/
    cp packages/sema-wasm/pkg/sema_wasm.js {{$1}}/sema_wasm.js
    cp packages/sema-wasm/pkg/sema_wasm_bg.wasm {{$1}}/sema_wasm_bg.wasm
    cp node_modules/@preact/signals-core/dist/signals-core.module.js {{$1}}/signals-core.module.js
    cp node_modules/morphdom/dist/morphdom-esm.js {{$1}}/morphdom-esm.js

# Vendor the browser runtime the `sema web` dev server embeds. build.rs picks
# these up (web_runtime cfg) and include_bytes! embeds them. Rebuild the sema
# binary afterward to embed. Artifacts are gitignored (built, multi-MB).
# Incremental vendoring (file recipes). Deps are the inputs `_vendor-runtime`
# does NOT rewrite — the WASM VM pkg (itself an incremental file recipe) plus
# the JS package sources. The npm-built `dist/` is deliberately NOT a dep: the
# recipe rebuilds it, so tracking it would re-trigger vendoring on every run.
# One vendored file per dir is the recipe output; the shared copy list lives in
# `_vendor-runtime` so it stays DRY.
file crates/sema/src/web/assets/sema_wasm_bg.wasm: packages/sema-wasm/pkg/sema_wasm_bg.wasm packages/sema/src/**/* packages/sema-web/src/**/*
    jake wasm._vendor-runtime {{web_runtime_dir}}

@group wasm
@desc "Vendor the sema-web browser runtime into the sema crate assets (incremental)"
task web-runtime: [crates/sema/src/web/assets/sema_wasm_bg.wasm]
    echo "Vendored web runtime -> {{web_runtime_dir}} (rebuild the sema binary to embed)"

file examples/sema-web-app/dist/vendor/sema_wasm_bg.wasm: packages/sema-wasm/pkg/sema_wasm_bg.wasm packages/sema/src/**/* packages/sema-web/src/**/*
    jake wasm._vendor-runtime {{example_dir}}/dist/vendor

@group wasm
@desc "Build the sema-web-example demo app (WASM + vendored runtime + app.vfs)"
task sema-web-example-build: [examples/sema-web-app/dist/vendor/sema_wasm_bg.wasm]
    cargo run -p sema-lang -- build --target web {{example_dir}}/app.sema -o {{example_dir}}/dist/app.vfs
    echo "Built {{example_dir}}/dist/app.vfs"

@group wasm
@desc "Build + serve the sema-web-example demo at :8788"
task sema-web-example: [sema-web-example-build]
    @needs npx
    echo "Serving the Sema Web example — open http://127.0.0.1:8788"
    npx serve -l 8788 {{example_dir}}
