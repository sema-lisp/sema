//! Model pricing lookup.
//!
//! Pricing comes from a single source of truth: a snapshot of [models.dev](https://models.dev)
//! (MIT-licensed data) vendored at `pricing-data.json` and embedded at build time via
//! `include_str!`. Refresh it with `jake update-pricing` (see
//! `scripts/update-pricing.sh`) and ship the diff in a patch release — we deliberately do
//! not fetch pricing at runtime (no dependency on a third-party endpoint we don't control;
//! see `docs/done/plans/2026-06-18-llm-pricing-models-dev.md`).
//!
//! Resolution order in [`model_pricing`]:
//! 1. Custom per-pattern overrides set via `(llm/set-pricing ...)`.
//! 2. The embedded models.dev snapshot.
//!
//! All prices are USD per 1,000,000 tokens.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::OnceLock;

use crate::types::Usage;

/// Vendored pricing snapshot, generated from models.dev by `jake update-pricing`.
const EMBEDDED_PRICING_JSON: &str = include_str!("pricing-data.json");

#[derive(Debug, serde::Deserialize)]
struct PricingData {
    updated_at: String,
    prices: Vec<PricingEntry>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, serde::Deserialize)]
struct PricingEntry {
    id: String,
    vendor: String,
    #[allow(dead_code)]
    #[serde(default)]
    name: String,
    input: f64,
    output: f64,
    #[allow(dead_code)]
    #[serde(default)]
    input_cached: Option<f64>,
    /// True for the canonical first-party listing of this id (the lab over resellers).
    /// Bare-id lookups use canonical entries; every vendor listing is still kept for
    /// provider-qualified lookups.
    #[serde(default = "default_true")]
    canonical: bool,
}

/// Parsed, indexed form of the embedded snapshot.
struct EmbeddedPricing {
    updated_at: String,
    /// Bare `id` → price, canonical (first-party) listing only.
    by_id: HashMap<String, (f64, f64)>,
    /// `"vendor/id"` → price, for every vendor listing (resellers/gateways included).
    by_qualified: HashMap<String, (f64, f64)>,
    /// `(id, input, output)` for longest-substring fallback matching (canonical only).
    entries: Vec<(String, f64, f64)>,
}

static EMBEDDED: OnceLock<EmbeddedPricing> = OnceLock::new();

fn embedded() -> &'static EmbeddedPricing {
    EMBEDDED.get_or_init(|| {
        // Invariant: pricing-data.json is vendored, compiled in, and validated by
        // `test_embedded_pricing_parses` — a parse failure here means a broken build.
        let data: PricingData = serde_json::from_str(EMBEDDED_PRICING_JSON)
            .expect("embedded pricing-data.json must be valid JSON (run `jake update-pricing`)");
        let mut by_id = HashMap::new();
        let mut by_qualified = HashMap::with_capacity(data.prices.len());
        let mut entries = Vec::new();
        for e in &data.prices {
            by_qualified.insert(format!("{}/{}", e.vendor, e.id), (e.input, e.output));
            if e.canonical {
                by_id.insert(e.id.clone(), (e.input, e.output));
                entries.push((e.id.clone(), e.input, e.output));
            }
        }
        EmbeddedPricing {
            updated_at: data.updated_at,
            by_id,
            by_qualified,
            entries,
        }
    })
}

thread_local! {
    static CUSTOM_PRICING: RefCell<HashMap<String, (f64, f64)>> = RefCell::new(HashMap::new());
}

fn lookup_custom(model: &str) -> Option<(f64, f64)> {
    CUSTOM_PRICING.with(|p| {
        p.borrow()
            .iter()
            .find(|(pattern, _)| model.contains(pattern.as_str()))
            .map(|(_, pricing)| *pricing)
    })
}

fn lookup_embedded(model: &str) -> Option<(f64, f64)> {
    let p = embedded();
    // Exact match on canonical bare id, then on an explicit "vendor/id".
    if let Some(price) = p.by_id.get(model).or_else(|| p.by_qualified.get(model)) {
        return Some(*price);
    }
    // Substring fallback: longest matching id wins (handles dated/suffixed variants).
    p.entries
        .iter()
        .filter(|(id, _, _)| model.contains(id.as_str()))
        .max_by_key(|(id, _, _)| id.len())
        .map(|(_, input, output)| (*input, *output))
}

