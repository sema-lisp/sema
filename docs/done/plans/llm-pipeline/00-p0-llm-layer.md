# P0 — LLM Layer (Tasks 1-3)

> Parent plan: [../2026-02-17-llm-pipeline-stdlib.md](../2026-02-17-llm-pipeline-stdlib.md)

---

## Task 1: LLM Response Caching

**Files:**

- Modify: `crates/sema-llm/src/builtins.rs` (thread-locals, `do_complete`, new builtins)
- Test: `crates/sema/tests/integration_test.rs`

### Overview

Disk-based LLM response cache keyed by `sha256(model + temperature + messages_json + system_prompt)`.
Thread-local `HashMap<String, CachedResponse>` for in-memory hot cache.
JSON files in `~/.sema/cache/llm/` for persistence. TTL-based expiry.
Integrated transparently into `do_complete` via a thread-local enable flag.

### Step 1: Write failing tests

Add to `crates/sema/tests/integration_test.rs`:

```rust
#[test]
fn test_llm_cache_clear() {
    let result = eval("(llm/cache-clear)");
    assert_eq!(result, Value::int(0));
}

#[test]
fn test_llm_cache_stats_empty() {
    let result = eval("(llm/cache-stats)");
    let map = result.as_map_rc().expect("should be a map");
    assert!(map.contains_key(&Value::keyword("hits")));
    assert!(map.contains_key(&Value::keyword("misses")));
    assert!(map.contains_key(&Value::keyword("size")));
}

#[test]
fn test_llm_cache_key_generation() {
    let k1 = eval(r#"(llm/cache-key "hello" {:model "gpt-4" :temperature 0.5})"#);
    let k2 = eval(r#"(llm/cache-key "hello" {:model "gpt-4" :temperature 0.5})"#);
    assert_eq!(k1, k2);
    let k3 = eval(r#"(llm/cache-key "world" {:model "gpt-4" :temperature 0.5})"#);
    assert_ne!(k1, k3);
}

#[test]
fn test_llm_cache_key_different_model() {
    let k1 = eval(r#"(llm/cache-key "hello" {:model "gpt-4"})"#);
    let k2 = eval(r#"(llm/cache-key "hello" {:model "claude-3"})"#);
    assert_ne!(k1, k2);
}

#[test]
fn test_llm_cache_key_different_temperature() {
    let k1 = eval(r#"(llm/cache-key "hello" {:model "gpt-4" :temperature 0.0})"#);
    let k2 = eval(r#"(llm/cache-key "hello" {:model "gpt-4" :temperature 0.7})"#);
    assert_ne!(k1, k2);
}
```

**Run:** `cargo test -p sema --test integration_test -- test_llm_cache`
**Expected:** FAIL — functions not defined

### Step 2: Add cache data structures and thread-locals

In `crates/sema-llm/src/builtins.rs`, add after the existing thread-locals (line ~39):

```rust
use sha2::{Sha256, Digest};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct CachedResponse {
    content: String,
    model: String,
    prompt_tokens: u32,
    completion_tokens: u32,
    cached_at: i64, // unix timestamp
}

thread_local! {
    static CACHE_ENABLED: Cell<bool> = const { Cell::new(false) };
    static CACHE_MEM: RefCell<std::collections::HashMap<String, CachedResponse>> =
        RefCell::new(std::collections::HashMap::new());
    static CACHE_TTL_SECS: Cell<i64> = const { Cell::new(3600) }; // 1 hour default
    static CACHE_HITS: Cell<u64> = const { Cell::new(0) };
    static CACHE_MISSES: Cell<u64> = const { Cell::new(0) };
}
```

Add to `reset_runtime_state()`:

```rust
CACHE_ENABLED.with(|c| c.set(false));
CACHE_MEM.with(|c| c.borrow_mut().clear());
CACHE_TTL_SECS.with(|c| c.set(3600));
CACHE_HITS.with(|c| c.set(0));
CACHE_MISSES.with(|c| c.set(0));
```

### Step 3: Implement cache key generation

```rust
fn compute_cache_key(request: &ChatRequest) -> String {
    let mut hasher = Sha256::new();
    hasher.update(request.model.as_bytes());
    if let Some(temp) = request.temperature {
        hasher.update(temp.to_le_bytes());
    }
    if let Some(ref system) = request.system {
        hasher.update(system.as_bytes());
    }
    for msg in &request.messages {
        hasher.update(msg.role.as_bytes());
        hasher.update(msg.content.to_text().as_bytes());
    }
    format!("{:x}", hasher.finalize())
}
```

