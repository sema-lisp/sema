# Docs — builtin-doc index, LLM pricing snapshot, markdown link check.
# Imported UNnamespaced (`jake docs`, `jake docs-check`).

@group docs
@desc "Regenerate the builtin doc index from crates/sema-docs/entries"
task docs:
    cargo run -q -p sema-docs -- gen

# CI gate: every entry has a summary (--strict), the committed index is current,
# and every registered builtin/special form is documented.
@group docs
@desc "Doc CI gate: strict gen + up-to-date index + coverage test"
task docs-check:
    cargo run -q -p sema-docs -- gen --strict
    git diff --exit-code crates/sema-docs/builtin_docs.generated.json
    cargo test -q -p sema-lsp builtin_doc_coverage

# Regenerate the vendored LLM pricing snapshot from models.dev. Commit the diff
# to ship updated prices in a patch release.
@group docs
@desc "Refresh the vendored LLM pricing snapshot (pricing-data.json)"
task update-pricing:
    ./scripts/update-pricing.sh

@group docs
@desc "Check markdown links with lychee"
@needs lychee "cargo install lychee"
task lint-links:
    lychee --config lychee.toml --no-progress '**/*.md'

# Hermetic gate: docs_search must work from the binary alone in a from-scratch
# container (no source, no uncompiled docs, --network none).
@group docs
@desc "Hermetic docs_search container gate (requires docker + jq)"
@needs docker jq
task docs-search-gate:
    ./scripts/docs-search-gate.sh