/// Map a Sema provider name to its models.dev vendor key.
///
/// Known renames are aliased; any other provider name passes through unchanged, so a
/// provider added later whose name already matches a models.dev vendor (e.g. `azure`,
/// `openrouter`, `together`) gets correct per-vendor pricing with no code change here.
fn vendor_for_provider(provider: &str) -> &str {
    match provider {
        "gemini" => "google",
        "moonshot" => "moonshotai",
        other => other,
    }
}

/// Returns `(input_cost_per_million, output_cost_per_million)` for a model, using the
/// canonical first-party price.
pub fn model_pricing(model: &str) -> Option<(f64, f64)> {
    lookup_custom(model).or_else(|| lookup_embedded(model))
}

/// Provider-aware pricing: resolves the price for `model` as served by `provider`, so
/// resellers/gateways that list the same model id at a different price get their own rate.
/// Falls back to the canonical price when the provider doesn't list the model.
pub fn model_pricing_for(provider: &str, model: &str) -> Option<(f64, f64)> {
    if let Some(price) = lookup_custom(model) {
        return Some(price);
    }
    let qualified = format!("{}/{}", vendor_for_provider(provider), model);
    if let Some(price) = embedded().by_qualified.get(&qualified) {
        return Some(*price);
    }
    lookup_embedded(model)
}

fn cost_from(prices: (f64, f64), usage: &Usage) -> f64 {
    let (input, output) = prices;
    (usage.prompt_tokens as f64 * input + usage.completion_tokens as f64 * output) / 1_000_000.0
}

/// Calculate cost in USD from usage, using the canonical first-party price.
pub fn calculate_cost(usage: &Usage) -> Option<f64> {
    model_pricing(&usage.model).map(|prices| cost_from(prices, usage))
}

/// Provider-aware cost: like [`calculate_cost`] but prices the model as served by `provider`.
pub fn calculate_cost_for(provider: &str, usage: &Usage) -> Option<f64> {
    model_pricing_for(provider, &usage.model).map(|prices| cost_from(prices, usage))
}

/// Provider-aware cost split: `(prompt_cost, completion_cost)` in USD. Used to surface the
/// per-direction breakdown some backends (OpenInference `llm.cost.prompt`/`.completion`)
/// expect, alongside the combined total from [`calculate_cost_for`].
pub fn calculate_cost_split_for(provider: &str, usage: &Usage) -> Option<(f64, f64)> {
    model_pricing_for(provider, &usage.model).map(|(input, output)| {
        (
            usage.prompt_tokens as f64 * input / 1_000_000.0,
            usage.completion_tokens as f64 * output / 1_000_000.0,
        )
    })
}

/// Set custom pricing for a model pattern (substring match), overriding the embedded snapshot.
pub fn set_custom_pricing(model_pattern: &str, input_per_million: f64, output_per_million: f64) {
    CUSTOM_PRICING.with(|p| {
        p.borrow_mut().insert(
            model_pattern.to_string(),
            (input_per_million, output_per_million),
        );
    });
}

/// Clear all custom pricing overrides. Called when resetting interpreter runtime state.
pub fn clear_custom_pricing() {
    CUSTOM_PRICING.with(|p| p.borrow_mut().clear());
}

/// Snapshot (clone) the current thread's custom-pricing overrides. Custom pricing is
/// TASK-SNAPSHOT config: the LLM dynamic scope parks this map onto a suspended task so a
/// sibling's `(llm/set-pricing ...)` cannot reprice work already in flight (see
/// `builtins::LlmDynScope`). The map is small (per-call overrides), so a clone is cheap.
pub(crate) fn snapshot_custom_pricing() -> HashMap<String, (f64, f64)> {
    CUSTOM_PRICING.with(|p| p.borrow().clone())
}

/// Overwrite the current thread's custom-pricing overrides with `map`. The dynamic-scope
/// swap uses this to reinstall a task's own pricing snapshot when it is scheduled back in.
pub(crate) fn restore_custom_pricing(map: HashMap<String, (f64, f64)>) {
    CUSTOM_PRICING.with(|p| *p.borrow_mut() = map);
}

/// True when no custom-pricing overrides are active on this thread (fast-path predicate
/// for the LLM dynamic-scope empty check — no clone).
pub(crate) fn custom_pricing_is_empty() -> bool {
    CUSTOM_PRICING.with(|p| p.borrow().is_empty())
}