### Step 4: Implement cache disk I/O

Use `std::time::SystemTime` to avoid adding chrono dep to sema-llm:

```rust
fn unix_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn cache_dir() -> std::path::PathBuf {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
        .join(".sema").join("cache").join("llm")
}

fn cache_file_path(key: &str) -> std::path::PathBuf {
    cache_dir().join(format!("{key}.json"))
}

fn load_cached(key: &str) -> Option<CachedResponse> {
    let mem_hit = CACHE_MEM.with(|c| c.borrow().get(key).cloned());
    if let Some(cached) = mem_hit { return Some(cached); }
    let path = cache_file_path(key);
    let data = std::fs::read_to_string(&path).ok()?;
    let cached: CachedResponse = serde_json::from_str(&data).ok()?;
    CACHE_MEM.with(|c| c.borrow_mut().insert(key.to_string(), cached.clone()));
    Some(cached)
}

fn store_cached(key: &str, response: &ChatResponse) {
    let cached = CachedResponse {
        content: response.content.clone(),
        model: response.model.clone(),
        prompt_tokens: response.usage.prompt_tokens,
        completion_tokens: response.usage.completion_tokens,
        cached_at: unix_timestamp(),
    };
    CACHE_MEM.with(|c| c.borrow_mut().insert(key.to_string(), cached.clone()));
    let dir = cache_dir();
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(json) = serde_json::to_string(&cached) {
        let _ = std::fs::write(cache_file_path(key), json);
    }
}

fn is_cache_valid(cached: &CachedResponse) -> bool {
    let ttl = CACHE_TTL_SECS.with(|c| c.get());
    (unix_timestamp() - cached.cached_at) < ttl
}
```

### Step 5: Integrate caching into `do_complete`

Rename existing `do_complete` (line 2680) to `do_complete_uncached`, create new `do_complete`:

```rust
fn do_complete(mut request: ChatRequest) -> Result<ChatResponse, SemaError> {
    let cache_enabled = CACHE_ENABLED.with(|c| c.get());
    if cache_enabled {
        if request.model.is_empty() {
            let default_model = with_provider(|p| Ok(p.default_model().to_string()))?;
            request.model = default_model;
        }
        let cache_key = compute_cache_key(&request);
        if let Some(cached) = load_cached(&cache_key) {
            if is_cache_valid(&cached) {
                CACHE_HITS.with(|c| c.set(c.get() + 1));
                return Ok(ChatResponse {
                    content: cached.content,
                    role: "assistant".to_string(),
                    model: cached.model,
                    tool_calls: vec![],
                    usage: Usage {
                        prompt_tokens: cached.prompt_tokens,
                        completion_tokens: cached.completion_tokens,
                        model: request.model.clone(),
                    },
                    stop_reason: Some("cache_hit".to_string()),
                });
            }
        }
        CACHE_MISSES.with(|c| c.set(c.get() + 1));
        let response = do_complete_uncached(request)?;
        store_cached(&cache_key, &response);
        Ok(response)
    } else {
        do_complete_uncached(request)
    }
}

/// Original do_complete logic (provider dispatch + rate-limit retry).
fn do_complete_uncached(mut request: ChatRequest) -> Result<ChatResponse, SemaError> {
    with_provider(|p| {
        if request.model.is_empty() {
            request.model = p.default_model().to_string();
        }
        let mut retries = 0;
        let max_retries = 3;
        loop {
            match p.complete(request.clone()) {
                Ok(resp) => return Ok(resp),
                Err(crate::types::LlmError::RateLimited { retry_after_ms }) => {
                    retries += 1;
                    if retries > max_retries {
                        return Err(SemaError::Llm("rate limited after 3 retries".to_string()));
                    }
                    let wait = std::cmp::min(retry_after_ms, 30000);
                    std::thread::sleep(std::time::Duration::from_millis(wait));
                }
                Err(e) => return Err(SemaError::Llm(e.to_string())),
            }
        }
    })
}
```

### Step 6: Register builtin functions

Add inside `register_llm_builtins()`:

