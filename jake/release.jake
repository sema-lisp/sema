# Coverage, mutation testing, and release guards. Namespaced as `release`.

# Fail if a publishable workspace crate is missing from the crates.io publish
# order (the publish-crates job in publish-npm.yml).
@group release
@desc "Guard: every publishable crate is in the crates.io publish order"
task check-publish-list:
    ./scripts/check-publish-list.sh

# ── Package-boundary gates ───────────────────────────────────────────
# Both wrap CI's release gates so the local `jake ci` and verify.yml run the
# same commands (no inline-CI-bash drift).

@group release
@desc "Packaged-crate gate: build `sema web` from the packed .crate and serve it"
task packaged-web:
    ./scripts/test-packaged-sema-web.sh

@group release
@desc "Guard: committed sema-web browser runtime is fresh (input fingerprint)"
task runtime-fresh:
    ./scripts/check-web-runtime-fresh.sh

# ── Changelog ────────────────────────────────────────────────────────
# CHANGELOG.md is assembled from changelog.d/ fragments; nothing edits it in
# place. See changelog.d/README.md for the fragment format. The assembler is
# written in Sema (scripts/changelog.sema), so it needs an installed `sema`.

@group release
@desc "Render the next CHANGELOG section from changelog.d/ (writes nothing)"
@needs sema "jake install"
task changelog-preview:
    sema scripts/changelog.sema -- preview

@group release
@desc "Assemble changelog.d/ into a ## X.Y.Z section and clear the fragments"
@needs sema "jake install"
task changelog-release version:
    sema scripts/changelog.sema -- release {{version}}

# ── Coverage ─────────────────────────────────────────────────────────

@group release
@desc "Workspace coverage -> lcov.info"
@needs cargo-llvm-cov "cargo install cargo-llvm-cov"
task coverage:
    cargo llvm-cov --workspace --lcov --output-path lcov.info

@group release
@desc "Workspace coverage -> HTML report"
@needs cargo-llvm-cov "cargo install cargo-llvm-cov"
task coverage-html:
    cargo llvm-cov --workspace --html
    echo "Coverage report: target/llvm-cov/html/index.html"

# ── Mutation testing ─────────────────────────────────────────────────

@group release
@desc "Mutation testing (high-value stdlib crate)"
@needs cargo-mutants "cargo install cargo-mutants"
task mutants:
    echo "=== Mutation testing (sema-stdlib) ==="
    cargo mutants -p sema-stdlib --timeout 30 -- --test-threads=1

@group release
@desc "Mutation testing (sema-core)"
@needs cargo-mutants
task mutants-core:
    cargo mutants -p sema-core --timeout 30

@group release
@desc "Mutation testing (sema-notebook)"
@needs cargo-mutants
task mutants-notebook:
    cargo mutants -p sema-notebook --timeout 30

@group release
@desc "Full-workspace mutation testing (slow)"
@needs cargo-mutants
task mutants-all:
    echo "=== Full mutation testing (all crates, slow) ==="
    cargo mutants --workspace --timeout 60 -- --test-threads=1
