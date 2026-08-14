# LLM pricing data: adopt models.dev as the source of truth

**Date:** 2026-06-18
**Status:** implemented
**Area:** `crates/sema-llm/src/pricing.rs`

## Implemented (final decisions)

- **Embed shape:** full set — `jake update-pricing` (`scripts/update-pricing.sh`) downloads
  models.dev `api.json`, normalizes to our flat schema keeping *every* vendor listing (canonical
  first-party flagged per id), and writes `crates/sema-llm/src/pricing-data.json` (~1 MB, 2,400+
  ids / 4,900+ listings), embedded via `include_str!`.
- **Runtime fetch:** removed entirely (no llm-prices.com, no disk cache). Refresh via patch
  release.
- **Hardcoded `match`:** removed.
- **Provider-aware lookup:** `model_pricing_for(provider, model)` / `calculate_cost_for` resolve
  the price as served by a specific provider (via a `provider → models.dev vendor` alias, unknown
  providers pass through); `track_usage` stamps the serving provider through `do_complete`'s
  dispatch arms. Bare-id lookups still use the canonical first-party price.

Original scoping below.

---

## Problem

`model_pricing()` resolves cost per model through three layers:

1. **Custom overrides** — `(llm/set-pricing "model" in out)` (thread-local).
2. **Fetched** — runtime download from `https://www.llm-prices.com/current-v1.json`, disk-cached at `~/.sema/pricing-cache.json`, refreshed during `llm/auto-configure`.
3. **Hardcoded fallback** — a `match` on model-name substrings, "last manually updated 2025-01".

The hardcoded fallback is the floor everyone hits offline / in sandbox / in CI / before the
first network refresh. It is **stale and incomplete**: none of the current default models
(`gpt-5.5`, `claude-sonnet-4-6`, `gemini-3.5-flash`, `grok-4.3`, `mistral-large-latest`,
`kimi-k2.6`, `gemma4`) are priced, so `llm/cost`, budgets, and `llm/usage` silently report
nothing for them. Maintaining it by hand has already drifted.

## Goal

A pricing baseline that is (a) current, (b) always available offline, and (c) cheap to keep
fresh — without hand-maintaining a `match` block.

## Source evaluation

### models.dev (recommended)

- **Repo:** `github.com/anomalyco/models.dev` (the registry behind SST/OpenCode). Active
  (~5k stars, ~daily commits). **Data is MIT-licensed** → safe to vendor/redistribute in this
  repo.
- **Consumable artifact:** `https://models.dev/api.json` — one built JSON file, **2.36 MB**,
  **145 providers / 5,276 models**.
- **Schema:** top-level object keyed by provider id; each model carries
  `cost: { input, output, cache_read, cache_write }` in **USD per 1,000,000 tokens** — the same
  unit `calculate_cost()` already assumes (divides by 1e6). Newer models add optional
  `cost.tiers[]` / `cost.context_over_200k` (long-context uplift) and
  `experimental.modes.<name>.cost`.
- **Coverage:** every Sema built-in provider's current default is present (verified against the
  live file). Ollama local models have no `cost` (free) but the catalog exists.
- **Gotchas:** model `id` is **bare** (`claude-opus-4-8`, not `anthropic/claude-opus-4-8`) — the
  provider comes from the top-level key, concatenate yourself. Many fields optional → liberal
  `Option` + `#[serde(default)]`. No git tags/releases (rolling `dev` branch) → a vendored
  snapshot pins to the commit we fetched.

### Alternatives (cross-check / fallback only)

- **LiteLLM `model_prices_and_context_window.json`** (`github.com/BerriAI/litellm`, MIT, ~1.5 MB,
  2,784 keys). Per-**token** units (×1e6 to compare), flatter schema, messier ids. Good as an
  accuracy cross-check.
- **OpenRouter `/api/v1/models`** — live JSON, per-token string prices. API ToS (not a
  redistributable data file) → runtime lookup only, never embed. OpenRouter-routed models only.

### Freshness + accuracy bake-off (2026-06-18)

Cross-checked all three live sources against official provider pricing pages for the six current
flagships (input/output per 1M tokens). **models.dev won decisively:**

| Model (official) | llm-prices.com | models.dev | LiteLLM |
|---|---|---|---|
| gpt-5.5 ($5/$30) | ✅ | ✅ | ❌ missing |
| claude-sonnet-4-6 ($3/$15) | ⚠️ folded into "Sonnet 4/4.5" | ✅ | ✅ |
| claude-opus-4-8 ($5/$25) | ✅ | ✅ | ✅ |
| gemini-3.5-flash ($1.50/$9) | ✅ | ✅ | ❌ missing |
| grok-4.3 ($1.25/$2.50) | ❌ missing | ✅ | ❌ missing |
| mistral-large-latest ($0.50/$1.50) | ❌ **stale: $2/$6** (4× off) | ✅ | — |
| kimi-k2.6 ($0.95/$4.00) | ❌ missing | ✅ | ❌ missing |
| **Score** | **3/6** | **6/6** | **2/6** |