```rust
// (llm/cache-key prompt opts) — returns SHA-256 cache key for debugging
register_fn(env, "llm/cache-key", |args| {
    if args.is_empty() || args.len() > 2 {
        return Err(SemaError::arity("llm/cache-key", "1-2", args.len()));
    }
    let prompt = args[0].as_str()
        .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
    let mut model = String::new();
    let mut temperature = None;
    let mut system = None;
    if let Some(opts) = args.get(1).and_then(|v| v.as_map_rc()) {
        model = get_opt_string(&opts, "model").unwrap_or_default();
        temperature = get_opt_f64(&opts, "temperature");
        system = get_opt_string(&opts, "system");
    }
    let messages = vec![ChatMessage::new("user", prompt)];
    let mut request = ChatRequest::new(model, messages);
    request.temperature = temperature;
    request.system = system;
    Ok(Value::string(&compute_cache_key(&request)))
});

// (llm/cache-clear) — clear all cached responses, returns count cleared
register_fn(env, "llm/cache-clear", |_args| {
    let mem_count = CACHE_MEM.with(|c| {
        let mut cache = c.borrow_mut();
        let count = cache.len();
        cache.clear();
        count
    });
    let dir = cache_dir();
    if dir.exists() {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                if entry.path().extension().map(|e| e == "json").unwrap_or(false) {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
    }
    CACHE_HITS.with(|c| c.set(0));
    CACHE_MISSES.with(|c| c.set(0));
    Ok(Value::int(mem_count as i64))
});

// (llm/cache-stats) — return cache statistics
register_fn(env, "llm/cache-stats", |_args| {
    let hits = CACHE_HITS.with(|c| c.get());
    let misses = CACHE_MISSES.with(|c| c.get());
    let size = CACHE_MEM.with(|c| c.borrow().len());
    let mut map = BTreeMap::new();
    map.insert(Value::keyword("hits"), Value::int(hits as i64));
    map.insert(Value::keyword("misses"), Value::int(misses as i64));
    map.insert(Value::keyword("size"), Value::int(size as i64));
    Ok(Value::map(map))
});

// (llm/with-cache body-fn) or (llm/with-cache {:ttl 7200} body-fn)
register_fn_ctx(env, "llm/with-cache", |ctx, args| {
    if args.is_empty() || args.len() > 2 {
        return Err(SemaError::arity("llm/with-cache", "1-2", args.len()));
    }
    let (body_fn, ttl) = if args.len() == 2 {
        let opts = args[0].as_map_rc()
            .ok_or_else(|| SemaError::type_error("map", args[0].type_name()))?;
        let ttl = get_opt_u32(&opts, "ttl").unwrap_or(3600) as i64;
        (&args[1], ttl)
    } else {
        (&args[0], 3600i64)
    };
    if body_fn.as_lambda_rc().is_none() && body_fn.as_native_fn_rc().is_none() {
        return Err(SemaError::type_error("function", body_fn.type_name()));
    }
    let prev_enabled = CACHE_ENABLED.with(|c| c.get());
    let prev_ttl = CACHE_TTL_SECS.with(|c| c.get());
    CACHE_ENABLED.with(|c| c.set(true));
    CACHE_TTL_SECS.with(|c| c.set(ttl));
    let result = call_value_fn(ctx, body_fn, &[]);
    CACHE_ENABLED.with(|c| c.set(prev_enabled));
    CACHE_TTL_SECS.with(|c| c.set(prev_ttl));
    result
});
```

### Step 7: Run tests

**Run:** `cargo test -p sema --test integration_test -- test_llm_cache`
**Expected:** PASS

### Step 8: Commit

```bash
git add crates/sema-llm/src/builtins.rs crates/sema/tests/integration_test.rs
git commit -m "feat(llm): add LLM response caching with disk persistence and TTL"
```

---

## Task 2: Enhanced Retry with Reask

**Files:**

- Modify: `crates/sema-llm/src/builtins.rs` (lines 1157-1244, the `llm/extract` handler)
- Test: `crates/sema/tests/integration_test.rs` + unit tests in `builtins.rs`

### Overview

Enhance existing `llm/extract` retry. Currently it injects generic "Your previous response had
validation errors" message. The reask pattern should:

