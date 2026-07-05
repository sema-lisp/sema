# Coverage, mutation testing, and release guards. Namespaced as `release`.

# Fail if a publishable workspace crate is missing from publish.yml's order.
@group release
@desc "Guard: every publishable crate is in publish.yml's order"
task check-publish-list:
    ./scripts/check-publish-list.sh

# ── Coverage ─────────────────────────────────────────────────────────

@group coverage
@desc "Workspace coverage -> lcov.info"
@needs cargo-llvm-cov "cargo install cargo-llvm-cov"
task coverage:
    cargo llvm-cov --workspace --lcov --output-path lcov.info

@group coverage
@desc "Workspace coverage -> HTML report"
@needs cargo-llvm-cov "cargo install cargo-llvm-cov"
task coverage-html:
    cargo llvm-cov --workspace --html
    echo "Coverage report: target/llvm-cov/html/index.html"

# ── Mutation testing ─────────────────────────────────────────────────

@group mutants
@desc "Mutation testing (high-value stdlib crate)"
@needs cargo-mutants "cargo install cargo-mutants"
task mutants:
    echo "=== Mutation testing (sema-stdlib) ==="
    cargo mutants -p sema-stdlib --timeout 30 -- --test-threads=1

@group mutants
@desc "Mutation testing (sema-core)"
@needs cargo-mutants
task mutants-core:
    cargo mutants -p sema-core --timeout 30

@group mutants
@desc "Mutation testing (sema-notebook)"
@needs cargo-mutants
task mutants-notebook:
    cargo mutants -p sema-notebook --timeout 30

@group mutants
@desc "Full-workspace mutation testing (slow)"
@needs cargo-mutants
task mutants-all:
    echo "=== Full mutation testing (all crates, slow) ==="
    cargo mutants --workspace --timeout 60 -- --test-threads=1
