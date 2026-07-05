# Jake migration — replacing the Makefile with jakefile.dev

**Date:** 2026-07-03 · **Status (2026-07-05):** adopted **alongside** the Makefile in `main`; jake#17–#20 fixed (jake 0.9.3). Split-adapted — no editors/grammar module (those moved to their own repos, each with a `@rooted` Jakefile); the mono keeps only its own browser E2E (in `jake/examples.jake`). Makefile + CI workflows unchanged for now — the full switch (retire Makefile, rewire CI to jake) is a later step.

Originally a spike to evaluate replacing the 393-line `Makefile` with [Jake](https://jakefile.dev)
(`helgesverre/jake`, dogfooding our own tool). The jake#17 blocker below has since been fixed;
the Jakefile now lives in `main` next to the Makefile.

## What was built

A modular `Jakefile` + `jake/*.jake` set porting the whole Makefile (96 recipes, 23 groups):

| File | Namespace | Covers |
| --- | --- | --- |
| `Jakefile` | — | imports + aggregate pipelines (`all`, `ci`, `deploy`, `deploy-all`) |
| `jake/rust.jake` | *(bare)* | build/release/pgo, install, test*, lint/clippy/fmt |
| `jake/docs.jake` | *(bare)* | doc index, pricing, link check, docs-search-gate |
| `jake/examples.jake` | *(bare)* | examples, examples-build, smoke-bytecode, notebooks, llm/provider smokes |
| `jake/wasm.jake` | `wasm` | WASM VM build + **shared** browser-runtime vendoring |
| `jake/ui.jake` | `ui` | `@sema/ui` build + **vendoring into notebook/playground** |
| `jake/web.jake` | `site` | VitePress docs site build/preview/deploy/og |
| `jake/playground.jake` | `pg` | sema.run WASM playground build/dev/deploy |
| `jake/bench.jake` | `bench` | hyperfine suites, 1BRC ladder, samply profile |
| `jake/fuzz.jake` | `fuzz` | cargo-fuzz + in-Sema grammar fuzzer |
| `jake/editors.jake` | `ed` | tree-sitter grammar, **VS Code + IntelliJ extension packaging/publishing**, browser E2E |
| `jake/release.jake` | `release` | coverage, mutation testing, publish-list guard |

The daily drivers (`build`, `test`, `lint`, `fmt`, `run`) are imported **un-namespaced** so muscle
memory carries over (`jake build` == `make build`). Everything else is namespaced to keep `jake -l`
navigable.

## Why Jake is a real improvement here (verified)

1. **Discoverability.** `jake -l` groups 96 recipes under 23 `@group`s with a `@desc` each; `jake -s <r>`
   shows deps/params/commands; typo suggestions. The Makefile is a flat 50-target `.PHONY` wall.
2. **`@needs` pre-flight.** Recipes declare their external tools (`wasm-pack`, `vercel`/`npx`, `uv`,
   `docker`, `jq`, `lychee`, `samply`, `hyperfine`, `cargo-llvm-cov`, `cargo-mutants`) with install
   hints. Verified: a missing tool fails **before** any command runs, with the custom hint. The
   Makefile just explodes mid-recipe with a cryptic `command not found`.
3. **`@cd` kills the `cd X && …` noise** across website/playground/ui/tree-sitter recipes.
4. **`@confirm` on deploys.** `site.deploy` / `pg.deploy` / `deploy-all` now prompt before
   `vercel --prod`; the Makefile fired `--prod --yes` immediately.
5. **DRY vendoring.** The web-runtime copy-list (~8 files) was duplicated between `web-runtime` and
   `sema-web-example-build` in the Makefile; here it's one private `wasm._vendor-runtime <dir>`
   parameterized recipe.
6. **Parallelism.** `jake -j` runs independent recipes concurrently (site + wasm + ui builds), with
   dependency ordering respected. No `make -j` foot-guns.
7. **Params instead of `$(if $(N),…)` soup.** `bench`, `profile`, `fuzz.grammar` take clean
   `name=value` params (`jake fuzz.grammar n=20000 seed=123`) with `@if`/`@else` for optional flags.

Verified end-to-end: `jake -l` (parse), dependency + cross-namespace resolution (`ci`,
`ed.test-web-e2e -> wasm.web-runtime`, bare `[build]` from a namespaced import), param/conditional
expansion, and a **real** `jake fmt-check` run against the repo (`cargo fmt --check`, exit 0).

## The monorepo vendoring story (the main motivation)

`@sema/ui` (Lit components) is **not npm-published**; it's built once (`ui/dist/sema-ui.js`) and
**copied** into each consumer — currently the notebook crate (`crates/sema-notebook/src/ui/vendor/`,
embedded via `include_str!`), with the playground to follow (notebook-ui-refactor branch). The same
pattern already exists for the sema-web browser runtime.

Modeled as a two-stage `file`-recipe chain:

```
file ui/dist/sema-ui.js: ui/src/**/* …        # stage 1: build the bundle
file crates/sema-notebook/.../sema-ui.js: ui/dist/sema-ui.js   # stage 2: re-copy
task ui.vendor: [vendor-notebook, vendor-playground]
```

This is the ideal shape: re-vendor only when `ui/src` actually changed, one `jake ui.vendor` for all
consumers. **However — see the blocker.**

## ⚠️ Blocker: incremental `file` recipes are currently a no-op (jake#17)

Jake 0.8.1's incremental caching does not work in the CLI path: **`file` recipes and `@cache` always
re-run**, and `.jake/cache` stays 0 bytes. Root-caused to `Executor.initWithIndexAndContext` copying
`runtime.cache` **by value** (`src/executor.zig:123`) — the executor mutates its copy, but
`RuntimeContext.deinit` saves the un-updated original. Filed as
[jake#17](https://github.com/HelgeSverre/jake/issues/17) with the source analysis and a suggested fix
(share the cache by pointer, like `environment`/`hook_runner`).

**Consequence for adoption:** with the bug present, our `file` recipes behave exactly like the
Makefile's always-copy targets — i.e. **no regression**, just no incremental win yet. Once jake#17
lands, `ui.vendor` / wasm vendoring become genuinely incremental with zero Jakefile changes.

Also filed while spiking:
- [jake#18](https://github.com/HelgeSverre/jake/issues/18) — import error doesn't name the missing file.
- [jake#19](https://github.com/HelgeSverre/jake/issues/19) — `--fmt` hoists/merges leading comments across
  directive-decorated recipes (so **don't** run `jake --fmt` on these files until fixed; the source is
  intentionally comment-documented).

## Recommended path

1. **Land jake#17** (and ideally #18/#19) in `../jake`, cut a patch, reinstall.
2. **Adopt incrementally, keep the Makefile as a thin shim** during transition (e.g. `build:\n\tjake build`)
   so CI and muscle memory don't break on day one.
3. **Point CI at jake** once the E2E/examples/bytecode gates are proven under `jake ci`.
4. **Wire the playground `@sema/ui` vendor path** (`playground/vendor/sema-ui.js` is provisional here)
   when notebook-ui-refactor merges, then `jake ui.vendor` covers both consumers.
5. **`jake --install`** for shell completions across recipe names.

## Notes / gotchas found

- File targets from a namespaced import display as `ui.ui/dist/sema-ui.js` in `-l`/dry-run, but the
  real path in the recipe body (`ui/dist/sema-ui.js`) is what's used for the fs check — cosmetic.
- Parameterizing a dependency isn't supported (deps are bare names); use a subprocess call
  (`jake wasm._vendor-runtime <dir>`) to pass an argument to a shared recipe.
- `@cd` in a `file` recipe changes the command cwd only; the output path in the recipe name stays
  relative to the repo root (jake's cwd). Works as intended for `cd ui && npm run build`.
