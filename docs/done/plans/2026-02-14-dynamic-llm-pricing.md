# Dynamic LLM Pricing Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace stale hardcoded LLM pricing with dynamic pricing fetched from llm-prices.com, with offline-safe fallback chain.

**Status:** Implemented

**Architecture:** On `llm/auto-configure`, load cached pricing from `~/.sema/pricing-cache.json` (fast, sync), then attempt a best-effort HTTP fetch (1s timeout) to refresh it. Pricing lookup follows a fallback chain: user custom → fetched → hardcoded → None. All failures are silent — pricing never breaks the interpreter.

**Tech Stack:** Rust, serde_json, reqwest (already in sema-llm), chrono (already in workspace)

---

## Task 1: Add fetched pricing data structures and thread-local storage

**Files:**

- Modify: `crates/sema-llm/src/pricing.rs`

**Step 1: Write the failing test**

Add at the bottom of `crates/sema-llm/src/pricing.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fetched_pricing_overrides_hardcoded() {
        // Load a fetched entry for a known hardcoded model
        let json = r#"{
            "updated_at": "2025-10-10",
            "prices": [
                {"id": "gpt-4o-mini", "vendor": "openai", "name": "GPT-4o Mini", "input": 0.10, "output": 0.40, "input_cached": null}
            ]
        }"#;
        load_fetched_pricing_from_str(json).unwrap();

        // Fetched price should win over hardcoded
        let (input, output) = model_pricing("gpt-4o-mini").unwrap();
        assert!((input - 0.10).abs() < f64::EPSILON);
        assert!((output - 0.40).abs() < f64::EPSILON);

        // Clean up thread-local state
        clear_fetched_pricing();
    }

    #[test]
    fn test_hardcoded_fallback_when_no_fetch() {
        clear_fetched_pricing();
        // gpt-4o-mini has a hardcoded entry
        let result = model_pricing("gpt-4o-mini");
        assert!(result.is_some());
    }

    #[test]
    fn test_custom_pricing_wins_over_fetched() {
        let json = r#"{
            "updated_at": "2025-10-10",
            "prices": [
                {"id": "my-model", "vendor": "custom", "name": "My Model", "input": 1.0, "output": 2.0, "input_cached": null}
            ]
        }"#;
        load_fetched_pricing_from_str(json).unwrap();
        set_custom_pricing("my-model", 5.0, 10.0);

        let (input, output) = model_pricing("my-model").unwrap();
        assert!((input - 5.0).abs() < f64::EPSILON);
        assert!((output - 10.0).abs() < f64::EPSILON);

        clear_fetched_pricing();
        CUSTOM_PRICING.with(|p| p.borrow_mut().clear());
    }

    #[test]
    fn test_fetched_substring_matching() {
        let json = r#"{
            "updated_at": "2025-10-10",
            "prices": [
                {"id": "claude-sonnet-4", "vendor": "anthropic", "name": "Claude Sonnet 4", "input": 3.0, "output": 15.0, "input_cached": null}
            ]
        }"#;
        load_fetched_pricing_from_str(json).unwrap();

        // Model string with date suffix should still match
        let result = model_pricing("claude-sonnet-4-20250514");
        assert!(result.is_some());
        let (input, _) = result.unwrap();
        assert!((input - 3.0).abs() < f64::EPSILON);

        clear_fetched_pricing();
    }

    #[test]
    fn test_malformed_json_returns_error() {
        let result = load_fetched_pricing_from_str("not json");
        assert!(result.is_err());
    }

    #[test]
    fn test_unknown_model_returns_none() {
        clear_fetched_pricing();
        assert!(model_pricing("totally-unknown-model-xyz").is_none());
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p sema-llm -- pricing::tests -v`
Expected: FAIL — `load_fetched_pricing_from_str` and `clear_fetched_pricing` don't exist.

**Step 3: Implement the fetched pricing data structures**

At the top of `pricing.rs`, add the serde types and thread-local storage for fetched pricing. Add the new functions `load_fetched_pricing_from_str` and `clear_fetched_pricing`. Modify `model_pricing` to check fetched prices between custom and hardcoded.