Freshness signals: models.dev last commit **2026-06-17** (~1 day), llm-prices.com `updated_at`
**2026-06-09** (~9 days), LiteLLM file touched **2026-06-18** (same day, but slowest to add
first-party flagships). models.dev is the only source carrying grok-4.3 and kimi-k2.6 at all.

**Consequence for Decision B:** llm-prices.com is not just less fresh — it carries *wrong* data
(mistral-large 4× off, stale Sonnet folding). Keeping it as a runtime "override" on top of an
accurate models.dev embed would let stale data *overwrite correct prices*. So the freshness
research flips the earlier B1 recommendation → **single-source on models.dev**.

## Proposed design

### 1. Generator (offline, mirrors the existing builtin-docs regen pattern)

Add `jake update-pricing` (cf. `jake docs` at `Makefile:50` + its `git diff --exit-code` check):

- Download `models.dev/api.json`.
- Flatten + normalize to our existing flat schema
  `{ updated_at, prices: [{ id, vendor, input, output, input_cached }] }`
  (extend with `cache_read`/`cache_write` if we want accurate cache-cost reporting — optional).
- Write the committed snapshot. Doing the transform **in the script** keeps the Rust side simple
  and the build hermetic (no network at build time).

This makes "refresh prices" a one-command, reviewable diff — no more editing a `match`.

### 2. Embedded baseline (replaces the hardcoded `match`)

`include_str!` the committed snapshot, parse once into a `OnceLock<HashMap<…>>`, and serve it as
layer 3. Delete the stale `match`. Reuse the existing `lookup_fetched` matching logic
(exact id → `vendor/id` → longest-substring) for the embed too.

**Decision A — embed shape:**
- **(A1) Filtered subset** — only providers Sema ships built-in (≈ tens of KB). Lean binary, fast
  parse. Exotic user-configured OpenAI-compat models stay uncovered offline (they already are
  today; runtime fetch still covers them when online).
- **(A2) Full set, gzip-compressed** — `flate2` is already a transitive dep; full 2.36 MB
  compresses to ~400 KB, decompressed once at startup. Full offline coverage of all 5,276 models
  at a ~400 KB binary cost.

Recommendation: **A1** to start (covers every built-in default, smallest footprint); the
generator can widen the provider allowlist trivially if we want more later.

### 3. Runtime fetch+cache (freshness override) — keep, with a source decision

**Decision B — runtime source (revised after the bake-off):**
- ~~**(B1) Keep `llm-prices.com`**~~ — **rejected.** It carries wrong/stale prices (mistral-large
  4× off, no grok-4.3/kimi-k2.6); layering it over an accurate models.dev embed would overwrite
  correct prices with stale ones.
- **(B2) Repoint runtime fetch to `models.dev/api.json`.** Single source of truth. Needs a
  Rust-side normalizer for models.dev's nested schema — but that's the *same* transform the
  generator script does, so we can share the logic (or just reuse the script's flat output shape
  and have Rust parse models.dev natively only on the runtime path).
- **(B3) Drop runtime fetch entirely.** Rely on the embedded models.dev snapshot refreshed via
  `jake update-pricing` per release. Simplest; no runtime network, no 2.36 MB parse. Prices are
  only as fresh as the last build — acceptable since prices move slowly and the embed already
  comes from the best source.

Recommendation: **B3 for v1** (ship the accurate embed, drop the stale fetch, refresh per
release), with **B2 as a fast-follow** if we want sub-release freshness. Either way,
**llm-prices.com is removed.**

### Final precedence

- **B3 (v1):** `custom override → embedded models.dev snapshot`.
- **B2 (fast-follow):** `custom override → runtime-fetched models.dev (cache/network) → embedded
  models.dev snapshot`.

Either way the hand-maintained `match` **and** the llm-prices.com fetch are removed.

## Scope / effort

- Generator script + `jake update-pricing` target: ~half day.
- Embed plumbing (`include_str!` + `OnceLock`, delete `match`, wire as layer 3): ~half day.
- Tests: snapshot parses; known default models resolve a non-`None` price; precedence ordering;
  offline path returns embed not `None`. ~quarter day.
- Optional (A2 gzip / B2 models.dev runtime parser / cache_read cost): add ~half day each.

## Risks / notes

- **No versioned models.dev releases** → vendored snapshot is a point-in-time pin; `make
  update-pricing` + a release-checklist line keeps it current. Acceptable: prices move slowly and
  the runtime layer covers gaps.
- **Tiered/long-context pricing** ignored in v1 (we store flat base `input`/`output`). Fine for
  the common case; revisit if budget accuracy on >200k-context calls matters.
- **License hygiene:** add models.dev MIT attribution next to the vendored snapshot.
- Keep the existing `llm/set-pricing` override and `pricing_status` reporting (extend the latter
  to report `"embedded"` as a source alongside `"fetched"`/`"hardcoded"`).
