# Rust / cargo — the daily drivers. Imported UNnamespaced so `jake build`,
# `jake test`, `jake lint` work bare, exactly like the old `make` targets.


# ── Build ────────────────────────────────────────────────────────────

@default
@group build
@desc "Dev build"
task build:
    @needs cargo
    @watch crates/**/*.rs Cargo.toml
    cargo build

@group build
@desc "Optimized release build"
task release:
    @needs cargo
    cargo build --release

@group build
@desc "Optimized local build with incremental compilation"
task release-fast:
    @needs cargo
    cargo build --profile release-fast

# PGO build (instrument -> train -> rebuild). ~25% faster on 1BRC; see
# docs/performance-roadmap.md.
@group build
@desc "PGO build (instrument -> train -> rebuild)"
task build-pgo:
    ./scripts/pgo-build.sh

@group build
@desc "Emit only the PGO .profdata that CI consumes (target/pgo/merged.profdata)"
task pgo-profile:
    ./scripts/pgo-build.sh --profile-only

@group build
@desc "Type-check without codegen"
task check:
    cargo check

@group build
@desc "Remove cargo build artifacts"
task clean:
    cargo clean

@group dev
@desc "Start the REPL"
task run:
    cargo run --bin sema

# ── Install ──────────────────────────────────────────────────────────

@group install
@desc "Install sema to ~/.cargo/bin"
task install:
    cargo install --path crates/sema

# PGO-optimized install: instrument -> train -> rebuild, then drop the binary
# into the cargo bin dir. Slower to build, faster at runtime.
@group install
@desc "PGO-optimized install into the cargo bin dir"
task install-pgo: [build-pgo]
    install -m 0755 target/release/sema "${CARGO_HOME:-$HOME/.cargo}/bin/sema"
    echo "Installed PGO-optimized sema -> ${CARGO_HOME:-$HOME/.cargo}/bin/sema"

@group install
@desc "Uninstall the sema binary"
task uninstall:
    cargo uninstall sema-lang

@group install
@desc "Uninstall then reinstall sema to ~/.cargo/bin"
task reinstall: [uninstall, install]

# ── Test ─────────────────────────────────────────────────────────────

@group test
@desc "Run all tests (http/llm ignored)"
task test:
    @needs cargo-nextest
    @watch crates/**/*.rs Cargo.toml
    cargo nextest run

# Test variants (workspace, lsp, http, llm, e2e, providers) live in
# jake/test.jake, namespaced as `test.*` (jake test.http, jake test.lsp, …).

# ── Lint / format ────────────────────────────────────────────────────

@group lint
@desc "fmt-check + clippy -D warnings"
task lint: [fmt-check, clippy]
    echo "lint clean"

# --workspace, matching CI exactly: a crate subset here once let `jake lint`
# pass locally while CI failed on the excluded crates.
@group lint
@desc "clippy with -D warnings across the whole workspace"
task clippy:
    cargo clippy --workspace -- -D warnings

@group lint
@desc "Format the workspace"
task fmt:
    cargo fmt

@group lint
@desc "Check formatting without writing"
task fmt-check:
    cargo fmt -- --check