Specifically:

1. Add `use chrono::NaiveDate;` and serde derives.
2. Define:

   ```rust
   #[derive(Debug, serde::Deserialize)]
   struct PricingResponse {
       updated_at: String,
       prices: Vec<PricingEntry>,
   }

   #[derive(Debug, serde::Deserialize)]
   struct PricingEntry {
       id: String,
       vendor: String,
       #[allow(dead_code)]
       name: String,
       input: f64,
       output: f64,
       #[allow(dead_code)]
       input_cached: Option<f64>,
   }

   /// In-memory fetched pricing: list of (id, vendor, input_per_million, output_per_million)
   struct FetchedPricing {
       entries: Vec<(String, String, f64, f64)>, // (id, vendor, input, output)
       updated_at: String,
   }
   ```

3. Add thread-local:
   ```rust
   thread_local! {
       static FETCHED_PRICING: RefCell<Option<FetchedPricing>> = const { RefCell::new(None) };
   }
   ```
4. Add `load_fetched_pricing_from_str(json: &str) -> Result<(), String>` that:
   - Parses the JSON into `PricingResponse`
   - Builds the `FetchedPricing` with entries as `(id, vendor, input, output)` tuples
   - Stores it in the thread-local
5. Add `clear_fetched_pricing()` that sets the thread-local to `None`.
6. Add `fn lookup_fetched(model: &str) -> Option<(f64, f64)>` that:
   - Tries exact match on `id`
   - Tries exact match on `vendor/id`
   - Tries substring: `model.contains(&entry.id)` (longest match wins to avoid e.g. "gpt-4o" matching before "gpt-4o-mini")
7. Update `model_pricing()` to insert a fetched lookup between custom and hardcoded:

   ```rust
   pub fn model_pricing(model: &str) -> Option<(f64, f64)> {
       // 1. Custom pricing (user overrides)
       let custom = CUSTOM_PRICING.with(|p| { ... });
       if custom.is_some() { return custom; }

       // 2. Fetched pricing (from llm-prices.com)
       if let Some(result) = lookup_fetched(model) {
           return Some(result);
       }

       // 3. Hardcoded fallback (may be stale)
       match model {
           ...
       }
   }
   ```

8. Add a public `pub fn fetched_pricing_updated_at() -> Option<String>` for use in status reporting.

**Step 4: Run tests to verify they pass**

Run: `cargo test -p sema-llm -- pricing::tests -v`
Expected: All 6 tests PASS.

**Step 5: Commit**

```bash
git add crates/sema-llm/src/pricing.rs
git commit -m "feat(pricing): add fetched pricing data structures with fallback chain"
```

---

## Task 2: Add disk cache read/write

**Files:**

- Modify: `crates/sema-llm/src/pricing.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn test_cache_roundtrip() {
    let dir = std::env::temp_dir().join("sema-pricing-test");
    let _ = std::fs::create_dir_all(&dir);
    let cache_path = dir.join("pricing-cache.json");

    let json = r#"{
        "updated_at": "2025-10-10",
        "prices": [
            {"id": "test-model", "vendor": "test", "name": "Test", "input": 1.5, "output": 3.0, "input_cached": null}
        ]
    }"#;

    // Write cache
    write_pricing_cache(&cache_path, json).unwrap();
    assert!(cache_path.exists());

    // Read cache back
    let loaded = read_pricing_cache(&cache_path).unwrap();
    assert!(loaded.is_some());
    let content = loaded.unwrap();
    assert!(content.contains("test-model"));

    // Clean up
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_cache_read_missing_file() {
    let result = read_pricing_cache(std::path::Path::new("/nonexistent/path/cache.json"));
    assert!(result.is_ok());
    assert!(result.unwrap().is_none());
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p sema-llm -- pricing::tests -v`
Expected: FAIL — functions don't exist.

**Step 3: Implement cache read/write**

Add two public functions:

