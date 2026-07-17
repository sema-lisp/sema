#!/usr/bin/env bash
#
# Regenerate the vendored LLM pricing snapshot from models.dev.
#
# Source:  https://models.dev/api.json  (data is MIT-licensed; see the snapshot header)
# Output:  crates/sema-llm/src/pricing-data.json
#
# models.dev is the single source of truth for model pricing (see
# docs/done/plans/2026-06-18-llm-pricing-models-dev.md). We embed a full snapshot at
# build time via include_str! and refresh it with minor patch releases rather than
# fetching at runtime. Run this, review the diff, and commit.
#
# Usage: jake update-pricing   (or: scripts/update-pricing.sh)

set -euo pipefail

API_URL="https://models.dev/api.json"
OUT="$(cd "$(dirname "$0")/.." && pwd)/crates/sema-llm/src/pricing-data.json"
GENERATED="$(date -u +%Y-%m-%d)"

echo "Fetching $API_URL ..."
RAW="$(mktemp)"
trap 'rm -f "$RAW"' EXIT
curl -fsSL --max-time 30 "$API_URL" -o "$RAW"

echo "Transforming to flat pricing schema ..."
# models.dev is a provider-keyed object: { <vendor>: { models: { <id>: { cost: {...} } } } }.
# Flatten to { updated_at, source, prices: [{ id, vendor, name, input, output, input_cached }] },
# keeping only models that publish both input and output cost (skips free/local and
# embedding-only entries). Cost units are USD per 1,000,000 tokens — unchanged.
#
# The same bare model id is listed under many vendors (the lab plus resellers/gateways),
# sometimes at divergent prices. We keep EVERY vendor listing so a future provider-aware
# lookup (e.g. "azure/gpt-5.5") resolves that vendor's exact price, and additionally flag the
# canonical first-party entry per id (preferring the lab over resellers via a vendor priority
# list, alphabetical fallback). A bare-id lookup uses the canonical entry → the official price.
jq --arg generated "$GENERATED" '
  def vrank($v):
    ["anthropic","openai","google","google-vertex","xai","mistral","moonshotai",
     "groq","deepseek","meta","cohere","ollama","ollama-cloud","together",
     "fireworks-ai","perplexity","azure","amazon-bedrock"]
    | (index($v) // 999);
  {
    updated_at: $generated,
    source: "models.dev",
    prices: [
      to_entries[]
      | .key as $vendor
      | (.value.models // {})
      | to_entries[]
      | .value as $m
      | select($m.cost != null and ($m.cost.input != null) and ($m.cost.output != null))
      | {
          id: $m.id,
          vendor: $vendor,
          name: ($m.name // $m.id),
          input: $m.cost.input,
          output: $m.cost.output,
          input_cached: ($m.cost.cache_read // null)
        }
    ]
    # Mark the canonical (highest-priority vendor) entry per id, keeping all entries.
    | group_by(.id)
    | map(
        (sort_by(vrank(.vendor), .vendor) | .[0].vendor) as $canon
        | map(. + { canonical: (.vendor == $canon) })
      )
    | add
    | sort_by(.id, vrank(.vendor), .vendor)
  }
' "$RAW" >"$OUT"

COUNT="$(jq '.prices | length' "$OUT")"
BYTES="$(wc -c <"$OUT" | tr -d ' ')"
echo "Wrote $OUT"
echo "  $COUNT priced models, $BYTES bytes, generated $GENERATED"