/// Returns the pricing source name and the snapshot's `updated_at` date.
pub fn pricing_status() -> (&'static str, Option<String>) {
    ("embedded", Some(embedded().updated_at.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_embedded_pricing_parses() {
        let p = embedded();
        assert!(
            p.entries.len() > 500,
            "expected a substantial snapshot, got {}",
            p.entries.len()
        );
        assert!(!p.updated_at.is_empty());
    }

    #[test]
    fn test_current_default_models_are_priced() {
        // Every provider's current default (see providers.md) must resolve to a price,
        // so llm/cost and budgets work out of the box.
        for model in [
            "claude-sonnet-4-6",
            "gpt-5.5",
            "gemini-3.5-flash",
            "grok-4.3",
            "mistral-large-latest",
            "kimi-k2.6",
            "llama-3.3-70b-versatile",
        ] {
            assert!(
                model_pricing(model).is_some(),
                "no embedded price for default model {model}"
            );
        }
    }

    #[test]
    fn test_known_prices_match_official() {
        // Spot-check canonical first-party prices (USD per 1M tokens).
        assert_eq!(model_pricing("gpt-5.5"), Some((5.0, 30.0)));
        assert_eq!(model_pricing("claude-sonnet-4-6"), Some((3.0, 15.0)));
        assert_eq!(model_pricing("grok-4.3"), Some((1.25, 2.5)));
        assert_eq!(model_pricing("mistral-large-latest"), Some((0.5, 1.5)));
    }

    #[test]
    fn test_qualified_vendor_id_lookup() {
        // "vendor/id" form resolves to the same price as the bare id.
        assert_eq!(
            model_pricing("anthropic/claude-sonnet-4-6"),
            model_pricing("claude-sonnet-4-6"),
        );
    }

    #[test]
    fn test_provider_aware_lookup_canonical() {
        // Provider-qualified lookup resolves to the first-party price.
        assert_eq!(model_pricing_for("openai", "gpt-5.5"), Some((5.0, 30.0)));
        assert_eq!(
            model_pricing_for("anthropic", "claude-sonnet-4-6"),
            Some((3.0, 15.0))
        );
    }

    #[test]
    fn test_provider_alias_mapping() {
        // Sema provider names that differ from models.dev vendor keys are aliased.
        assert_eq!(vendor_for_provider("gemini"), "google");
        assert_eq!(vendor_for_provider("moonshot"), "moonshotai");
        assert_eq!(vendor_for_provider("openai"), "openai");
        assert_eq!(
            model_pricing_for("gemini", "gemini-3.5-flash"),
            Some((1.5, 9.0))
        );
        assert_eq!(
            model_pricing_for("moonshot", "kimi-k2.6"),
            Some((0.95, 4.0))
        );
    }

    #[test]
    fn test_provider_unknown_falls_back_to_canonical() {
        // An unknown provider that doesn't list the model falls back to the canonical price.
        assert_eq!(
            model_pricing_for("some-future-provider", "gpt-5.5"),
            Some((5.0, 30.0))
        );
    }

    #[test]
    fn test_calculate_cost_for_provider() {
        let usage = Usage {
            prompt_tokens: 1_000_000,
            completion_tokens: 0,
            model: "gpt-5.5".to_string(),
            ..Default::default()
        };
        assert_eq!(calculate_cost_for("openai", &usage), Some(5.0));
    }

    #[test]
    fn test_substring_match_for_dated_variant() {
        // A dated/suffixed model id falls back to the longest matching base id.
        let dated = model_pricing("claude-sonnet-4-6-20260217");
        assert_eq!(dated, model_pricing("claude-sonnet-4-6"));
    }

    #[test]
    fn test_custom_pricing_wins_over_embedded() {
        set_custom_pricing("gpt-5.5", 1.0, 2.0);
        assert_eq!(model_pricing("gpt-5.5"), Some((1.0, 2.0)));
        clear_custom_pricing();
        // Back to embedded after clearing.
        assert_eq!(model_pricing("gpt-5.5"), Some((5.0, 30.0)));
    }

    #[test]
    fn test_unknown_model_returns_none() {
        clear_custom_pricing();
        assert!(model_pricing("totally-unknown-model-xyz-999").is_none());
    }

    #[test]
    fn test_calculate_cost() {
        let usage = Usage {
            prompt_tokens: 1_000_000,
            completion_tokens: 1_000_000,
            model: "gpt-5.5".to_string(),
            ..Default::default()
        };
        // 1M input @ $5 + 1M output @ $30 = $35.
        let cost = calculate_cost(&usage).unwrap();
        assert!((cost - 35.0).abs() < 1e-9, "got {cost}");
    }

    #[test]
    fn test_pricing_status_reports_embedded() {
        let (source, updated_at) = pricing_status();
        assert_eq!(source, "embedded");
        assert!(updated_at.is_some());
    }
}