```rust
/// Write pricing JSON to cache file (atomic: write temp + rename).
pub fn write_pricing_cache(path: &std::path::Path, json: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, json).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())?;
    Ok(())
}

/// Read pricing JSON from cache file. Returns Ok(None) if file doesn't exist.
pub fn read_pricing_cache(path: &std::path::Path) -> Result<Option<String>, String> {
    match std::fs::read_to_string(path) {
        Ok(content) => Ok(Some(content)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p sema-llm -- pricing::tests -v`
Expected: All PASS.

**Step 5: Commit**

```bash
git add crates/sema-llm/src/pricing.rs
git commit -m "feat(pricing): add disk cache read/write with atomic writes"
```

---

## Task 3: Add network fetch function

**Files:**

- Modify: `crates/sema-llm/src/pricing.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn test_fetch_pricing_url_is_correct() {
    assert_eq!(PRICING_URL, "https://www.llm-prices.com/current-v1.json");
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p sema-llm -- pricing::tests::test_fetch_pricing_url -v`
Expected: FAIL — constant doesn't exist.

**Step 3: Implement the fetch function**

Add:

```rust
pub const PRICING_URL: &str = "https://www.llm-prices.com/current-v1.json";

/// Fetch pricing from llm-prices.com with a short timeout.
/// Returns Ok(json_string) on success, Err on any failure.
/// This function uses tokio::block_on internally (same pattern as LLM providers).
pub fn fetch_pricing_from_remote() -> Result<String, String> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;

    rt.block_on(async {
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_millis(500))
            .timeout(std::time::Duration::from_secs(2))
            .build()
            .map_err(|e| e.to_string())?;

        let resp = client
            .get(PRICING_URL)
            .send()
            .await
            .map_err(|e| e.to_string())?;

        resp.text().await.map_err(|e| e.to_string())
    })
}
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p sema-llm -- pricing::tests -v`
Expected: All PASS (no network tests — the fetch is only tested by the URL constant).

**Step 5: Commit**

```bash
git add crates/sema-llm/src/pricing.rs
git commit -m "feat(pricing): add remote pricing fetch with short timeout"
```

---

## Task 4: Add the orchestrator function called from auto-configure

**Files:**

- Modify: `crates/sema-llm/src/pricing.rs`
- Modify: `crates/sema-llm/src/builtins.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn test_refresh_pricing_loads_cache_fallback() {
    // With no network and no cache, should not panic and hardcoded should still work
    clear_fetched_pricing();
    let fake_cache = std::env::temp_dir().join("sema-pricing-noexist").join("cache.json");
    refresh_pricing(Some(&fake_cache));
    // Hardcoded should still work
    assert!(model_pricing("gpt-4o-mini").is_some());
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p sema-llm -- pricing::tests -v`
Expected: FAIL — `refresh_pricing` doesn't exist.

**Step 3: Implement the orchestrator**

Add to `pricing.rs`:

```rust
/// Best-effort pricing refresh. Called during llm/auto-configure.
/// 1. Load disk cache into memory (fast, sync)
/// 2. Attempt network fetch (short timeout, swallow errors)
/// 3. On success, update memory + write disk cache
pub fn refresh_pricing(cache_path: Option<&std::path::Path>) {
    let cache = cache_path.and_then(|p| {
        read_pricing_cache(p).ok().flatten()
    });

    // Load cache into memory
    if let Some(ref json) = cache {
        let _ = load_fetched_pricing_from_str(json);
    }

    // Try network fetch (best-effort)
    match fetch_pricing_from_remote() {
        Ok(json) => {
            if load_fetched_pricing_from_str(&json).is_ok() {
                if let Some(p) = cache_path {
                    let _ = write_pricing_cache(p, &json);
                }
            }
        }
        Err(_) => {
            // Network unavailable — silently continue with cache or hardcoded
        }
    }
}

/// Return the default cache path: ~/.sema/pricing-cache.json
pub fn default_cache_path() -> Option<std::path::PathBuf> {
    std::env::var("HOME")
        .ok()
        .map(|home| std::path::PathBuf::from(home).join(".sema").join("pricing-cache.json"))
}
```

