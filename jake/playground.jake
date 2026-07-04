# sema.run WASM playground. Namespaced as `pg`.

# Build the WASM VM into the playground, then generate examples.js from the
# .sema files. wasm-pack/cargo are already incremental; the file recipe keeps
# the JS regen (node build.mjs) from re-running when nothing changed.
@group playground
@desc "Build the playground (WASM VM + examples bundle)"
task build:
    @needs wasm-pack "cargo install wasm-pack"
    @needs node
    cd crates/sema-wasm && wasm-pack build --target web --out-dir ../../playground/pkg \
        -- --config 'profile.release.package.sema-wasm.opt-level="s"'
    cd playground && node build.mjs

@group playground
@desc "Build + serve the playground at :8787"
task dev: [build]
    @needs npx node
    cd playground && node scripts/gen-devtools-json.mjs
    cd playground && npx serve -l 8787

@group playground
@desc "Build + deploy the playground to production (Vercel)"
@needs npx
task deploy: [build]
    @confirm "Deploy the playground to production?"
    @cd playground
    npx vercel --prod --yes