1. Make validation + retry the **default** (`:validate true :retries 2` by default)
2. Add `:reask? true` option that includes actual response + errors in structured format
3. Include the failed response content in retry prompt so LLM can see what it got wrong

### Step 1: Write failing tests

Add unit tests to `mod tests` block in `builtins.rs`:

```rust
#[test]
fn test_validate_extraction_missing_key() {
    let schema = {
        let mut map = BTreeMap::new();
        let mut name_spec = BTreeMap::new();
        name_spec.insert(Value::keyword("type"), Value::keyword("string"));
        map.insert(Value::keyword("name"), Value::map(name_spec));
        let mut age_spec = BTreeMap::new();
        age_spec.insert(Value::keyword("type"), Value::keyword("number"));
        map.insert(Value::keyword("age"), Value::map(age_spec));
        Value::map(map)
    };
    let result = {
        let mut map = BTreeMap::new();
        map.insert(Value::keyword("name"), Value::string("Alice"));
        Value::map(map)
    };
    let err = validate_extraction(&result, &schema).unwrap_err();
    assert!(err.contains("missing key: age"), "got: {err}");
}

#[test]
fn test_validate_extraction_wrong_type() {
    let schema = {
        let mut map = BTreeMap::new();
        let mut name_spec = BTreeMap::new();
        name_spec.insert(Value::keyword("type"), Value::keyword("string"));
        map.insert(Value::keyword("name"), Value::map(name_spec));
        Value::map(map)
    };
    let result = {
        let mut map = BTreeMap::new();
        map.insert(Value::keyword("name"), Value::int(42));
        Value::map(map)
    };
    let err = validate_extraction(&result, &schema).unwrap_err();
    assert!(err.contains("expected string"), "got: {err}");
}

#[test]
fn test_validate_extraction_valid() {
    let schema = {
        let mut map = BTreeMap::new();
        let mut name_spec = BTreeMap::new();
        name_spec.insert(Value::keyword("type"), Value::keyword("string"));
        map.insert(Value::keyword("name"), Value::map(name_spec));
        Value::map(map)
    };
    let result = {
        let mut map = BTreeMap::new();
        map.insert(Value::keyword("name"), Value::string("Alice"));
        Value::map(map)
    };
    assert!(validate_extraction(&result, &schema).is_ok());
}

#[test]
fn test_format_reask_prompt() {
    let prev_response = r#"{"name": 42}"#;
    let errors = "key name: expected string, got integer";
    let schema_desc = r#"{ "name": <string> }"#;
    let result = format_reask_prompt(prev_response, errors, schema_desc);
    assert!(result.contains("Previous response:"));
    assert!(result.contains(prev_response));
    assert!(result.contains(errors));
}
```

**Run:** `cargo test -p sema-llm -- test_validate test_format_reask`
**Expected:** FAIL — `format_reask_prompt` not defined

### Step 2: Add reask prompt formatter

Add helper near `format_schema` (line ~2618):

````rust
fn format_reask_prompt(prev_response: &str, errors: &str, schema_desc: &str) -> String {
    format!(
        "Your previous response did not match the required schema.\n\n\
         Previous response:\n```json\n{prev_response}\n```\n\n\
         Validation errors:\n{errors}\n\n\
         Please respond with ONLY a corrected JSON object matching this schema:\n\
         {schema_desc}\nDo not include any other text."
    )
}
````

### Step 3: Enhance `llm/extract` with reask behavior

Replace the `llm/extract` handler (lines 1157-1244). Key changes from original:

- Default `validate = true` (was `false`), `max_retries = 2` (was `0`)
- New `:reask?` option (default `true`) — includes previous response in retry
- Stores `last_response_content` for reask prompt