Then in `builtins.rs`, inside the `llm/auto-configure` closure, add at the end (after all providers are registered, before the return):

```rust
// Best-effort pricing refresh
pricing::refresh_pricing(pricing::default_cache_path().as_deref());
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p sema-llm -- pricing::tests -v`
Expected: All PASS.

Also run full test suite: `cargo test`
Expected: All PASS. Non-LLM tests unaffected.

**Step 5: Commit**

```bash
git add crates/sema-llm/src/pricing.rs crates/sema-llm/src/builtins.rs
git commit -m "feat(pricing): wire up refresh_pricing into llm/auto-configure"
```

---

## Task 5: Add `llm/pricing-status` builtin

**Files:**

- Modify: `crates/sema-llm/src/builtins.rs`
- Modify: `crates/sema-llm/src/pricing.rs`

**Step 1: Write the failing integration test**

Add to `crates/sema/tests/integration_test.rs`:

```rust
#[test]
fn test_llm_pricing_status() {
    let interp = Interpreter::new();
    let result = interp.eval_str("(llm/pricing-status)").unwrap();
    // Should return a map with :source key
    match &result {
        Value::Map(m) => {
            assert!(m.contains_key(&Value::keyword("source")));
        }
        _ => panic!("expected map, got {result}"),
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p sema --test integration_test -- test_llm_pricing_status -v`
Expected: FAIL — unbound variable.

**Step 3: Implement the builtin**

In `pricing.rs`, add:

```rust
/// Returns the current pricing source info: source name and updated_at if available.
pub fn pricing_status() -> (&'static str, Option<String>) {
    let has_fetched = FETCHED_PRICING.with(|f| f.borrow().is_some());
    if has_fetched {
        let updated = fetched_pricing_updated_at();
        ("fetched", updated)
    } else {
        ("hardcoded", None)
    }
}
```

In `builtins.rs`, register:

```rust
// (llm/pricing-status)
register_fn(env, "llm/pricing-status", |_args| {
    let (source, updated_at) = pricing::pricing_status();
    let mut map = std::collections::BTreeMap::new();
    map.insert(Value::keyword("source"), Value::symbol(source));
    if let Some(date) = updated_at {
        map.insert(Value::keyword("updated-at"), Value::string(date));
    }
    Ok(Value::Map(Rc::new(map)))
});
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p sema --test integration_test -- test_llm_pricing_status -v`
Expected: PASS.

**Step 5: Commit**

```bash
git add crates/sema-llm/src/pricing.rs crates/sema-llm/src/builtins.rs crates/sema/tests/integration_test.rs
git commit -m "feat: add (llm/pricing-status) builtin"
```

---

## Task 6: Add budget-with-unknown-pricing warning

**Files:**

- Modify: `crates/sema-llm/src/builtins.rs`

**Step 1: Write the failing test**

Add to `crates/sema/tests/integration_test.rs`:

```rust
#[test]
fn test_budget_with_unknown_model_does_not_error() {
    let interp = Interpreter::new();
    // Set a budget — this should not cause errors for unknown models
    // We can't easily test LLM calls without a provider, but we can test
    // that budget-remaining works when nothing has been spent
    let result = interp.eval_str("(begin (llm/set-budget 1.00) (llm/budget-remaining))").unwrap();
    match &result {
        Value::Map(m) => {
            assert!(m.contains_key(&Value::keyword("limit")));
        }
        _ => panic!("expected map, got {result}"),
    }
}
```

