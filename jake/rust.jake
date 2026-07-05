# Rust / cargo — the daily drivers. Imported UNnamespaced so `jake build`,
# `jake test`, `jake lint` work bare, exactly like the old `make` targets.

# Crates that must stay clippy-clean with -D warnings (excludes optional/plugin crates).
clippy_crates = "-p sema-core -p sema-reader -p sema-eval -p sema-llm -p sema-stdlib -p sema-vm -p sema-lang -p sema-wasm"

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
    cargo run

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

# ── Test ─────────────────────────────────────────────────────────────

@group test
@desc "Run all tests (http/llm ignored)"
task test:
    @watch crates/**/*.rs Cargo.toml
    cargo test

@group test
@desc "Run the full workspace test suite"
task test-workspace:
    cargo test --workspace

@group test
@desc "LSP unit + e2e (pytest) tests"
task test-lsp: [release]
    @needs uv
    cargo test -p sema-lsp
    cd crates/sema-lsp/tests/e2e && uv run pytest -v

@group test
@desc "Embedding benchmark (ignored) test"
task test-embedding-bench:
    cargo test -p sema-lang --test embedding_bench -- --ignored --nocapture

@group test
@desc "HTTP integration tests (requires network)"
task test-http:
    cargo test -p sema-lang --test http_test -- --ignored --nocapture

@group test
@desc "LLM integration tests (requires API keys)"
@require ANTHROPIC_API_KEY OPENAI_API_KEY
task test-llm:
    cargo test -p sema-lang --test llm_test -- --ignored --nocapture

# ── Lint / format ────────────────────────────────────────────────────

@group lint
@desc "fmt-check + clippy -D warnings"
task lint: [fmt-check, clippy]
    echo "lint clean"

@group lint
@desc "clippy with -D warnings across the core crates"
task clippy:
    cargo clippy {{clippy_crates}} -- -D warnings

@group lint
@desc "Format the workspace"
task fmt:
    cargo fmt

@group lint
@desc "Check formatting without writing"
task fmt-check:
    cargo fmt -- --check
