# Embedded web runtime distribution

Every supported Sema installation must ship a working `sema web` command. The browser runtime is part of the language distribution, not an optional developer-generated add-on.

## Distribution invariant

The `sema-lang` crate and every native release artifact contain the complete browser runtime:

- the Sema Web JavaScript bundle;
- the Sema JavaScript API and storage backends;
- the WebAssembly VM and its JavaScript loader;
- the signals and DOM-diffing dependencies.

`sema web` extracts these embedded bytes and serves them without network access. It never asks an installed user to run Jake, npm, wasm-pack, or a repository-only build command.

## Packaging design

The generated runtime under `crates/sema/src/web/assets/` is a tracked release input. Keeping the artifacts in the `sema-lang` package makes the same source work for all installation paths: crates.io, `cargo install`, cargo-dist archives, shell installers, and Homebrew.

The internal `jake wasm.web-runtime` recipe remains the maintainer command that regenerates these files from the Rust and JavaScript sources. It does not participate in an end-user build.

Normal builds always compile `runtime.rs` with `include_bytes!`. There is no reduced `web_runtime` configuration. Missing or renamed assets therefore fail compilation instead of silently producing a binary with a broken public command.

## Alternatives rejected

A separate runtime crate would also make the assets publishable, but adds another versioned package and another publish-order dependency without improving the single-binary result.

Generating the runtime only in the cargo-dist workflow would repair GitHub and Homebrew artifacts but not the source uploaded to crates.io. Generating it in `build.rs` would force Cargo users to install Node.js, npm, wasm-pack, and a WASM target, and would make crate builds depend on tooling and network state.

## Verification

Regression coverage operates at three boundaries:

1. A source-level test rejects any reintroduction of the optional-runtime configuration or the end-user Jake fallback.
2. A package smoke test creates the actual `sema-lang` `.crate`, verifies every embedded asset is present, builds from its unpacked contents, starts `sema web`, and checks its HTTP shell.
3. The release workflow runs the packaged-artifact smoke test before cargo-dist builds distributable binaries.

The existing server integration test must fail when `sema web` exits early; missing runtime support is not a reason to skip a test.

## Release impact

The fix requires a patch release because all v1.30.0 native artifacts and the crates.io package expose a nonfunctional `sema web` command. The release notes should identify affected installation methods and tell users to upgrade; they should not offer Jake as an end-user workaround.