**Step 2: Run test to verify it passes** (this may already pass — it's a guard)

Run: `cargo test -p sema --test integration_test -- test_budget_with_unknown_model -v`

**Step 3: Update `track_usage` to handle unknown pricing gracefully**

In `builtins.rs`, modify the `track_usage` function. When `cost` is `None` and a budget is set, add a one-time `eprintln!` warning instead of silently skipping. Add a thread-local bool to avoid repeating the warning:

```rust
thread_local! {
    static PRICING_WARNING_SHOWN: Cell<bool> = const { Cell::new(false) };
}
```

In the `track_usage` function, after the `if let Some(c) = cost` block, add an `else` branch:

```rust
} else {
    // Cost unknown — warn once if budget is active
    BUDGET_LIMIT.with(|limit| {
        if limit.borrow().is_some() {
            PRICING_WARNING_SHOWN.with(|shown| {
                if !shown.get() {
                    shown.set(true);
                    eprintln!(
                        "Warning: pricing unknown for model '{}'; budget enforcement is best-effort",
                        usage.model
                    );
                }
            });
        }
    });
}
```

**Step 4: Run tests to verify they pass**

Run: `cargo test`
Expected: All PASS.

**Step 5: Commit**

```bash
git add crates/sema-llm/src/builtins.rs crates/sema/tests/integration_test.rs
git commit -m "feat(pricing): warn once when budget is set but pricing is unknown"
```

---

## Task 7: Update hardcoded pricing table comment

**Files:**

- Modify: `crates/sema-llm/src/pricing.rs`

**Step 1: Update the doc comment on the hardcoded table**

Change the comment above the hardcoded match block from:

```rust
// Built-in pricing table
```

to:

```rust
// Built-in pricing table (fallback estimates — may be outdated).
// Dynamic pricing from llm-prices.com is preferred when available.
// Last manually updated: 2025-01.
// Override with (llm/set-pricing "model" input output).
```

**Step 2: Update the Groq and Moonshot entries**

Groq models are no longer free. Update:

```rust
// Groq (paid — prices vary by model, these are estimates)
m if m.contains("llama") && !m.contains("ollama") => Some((0.10, 0.30)),
m if m.contains("mixtral") => Some((0.25, 0.25)),
m if m.contains("gemma") => Some((0.10, 0.20)),
```

Remove the `// free tier` comment. Note the `!m.contains("ollama")` guard so Ollama llama models don't match.

For Moonshot:

```rust
// Moonshot (estimates)
m if m.contains("moonshot") => Some((0.50, 1.50)),
```

**Step 3: Verify**

Run: `cargo test -p sema-llm -- pricing::tests -v`
Expected: All PASS.

Run: `make lint`
Expected: PASS.

**Step 4: Commit**

```bash
git add crates/sema-llm/src/pricing.rs
git commit -m "fix(pricing): update stale hardcoded prices, add fallback disclaimer"
```

---

## Task 8: Update documentation — cost tracking page

**Files:**

- Modify: `website/docs/llm/cost.md`

**Step 1: Update the cost docs**

Add a new section before "Budget Enforcement" explaining pricing sources:

````markdown
## Pricing Sources

Sema tracks LLM costs using pricing data from multiple sources, checked in this order:

1. **Custom pricing** — set via `(llm/set-pricing "model" input output)`, always wins
2. **Dynamic pricing** — fetched from [llm-prices.com](https://www.llm-prices.com) during `(llm/auto-configure)`, cached locally at `~/.sema/pricing-cache.json`
3. **Built-in estimates** — hardcoded fallback table (may be outdated)
4. **Unknown** — if no source matches, cost tracking returns `nil` and budget enforcement is best-effort

Dynamic pricing is fetched with a short timeout (2s) and failures are silently ignored. The language works fully offline — the cache persists between sessions.

### `llm/pricing-status`

Check which pricing source is active and when it was last updated.

```scheme
(llm/pricing-status)
; => {:source fetched :updated-at "2025-10-10"}
; or {:source hardcoded} if no dynamic pricing is available
```
````

````

Update the `llm/set-pricing` section to clarify it overrides all other sources:

```markdown
### `llm/set-pricing`

Set custom pricing for a model (overrides both dynamic and built-in pricing). Costs are per million tokens.

```scheme
(llm/set-pricing "my-model" 1.0 3.0)   ; $1.00/M input, $3.00/M output
````

````

Add a note to Budget Enforcement:

```markdown
> **Note:** If pricing is unknown for a model (not in any source), budget enforcement operates in best-effort mode — the call proceeds with a one-time warning. Use `(llm/set-pricing)` to set pricing for unlisted models.
````

**Step 2: Verify docs render correctly**

Visually inspect the markdown for formatting issues.

**Step 3: Commit**

```bash
git add website/docs/llm/cost.md
git commit -m "docs: update cost tracking docs with dynamic pricing sources"
```

---

## Task 9: Update README pricing section

**Files:**

- Modify: `README.md`

**Step 1: Update the Cost Tracking section**

In the README, find the "Cost Tracking & Budgets" section (around line 870). Update to mention dynamic pricing:

````markdown
### Cost Tracking & Budgets

```scheme
(llm/last-usage)                       ; => {:prompt-tokens 42 :completion-tokens 15 ...}
(llm/session-usage)                    ; => cumulative usage across all calls
(llm/reset-usage)                      ; reset session counters

;; Budget enforcement
(llm/set-budget 1.00)                  ; set $1.00 spending limit
(llm/budget-remaining)                 ; => {:limit 1.0 :spent 0.05 :remaining 0.95}
(llm/clear-budget)                     ; remove limit

;; Pricing sources (checked in order: custom > dynamic > built-in)
(llm/pricing-status)                   ; => {:source fetched :updated-at "2025-10-10"}
(llm/set-pricing "my-model" 1.0 3.0)  ; custom pricing (per million tokens)
```
````

Pricing data is fetched from [llm-prices.com](https://www.llm-prices.com) during auto-configure and cached locally. Works fully offline with built-in fallback estimates.

````

**Step 2: Verify**

Read the updated section to check formatting.

**Step 3: Commit**

```bash
git add README.md
git commit -m "docs: update README cost tracking section with dynamic pricing"
````

---

## Task 10: Update CHANGELOG

**Files:**

- Modify: `CHANGELOG.md`

**Step 1: Add entry at the top**

Add under a new unreleased section or the current version:

```markdown
## Unreleased

### Added

- **Dynamic LLM pricing** — pricing data is now fetched from [llm-prices.com](https://www.llm-prices.com) during `(llm/auto-configure)` and cached at `~/.sema/pricing-cache.json`. Falls back to built-in estimates when offline. Custom pricing via `(llm/set-pricing)` always takes priority.
- **`llm/pricing-status`** — new builtin to inspect which pricing source is active and when it was last updated.

### Fixed

- **Stale Groq pricing** — Groq models are no longer hardcoded as free ($0.00); updated to current estimates.
- **Budget enforcement with unknown pricing** — now warns once instead of silently skipping cost tracking when pricing is unavailable for a model.
```

**Step 2: Commit**

```bash
git add CHANGELOG.md
git commit -m "docs: add changelog entry for dynamic pricing"
```

---

## Task 11: Update AGENTS.md / CLAUDE.md

**Files:**

- Modify: `AGENTS.md`

**Step 1: Update the architecture notes**

In the `sema-llm` description, update from:

```
**sema-llm** → LLM provider trait + Anthropic/OpenAI clients (tokio `block_on`)
```

to:

```
**sema-llm** → LLM provider trait + Anthropic/OpenAI clients (tokio `block_on`), dynamic pricing from llm-prices.com with disk cache fallback
```

**Step 2: Commit**

```bash
git add AGENTS.md
git commit -m "docs: update AGENTS.md with dynamic pricing note"
```

---

## Task 12: Final verification

**Step 1: Run full test suite**

Run: `cargo test`
Expected: All tests PASS.

**Step 2: Run lint**

Run: `make lint`
Expected: PASS (fmt-check + clippy -D warnings).

**Step 3: Manual smoke test**

Run: `cargo run -- -e '(llm/pricing-status)'`
Expected: `{:source hardcoded}` (no auto-configure without LLM flag).

Run: `cargo run -- -e '(begin (llm/auto-configure) (llm/pricing-status))'`
Expected: Either `{:source fetched :updated-at "..."}` (if online) or `{:source hardcoded}` (if offline). No crash either way.

Run: `cargo run -- --no-llm -e '(+ 1 2)'`
Expected: `3` — non-LLM code completely unaffected.

**Step 4: Commit any final fixes**

```bash
git commit -m "chore: final verification pass"
```
