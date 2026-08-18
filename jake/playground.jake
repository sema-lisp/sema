# sema.run WASM playground. Namespaced as `pg`.

# Build the WASM VM into the playground, then generate examples.js from the
# .sema files. wasm-pack/cargo are already incremental; the file recipe keeps
# the JS regen (node build.mjs) from re-running when nothing changed.
@group pg
@desc "Build the playground (WASM VM + examples bundle)"
task build:
    @needs wasm-pack "cargo install wasm-pack"
    @needs node
    ./scripts/wasm-build.sh crates/sema-wasm --target web --out-dir ../../playground/pkg -- --config 'profile.release.package.sema-wasm.opt-level="s"'
    cd playground && node build.mjs

@group pg
@desc "Build + serve the playground at :8787"
task dev: [build]
    @needs npx node
    cd playground && node scripts/gen-devtools-json.mjs
    cd playground && npx serve -l 8787

@group pg
@desc "Build + deploy the playground to production (Vercel)"
@needs npx
task deploy: [build]
    @confirm "Deploy the playground to production?"
    # site.og also writes playground/og-playground.jpg; hash-idempotent, so this
    # is a no-op unless the template, logo, or version changed.
    jake site.og
    git status --porcelain playground/og-playground.jpg | grep -q . && echo "NOTE: og-playground.jpg regenerated — commit it" || true
    cd playground && npx vercel --prod --yes
    # A failed remote build leaves the previous deploy live — verify it serves.
    sleep 5
    curl -sf -o /dev/null https://sema.run/ && echo "pg.deploy: sema.run responds ✓" || { echo "pg.deploy: sema.run is not responding after deploy" >&2; exit 1; }