````rust
register_fn(env, "llm/extract", |args| {
    if args.len() < 2 || args.len() > 3 {
        return Err(SemaError::arity("llm/extract", "2-3", args.len()));
    }
    let schema = &args[0];
    let text = args[1].as_str()
        .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;

    let schema_desc = format_schema(schema);
    let system = format!(
        "Extract structured data from the text. Respond with ONLY a JSON object matching this schema:\n{}\nDo not include any other text.",
        schema_desc
    );
    let messages = vec![ChatMessage::new("user", text)];

    let mut model = String::new();
    let mut validate = true;       // changed from false
    let mut max_retries: u32 = 2;  // changed from 0
    let mut reask = true;          // new
    if let Some(opts_val) = args.get(2) {
        if let Some(opts) = opts_val.as_map_rc() {
            model = get_opt_string(&opts, "model").unwrap_or_default();
            if let Some(v) = opts.get(&Value::keyword("validate")) {
                validate = v.is_truthy();
            }
            if let Some(r) = get_opt_u32(&opts, "retries") { max_retries = r; }
            if let Some(v) = opts.get(&Value::keyword("reask?")) {
                reask = v.is_truthy();
            }
        }
    }

    let mut last_validation_error = String::new();
    let mut last_response_content = String::new();

    for attempt in 0..=max_retries {
        let mut request = ChatRequest::new(model.clone(), messages.clone());
        if attempt == 0 {
            request.system = Some(system.clone());
        } else if reask {
            request.system = Some(format_reask_prompt(
                &last_response_content, &last_validation_error, &schema_desc,
            ));
        } else {
            request.system = Some(format!(
                "{}\n\nYour previous response had validation errors: {}. Please fix.",
                system, last_validation_error
            ));
        }

        let response = do_complete(request)?;
        track_usage(&response.usage)?;

        let content = response.content.trim();
        let json_str = if content.starts_with("```") {
            content.trim_start_matches("```json").trim_start_matches("```")
                .trim_end_matches("```").trim()
        } else { content };
        let json: serde_json::Value = serde_json::from_str(json_str).map_err(|e| {
            SemaError::Llm(format!("failed to parse JSON: {e}\nResponse: {content}"))
        })?;
        let result = json_to_sema_value(&json);

        if validate {
            match validate_extraction(&result, schema) {
                Ok(()) => return Ok(result),
                Err(err) => {
                    last_validation_error = err;
                    last_response_content = content.to_string();
                    if attempt == max_retries {
                        return Err(SemaError::Llm(format!(
                            "extraction validation failed after {} attempt(s): {}",
                            max_retries + 1, last_validation_error
                        )));
                    }
                }
            }
        } else {
            return Ok(result);
        }
    }
    unreachable!()
});
````

### Step 4: Run tests

**Run:** `cargo test -p sema-llm -- test_validate test_format_reask`
**Expected:** PASS

### Step 5: Commit

```bash
git add crates/sema-llm/src/builtins.rs
git commit -m "feat(llm): enhance llm/extract with reask pattern and default validation"
```

---

## Task 3: Fallback Provider Chains

**Files:**

- Modify: `crates/sema-llm/src/builtins.rs` (new thread-local, modify dispatch, new builtins)
- Test: `crates/sema/tests/integration_test.rs` + unit tests in `builtins.rs`

### Overview

`llm/with-fallback` sets a thread-local list of provider names. When `do_complete` fails
(non-rate-limit), it tries the next provider. Uses `PROVIDER_REGISTRY.get(name)`.

### Step 1: Write failing tests

Integration tests:

```rust
#[test]
fn test_llm_providers_list() {
    let result = eval("(llm/providers)");
    assert!(result.as_list().is_some());
}

#[test]
fn test_llm_default_provider_none() {
    let result = eval("(llm/default-provider)");
    let is_valid = result.is_nil() || result.as_keyword().is_some();
    assert!(is_valid, "expected nil or keyword, got: {result}");
}
```

Unit test in `builtins.rs` `mod tests`:

```rust
#[test]
fn test_fallback_chain_thread_local() {
    FALLBACK_CHAIN.with(|chain| {
        assert!(chain.borrow().is_none());
        *chain.borrow_mut() = Some(vec!["openai".to_string(), "anthropic".to_string()]);
        assert_eq!(chain.borrow().as_ref().unwrap().len(), 2);
        *chain.borrow_mut() = None;
    });
}
```

**Run:** `cargo test -p sema --test integration_test -- test_llm_providers test_llm_default`
**Expected:** FAIL

### Step 2: Add fallback chain thread-local

```rust
thread_local! {
    static FALLBACK_CHAIN: RefCell<Option<Vec<String>>> = const { RefCell::new(None) };
}
```

Add to `reset_runtime_state()`:

```rust
FALLBACK_CHAIN.with(|c| *c.borrow_mut() = None);
```

### Step 3: Add provider-specific dispatch

