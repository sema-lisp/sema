#!/usr/bin/env bash
# Guard: every publishable workspace crate must appear in the crates.io publish
# order in .github/workflows/publish.yml. This catches the "added a new crate but
# forgot to add it to the publish list" mistake — which once half-published a
# release (sema-llm failed: "no matching package named sema-otel") because the new
# sema-otel crate wasn't in the list. Run in CI so it fails BEFORE any publish.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WF="$ROOT/.github/workflows/publish.yml"

# Publishable crates = workspace members without `publish = false` (publish != []).
# sema-wasm is publish=false (it ships to npm, not crates.io) and is excluded here.
publishable=$(cargo metadata --no-deps --format-version 1 --manifest-path "$ROOT/Cargo.toml" \
  | jq -r '.packages[] | select(.publish != []) | .name' | sort)

# Crates named by `publish <crate>` lines in the workflow.
listed=$(grep -oE 'publish sema-[a-z]+' "$WF" | awk '{print $2}' | sort -u)

missing=$(comm -23 <(echo "$publishable") <(echo "$listed") || true)
if [ -n "$missing" ]; then
  echo "::error::Publishable crates missing from the publish order in $WF:"
  echo "$missing" | sed 's/^/  - /'
  echo "Add a 'publish <crate>' line in dependency order (before its dependents)."
  exit 1
fi

extra=$(comm -13 <(echo "$publishable") <(echo "$listed") || true)
if [ -n "$extra" ]; then
  echo "::warning::publish.yml lists crates that aren't publishable workspace members:"
  echo "$extra" | sed 's/^/  - /'
fi

echo "publish list OK: all $(echo "$publishable" | wc -l | tr -d ' ') publishable crates are present."