```rust
fn do_complete_with_provider(
    provider_name: &str,
    mut request: ChatRequest,
) -> Result<ChatResponse, SemaError> {
    PROVIDER_REGISTRY.with(|reg| {
        let reg = reg.borrow();
        let provider = reg.get(provider_name).ok_or_else(|| {
            SemaError::Llm(format!("fallback provider '{}' not found", provider_name))
        })?;
        if request.model.is_empty() {
            request.model = provider.default_model().to_string();
        }
        let mut retries = 0;
        loop {
            match provider.complete(request.clone()) {
                Ok(resp) => return Ok(resp),
                Err(crate::types::LlmError::RateLimited { retry_after_ms }) => {
                    retries += 1;
                    if retries > 3 {
                        return Err(SemaError::Llm(format!(
                            "provider '{}' rate limited after 3 retries", provider_name
                        )));
                    }
                    let wait = std::cmp::min(retry_after_ms, 30000);
                    std::thread::sleep(std::time::Duration::from_millis(wait));
                }
                Err(e) => return Err(SemaError::Llm(e.to_string())),
            }
        }
    })
}
```

Update `do_complete` (or add wrapper between cache and uncached):

```rust
// Inside do_complete, after cache check, before calling do_complete_uncached:
fn do_complete_inner(request: ChatRequest) -> Result<ChatResponse, SemaError> {
    let fallback_chain = FALLBACK_CHAIN.with(|c| c.borrow().clone());
    match fallback_chain {
        Some(chain) if !chain.is_empty() => {
            let mut last_error = None;
            for provider_name in &chain {
                match do_complete_with_provider(provider_name, request.clone()) {
                    Ok(resp) => return Ok(resp),
                    Err(e) => {
                        eprintln!("Provider '{}' failed: {}, trying next...", provider_name, e);
                        last_error = Some(e);
                    }
                }
            }
            Err(last_error.unwrap_or_else(|| SemaError::Llm("all providers failed".into())))
        }
        _ => do_complete_uncached(request),
    }
}
```

### Step 4: Register builtins

```rust
// (llm/with-fallback '(:openai :anthropic :groq) body-fn)
register_fn_ctx(env, "llm/with-fallback", |ctx, args| {
    if args.len() != 2 {
        return Err(SemaError::arity("llm/with-fallback", "2", args.len()));
    }
    let providers = args[0].as_list()
        .ok_or_else(|| SemaError::type_error("list", args[0].type_name()))?;
    let body_fn = &args[1];
    if body_fn.as_lambda_rc().is_none() && body_fn.as_native_fn_rc().is_none() {
        return Err(SemaError::type_error("function", body_fn.type_name()));
    }
    let chain: Vec<String> = providers.iter().map(|v| {
        v.as_keyword().or_else(|| v.as_str().map(|s| s.to_string()))
            .ok_or_else(|| SemaError::type_error("keyword or string", v.type_name()))
    }).collect::<Result<_, _>>()?;

    let prev = FALLBACK_CHAIN.with(|c| c.borrow().clone());
    FALLBACK_CHAIN.with(|c| *c.borrow_mut() = Some(chain));
    let result = call_value_fn(ctx, body_fn, &[]);
    FALLBACK_CHAIN.with(|c| *c.borrow_mut() = prev);
    result
});

// (llm/providers) — list registered provider names
register_fn(env, "llm/providers", |_args| {
    let names = PROVIDER_REGISTRY.with(|reg| reg.borrow().provider_names());
    Ok(Value::list(names.into_iter().map(|n| Value::keyword(&n)).collect()))
});

// (llm/default-provider) — current default provider name
register_fn(env, "llm/default-provider", |_args| {
    let name = PROVIDER_REGISTRY.with(|reg| {
        reg.borrow().default_provider().map(|p| p.name().to_string())
    });
    match name {
        Some(n) => Ok(Value::keyword(&n)),
        None => Ok(Value::nil()),
    }
});
```

### Step 5: Run tests

**Run:** `cargo test -p sema-llm -- test_fallback`
**Run:** `cargo test -p sema --test integration_test -- test_llm_providers test_llm_default`
**Expected:** PASS

### Step 6: Commit

```bash
git add crates/sema-llm/src/builtins.rs crates/sema/tests/integration_test.rs
git commit -m "feat(llm): add fallback provider chains with llm/with-fallback"
```
