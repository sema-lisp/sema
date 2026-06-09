# P2 — Resilience & Convenience (Tasks 10-15)

> Parent plan: [../2026-02-17-llm-pipeline-stdlib.md](../2026-02-17-llm-pipeline-stdlib.md)

---

## Task 10: Rate Limiting

**Files:**

- Modify: `crates/sema-llm/src/builtins.rs`
- Test: `crates/sema/tests/integration_test.rs`

### Overview

Token-bucket rate limiter. `llm/with-rate-limit` wraps a body with a max requests-per-second.
Thread-local token bucket state. Sleeps when tokens exhausted.

### Step 1: Write failing tests

```rust
#[test]
fn test_llm_with_rate_limit_type_check() {
    let interp = Interpreter::new();
    // Should accept a number and a function
    let result = interp.eval_str(r#"(llm/with-rate-limit 5 (lambda () 42))"#);
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), Value::int(42));
}
```

### Step 2: Implement

Add thread-local:

```rust
thread_local! {
    static RATE_LIMIT_RPS: Cell<Option<f64>> = const { Cell::new(None) };
    static RATE_LIMIT_LAST: Cell<u64> = const { Cell::new(0) }; // last call timestamp ms
}
```

Add to `reset_runtime_state()`:

```rust
RATE_LIMIT_RPS.with(|r| r.set(None));
RATE_LIMIT_LAST.with(|r| r.set(0));
```

Add rate-limit check in `do_complete` or `do_complete_inner` (before calling provider):

```rust
fn enforce_rate_limit() {
    let rps = RATE_LIMIT_RPS.with(|r| r.get());
    if let Some(rps) = rps {
        let min_interval_ms = (1000.0 / rps) as u64;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64).unwrap_or(0);
        let last = RATE_LIMIT_LAST.with(|l| l.get());
        if last > 0 && now - last < min_interval_ms {
            let sleep_ms = min_interval_ms - (now - last);
            std::thread::sleep(std::time::Duration::from_millis(sleep_ms));
        }
        let actual_now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64).unwrap_or(0);
        RATE_LIMIT_LAST.with(|l| l.set(actual_now));
    }
}
```

Register builtin:

```rust
register_fn_ctx(env, "llm/with-rate-limit", |ctx, args| {
    if args.len() != 2 { return Err(SemaError::arity("llm/with-rate-limit", "2", args.len())); }
    let rps = args[0].as_float()
        .or_else(|| args[0].as_int().map(|i| i as f64))
        .ok_or_else(|| SemaError::type_error("number", args[0].type_name()))?;
    let body_fn = &args[1];
    if body_fn.as_lambda_rc().is_none() && body_fn.as_native_fn_rc().is_none() {
        return Err(SemaError::type_error("function", body_fn.type_name()));
    }
    let prev = RATE_LIMIT_RPS.with(|r| r.get());
    RATE_LIMIT_RPS.with(|r| r.set(Some(rps)));
    let result = call_value_fn(ctx, body_fn, &[]);
    RATE_LIMIT_RPS.with(|r| r.set(prev));
    result
});
```

### Step 3: Run tests, commit

**Run:** `cargo test -p sema --test integration_test -- test_llm_with_rate_limit`
**Expected:** PASS

```bash
git add crates/sema-llm/src/builtins.rs crates/sema/tests/integration_test.rs
git commit -m "feat(llm): add token-bucket rate limiting with llm/with-rate-limit"
```

---

## Task 11: Generic Retry / Exponential Backoff

**Files:**

- Modify: `crates/sema-stdlib/src/meta.rs` (or new file — check convention)
- Test: `crates/sema/tests/integration_test.rs`

### Overview

Generic `retry` function that calls a thunk up to N times with exponential backoff.
Lives in sema-stdlib (not LLM-specific). Options: `:max-attempts`, `:base-delay-ms`, `:backoff` (multiplier).

### Step 1: Write failing tests

```rust
#[test]
fn test_retry_succeeds_first_try() {
    let result = eval(r#"(retry (lambda () 42))"#);
    assert_eq!(result, Value::int(42));
}

#[test]
fn test_retry_with_options() {
    let result = eval(r#"(retry (lambda () 42) {:max-attempts 3})"#);
    assert_eq!(result, Value::int(42));
}

#[test]
fn test_retry_counter() {
    // Use a mutable counter via set! to verify retries happen
    let interp = Interpreter::new();
    interp.eval_str(r#"(define counter 0)"#).unwrap();
    interp.eval_str(r#"
        (retry (lambda ()
            (set! counter (+ counter 1))
            (if (< counter 3)
                (error "not yet")
                counter))
            {:max-attempts 5 :base-delay-ms 0})
    "#).unwrap();
    let count = interp.eval_str("counter").unwrap();
    assert_eq!(count, Value::int(3));
}

#[test]
fn test_retry_exhausted() {
    let interp = Interpreter::new();
    let result = interp.eval_str(
        r#"(retry (lambda () (error "always fails")) {:max-attempts 2 :base-delay-ms 0})"#
    );
    assert!(result.is_err());
}
```

### Step 2: Implement

Since `retry` needs to call a lambda and catch errors, it needs `register_fn_ctx` and
the `call_value_fn` pattern. However, sema-stdlib doesn't have `call_value_fn` — it uses
`sema_core::call_callback`. Check how `map`/`filter` work in list.rs.

Actually, looking at the codebase, higher-order fns in stdlib call through
`sema_core::call_callback` which dispatches to the eval callback. So:

Add to `crates/sema-stdlib/src/meta.rs` (which already has `apply`, `error`, etc.):

```rust
    // (retry thunk) or (retry thunk {:max-attempts 3 :base-delay-ms 100 :backoff 2.0})
    register_fn(env, "retry", |args| {
        if args.is_empty() || args.len() > 2 {
            return Err(SemaError::arity("retry", "1-2", args.len()));
        }
        let thunk = &args[0];
        if thunk.as_lambda_rc().is_none() && thunk.as_native_fn_rc().is_none() {
            return Err(SemaError::type_error("function", thunk.type_name()));
        }

        let mut max_attempts: u32 = 3;
        let mut base_delay_ms: u64 = 100;
        let mut backoff: f64 = 2.0;

        if let Some(opts) = args.get(1).and_then(|v| v.as_map_rc()) {
            if let Some(v) = opts.get(&Value::keyword("max-attempts")).and_then(|v| v.as_int()) {
                max_attempts = v.max(1) as u32;
            }
            if let Some(v) = opts.get(&Value::keyword("base-delay-ms")).and_then(|v| v.as_int()) {
                base_delay_ms = v.max(0) as u64;
            }
            if let Some(v) = opts.get(&Value::keyword("backoff")).and_then(|v| v.as_float()) {
                backoff = v;
            }
        }

        let mut last_error = None;
        for attempt in 0..max_attempts {
            match sema_core::call_callback(thunk, &[]) {
                Ok(val) => return Ok(val),
                Err(e) => {
                    last_error = Some(e);
                    if attempt + 1 < max_attempts && base_delay_ms > 0 {
                        let delay = (base_delay_ms as f64 * backoff.powi(attempt as i32)) as u64;
                        std::thread::sleep(std::time::Duration::from_millis(delay));
                    }
                }
            }
        }
        Err(last_error.unwrap())
    });
```

**Note:** This uses `sema_core::call_callback` which dispatches through the thread-local
eval callback. Verify this works by checking how `map`/`filter` call lambdas in `list.rs`.

### Step 3: Run tests, commit

**Run:** `cargo test -p sema --test integration_test -- test_retry`
**Expected:** PASS

```bash
git add crates/sema-stdlib/src/meta.rs crates/sema/tests/integration_test.rs
git commit -m "feat(stdlib): add generic retry with exponential backoff"
```

---

## Task 12: `llm/summarize`

**Files:**

- Modify: `crates/sema-llm/src/builtins.rs`
- Test: `crates/sema/tests/integration_test.rs`

### Overview

Convenience wrapper around `llm/complete` with a summarization system prompt.
Options: `:max-length`, `:style` (`:paragraph`, `:bullet-points`, `:one-line`).

### Step 1: Write failing tests

These can't call real LLMs so test argument validation only:

```rust
#[test]
fn test_llm_summarize_arity() {
    let interp = Interpreter::new();
    let result = interp.eval_str(r#"(llm/summarize)"#);
    assert!(result.is_err());
}
```

### Step 2: Implement

```rust
register_fn_gated(env, sandbox, sema_core::Caps::LLM, "llm/summarize", |args| {
    if args.is_empty() || args.len() > 2 {
        return Err(SemaError::arity("llm/summarize", "1-2", args.len()));
    }
    let text = args[0].as_str()
        .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;

    let mut model = String::new();
    let mut max_length: Option<u32> = None;
    let mut style = "paragraph".to_string();

    if let Some(opts) = args.get(1).and_then(|v| v.as_map_rc()) {
        model = get_opt_string(&opts, "model").unwrap_or_default();
        max_length = get_opt_u32(&opts, "max-length");
        if let Some(s) = get_opt_string(&opts, "style") { style = s; }
    }

    let style_instruction = match style.as_str() {
        "bullet-points" | "bullets" => "Use bullet points.",
        "one-line" => "Respond with a single sentence summary.",
        _ => "Write a concise paragraph summary.",
    };
    let length_instruction = match max_length {
        Some(n) => format!(" Keep the summary under {} words.", n),
        None => String::new(),
    };
    let system = format!("Summarize the following text. {style_instruction}{length_instruction}");

    let messages = vec![ChatMessage::new("user", text)];
    let mut request = ChatRequest::new(model, messages);
    request.system = Some(system);
    request.max_tokens = Some(4096);

    let response = do_complete(request)?;
    track_usage(&response.usage)?;
    Ok(Value::string(&response.content))
});
```

### Step 3: Run tests, commit

```bash
git add crates/sema-llm/src/builtins.rs crates/sema/tests/integration_test.rs
git commit -m "feat(llm): add llm/summarize convenience wrapper"
```

---

## Task 13: `llm/compare`

**Files:**

- Modify: `crates/sema-llm/src/builtins.rs`
- Test: `crates/sema/tests/integration_test.rs`

### Step 1: Write failing tests

```rust
#[test]
fn test_llm_compare_arity() {
    let interp = Interpreter::new();
    let result = interp.eval_str(r#"(llm/compare "a")"#);
    assert!(result.is_err()); // needs 2-3 args
}
```

### Step 2: Implement

````rust
register_fn_gated(env, sandbox, sema_core::Caps::LLM, "llm/compare", |args| {
    if args.len() < 2 || args.len() > 3 {
        return Err(SemaError::arity("llm/compare", "2-3", args.len()));
    }
    let text_a = args[0].as_str()
        .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
    let text_b = args[1].as_str()
        .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;

    let mut model = String::new();
    if let Some(opts) = args.get(2).and_then(|v| v.as_map_rc()) {
        model = get_opt_string(&opts, "model").unwrap_or_default();
    }

    let system = "Compare the following two texts. Respond with ONLY a JSON object containing:\n\
        - \"similarity\": a number from 0.0 (completely different) to 1.0 (identical)\n\
        - \"differences\": a list of key differences\n\
        - \"summary\": a brief comparison summary\n\
        Do not include any other text.".to_string();

    let user_msg = format!("Text A:\n{text_a}\n\nText B:\n{text_b}");
    let messages = vec![ChatMessage::new("user", &user_msg)];
    let mut request = ChatRequest::new(model, messages);
    request.system = Some(system);

    let response = do_complete(request)?;
    track_usage(&response.usage)?;

    let content = response.content.trim();
    let json_str = if content.starts_with("```") {
        content.trim_start_matches("```json").trim_start_matches("```")
            .trim_end_matches("```").trim()
    } else { content };
    let json: serde_json::Value = serde_json::from_str(json_str).map_err(|e| {
        SemaError::Llm(format!("failed to parse comparison JSON: {e}\nResponse: {content}"))
    })?;
    Ok(json_to_sema_value(&json))
});
````

### Step 3: Run tests, commit

```bash
git add crates/sema-llm/src/builtins.rs crates/sema/tests/integration_test.rs
git commit -m "feat(llm): add llm/compare for semantic text comparison"
```

---

## Task 14: Persistent Key-Value Store

**Files:**

- Create: `crates/sema-stdlib/src/kv.rs`
- Modify: `crates/sema-stdlib/src/lib.rs` (add `mod kv`, register)
- Test: `crates/sema/tests/integration_test.rs`

### Overview

Simple JSON-file-backed KV store. Thread-local `HashMap<String, KvStore>` holding open stores.
Each store is a JSON object on disk. All values stored as Sema → JSON → Sema round-trip.

### Step 1: Write failing tests

```rust
#[test]
fn test_kv_open_and_close() {
    let interp = Interpreter::new();
    let tmp = std::env::temp_dir().join("sema-kv-test-oc.json");
    let path = tmp.to_str().unwrap();
    let _ = std::fs::remove_file(&tmp);
    interp.eval_str(&format!(r#"(kv/open "test" "{path}")"#)).unwrap();
    interp.eval_str(r#"(kv/close "test")"#).unwrap();
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn test_kv_set_and_get() {
    let interp = Interpreter::new();
    let tmp = std::env::temp_dir().join("sema-kv-test-sg.json");
    let path = tmp.to_str().unwrap();
    let _ = std::fs::remove_file(&tmp);
    interp.eval_str(&format!(r#"(kv/open "sg" "{path}")"#)).unwrap();
    interp.eval_str(r#"(kv/set "sg" "name" "Alice")"#).unwrap();
    let result = interp.eval_str(r#"(kv/get "sg" "name")"#).unwrap();
    assert_eq!(result, Value::string("Alice"));
    interp.eval_str(r#"(kv/close "sg")"#).unwrap();
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn test_kv_get_missing() {
    let interp = Interpreter::new();
    let tmp = std::env::temp_dir().join("sema-kv-test-gm.json");
    let path = tmp.to_str().unwrap();
    let _ = std::fs::remove_file(&tmp);
    interp.eval_str(&format!(r#"(kv/open "gm" "{path}")"#)).unwrap();
    let result = interp.eval_str(r#"(kv/get "gm" "missing")"#).unwrap();
    assert!(result.is_nil());
    interp.eval_str(r#"(kv/close "gm")"#).unwrap();
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn test_kv_delete() {
    let interp = Interpreter::new();
    let tmp = std::env::temp_dir().join("sema-kv-test-del.json");
    let path = tmp.to_str().unwrap();
    let _ = std::fs::remove_file(&tmp);
    interp.eval_str(&format!(r#"(kv/open "del" "{path}")"#)).unwrap();
    interp.eval_str(r#"(kv/set "del" "k" "v")"#).unwrap();
    interp.eval_str(r#"(kv/delete "del" "k")"#).unwrap();
    let result = interp.eval_str(r#"(kv/get "del" "k")"#).unwrap();
    assert!(result.is_nil());
    interp.eval_str(r#"(kv/close "del")"#).unwrap();
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn test_kv_keys() {
    let interp = Interpreter::new();
    let tmp = std::env::temp_dir().join("sema-kv-test-keys.json");
    let path = tmp.to_str().unwrap();
    let _ = std::fs::remove_file(&tmp);
    interp.eval_str(&format!(r#"(kv/open "keys" "{path}")"#)).unwrap();
    interp.eval_str(r#"(kv/set "keys" "a" 1)"#).unwrap();
    interp.eval_str(r#"(kv/set "keys" "b" 2)"#).unwrap();
    let result = interp.eval_str(r#"(kv/keys "keys")"#).unwrap();
    let keys = result.as_list().unwrap();
    assert_eq!(keys.len(), 2);
    interp.eval_str(r#"(kv/close "keys")"#).unwrap();
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn test_kv_persistence() {
    let tmp = std::env::temp_dir().join("sema-kv-test-persist.json");
    let path = tmp.to_str().unwrap();
    let _ = std::fs::remove_file(&tmp);
    // Write
    {
        let interp = Interpreter::new();
        interp.eval_str(&format!(r#"(kv/open "p" "{path}")"#)).unwrap();
        interp.eval_str(r#"(kv/set "p" "key" "value")"#).unwrap();
        interp.eval_str(r#"(kv/close "p")"#).unwrap();
    }
    // Read back
    {
        let interp = Interpreter::new();
        interp.eval_str(&format!(r#"(kv/open "p" "{path}")"#)).unwrap();
        let result = interp.eval_str(r#"(kv/get "p" "key")"#).unwrap();
        assert_eq!(result, Value::string("value"));
        interp.eval_str(r#"(kv/close "p")"#).unwrap();
    }
    let _ = std::fs::remove_file(&tmp);
}
```

**Run:** `cargo test -p sema --test integration_test -- test_kv`
**Expected:** FAIL

### Step 2: Create `kv.rs`

Create `crates/sema-stdlib/src/kv.rs`:

```rust
use std::cell::RefCell;
use std::collections::HashMap;

use sema_core::{SemaError, Value};

struct KvStore {
    path: String,
    data: serde_json::Map<String, serde_json::Value>,
}

thread_local! {
    static KV_STORES: RefCell<HashMap<String, KvStore>> = RefCell::new(HashMap::new());
}

pub fn register(env: &sema_core::Env, sandbox: &sema_core::Sandbox) {
    crate::register_fn_gated(env, sandbox, sema_core::Caps::FS_WRITE, "kv/open", |args| {
        if args.len() != 2 { return Err(SemaError::arity("kv/open", "2", args.len())); }
        let name = args[0].as_str().ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let path = args[1].as_str().ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        let data = if std::path::Path::new(path).exists() {
            let content = std::fs::read_to_string(path)
                .map_err(|e| SemaError::Io(format!("kv/open: {e}")))?;
            serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&content)
                .unwrap_or_default()
        } else {
            serde_json::Map::new()
        };
        KV_STORES.with(|s| s.borrow_mut().insert(name.to_string(), KvStore { path: path.to_string(), data }));
        Ok(Value::string(name))
    });

    crate::register_fn(env, "kv/get", |args| {
        if args.len() != 2 { return Err(SemaError::arity("kv/get", "2", args.len())); }
        let name = args[0].as_str().ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let key = args[1].as_str().ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        KV_STORES.with(|s| {
            let s = s.borrow();
            let store = s.get(name).ok_or_else(|| SemaError::eval(format!("kv store '{}' not open", name)))?;
            match store.data.get(key) {
                Some(v) => Ok(json_val_to_sema(v)),
                None => Ok(Value::nil()),
            }
        })
    });

    crate::register_fn_gated(env, sandbox, sema_core::Caps::FS_WRITE, "kv/set", |args| {
        if args.len() != 3 { return Err(SemaError::arity("kv/set", "3", args.len())); }
        let name = args[0].as_str().ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let key = args[1].as_str().ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        let val = sema_to_json_val(&args[2]);
        KV_STORES.with(|s| {
            let mut s = s.borrow_mut();
            let store = s.get_mut(name).ok_or_else(|| SemaError::eval(format!("kv store '{}' not open", name)))?;
            store.data.insert(key.to_string(), val);
            flush_store(store)
        })?;
        Ok(args[2].clone())
    });

    crate::register_fn_gated(env, sandbox, sema_core::Caps::FS_WRITE, "kv/delete", |args| {
        if args.len() != 2 { return Err(SemaError::arity("kv/delete", "2", args.len())); }
        let name = args[0].as_str().ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let key = args[1].as_str().ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        KV_STORES.with(|s| {
            let mut s = s.borrow_mut();
            let store = s.get_mut(name).ok_or_else(|| SemaError::eval(format!("kv store '{}' not open", name)))?;
            let existed = store.data.remove(key).is_some();
            flush_store(store)?;
            Ok(Value::bool(existed))
        })
    });

    crate::register_fn(env, "kv/keys", |args| {
        if args.len() != 1 { return Err(SemaError::arity("kv/keys", "1", args.len())); }
        let name = args[0].as_str().ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        KV_STORES.with(|s| {
            let s = s.borrow();
            let store = s.get(name).ok_or_else(|| SemaError::eval(format!("kv store '{}' not open", name)))?;
            Ok(Value::list(store.data.keys().map(|k| Value::string(k)).collect()))
        })
    });

    crate::register_fn(env, "kv/close", |args| {
        if args.len() != 1 { return Err(SemaError::arity("kv/close", "1", args.len())); }
        let name = args[0].as_str().ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        KV_STORES.with(|s| {
            let mut s = s.borrow_mut();
            if let Some(store) = s.get(name) {
                let _ = flush_store_ref(store);
            }
            s.remove(name);
        });
        Ok(Value::nil())
    });
}

fn flush_store(store: &KvStore) -> Result<(), SemaError> {
    let json = serde_json::to_string_pretty(&store.data)
        .map_err(|e| SemaError::Io(format!("kv/flush: {e}")))?;
    std::fs::write(&store.path, json)
        .map_err(|e| SemaError::Io(format!("kv/flush: {e}")))?;
    Ok(())
}

fn flush_store_ref(store: &KvStore) -> Result<(), SemaError> {
    flush_store(store)
}

fn sema_to_json_val(val: &Value) -> serde_json::Value {
    if val.is_nil() { return serde_json::Value::Null; }
    if let Some(b) = val.as_bool() { return serde_json::Value::Bool(b); }
    if let Some(i) = val.as_int() { return serde_json::Value::Number(i.into()); }
    if let Some(f) = val.as_float() {
        return serde_json::Number::from_f64(f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null);
    }
    if let Some(s) = val.as_str() { return serde_json::Value::String(s.to_string()); }
    if let Some(l) = val.as_list() {
        return serde_json::Value::Array(l.iter().map(sema_to_json_val).collect());
    }
    if let Some(m) = val.as_map_rc() {
        let mut obj = serde_json::Map::new();
        for (k, v) in m.iter() {
            let key = k.as_keyword().or_else(|| k.as_str().map(|s| s.to_string()))
                .unwrap_or_else(|| k.to_string());
            obj.insert(key, sema_to_json_val(v));
        }
        return serde_json::Value::Object(obj);
    }
    serde_json::Value::String(val.to_string())
}

fn json_val_to_sema(json: &serde_json::Value) -> Value {
    match json {
        serde_json::Value::Null => Value::nil(),
        serde_json::Value::Bool(b) => Value::bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() { Value::int(i) }
            else if let Some(f) = n.as_f64() { Value::float(f) }
            else { Value::nil() }
        }
        serde_json::Value::String(s) => Value::string(s),
        serde_json::Value::Array(a) => Value::list(a.iter().map(json_val_to_sema).collect()),
        serde_json::Value::Object(o) => {
            let mut map = std::collections::BTreeMap::new();
            for (k, v) in o { map.insert(Value::keyword(k), json_val_to_sema(v)); }
            Value::map(map)
        }
    }
}
```

### Step 3: Register kv module

In `crates/sema-stdlib/src/lib.rs`:

- Add `mod kv;` (gated: `#[cfg(not(target_arch = "wasm32"))]`)
- Add `kv::register(env, sandbox);` in `register_stdlib` (gated same way)

### Step 4: Run tests, commit

**Run:** `cargo test -p sema --test integration_test -- test_kv`
**Expected:** PASS

```bash
git add crates/sema-stdlib/src/kv.rs crates/sema-stdlib/src/lib.rs crates/sema/tests/integration_test.rs
git commit -m "feat(stdlib): add persistent JSON-backed key-value store"
```

---

## Task 15: Document Metadata Tracking

**Files:**

- Modify: `crates/sema-stdlib/src/text.rs`
- Test: `crates/sema/tests/integration_test.rs`

### Overview

**Convention-based approach** — no new Value type needed. A "document" is just a Sema map:
`{:text "..." :metadata {:source "file.pdf" :page 3}}`. The `document/chunk` function
chunks the `:text` and propagates `:metadata` to each chunk.

Depends on Task 4 (text chunking).

### Step 1: Write failing tests

```rust
#[test]
fn test_document_create() {
    let result = eval(r#"(document/create "hello world" {:source "test.txt"})"#);
    let map = result.as_map_rc().unwrap();
    assert_eq!(map.get(&Value::keyword("text")).unwrap().as_str().unwrap(), "hello world");
    let meta = map.get(&Value::keyword("metadata")).unwrap().as_map_rc().unwrap();
    assert_eq!(meta.get(&Value::keyword("source")).unwrap().as_str().unwrap(), "test.txt");
}

#[test]
fn test_document_chunk_preserves_metadata() {
    let interp = Interpreter::new();
    let result = interp.eval_str(r#"
        (document/chunk
            (document/create "aaaa bbbb cccc dddd" {:source "test.txt" :page 1})
            {:size 10})
    "#).unwrap();
    let chunks = result.as_list().unwrap();
    assert!(chunks.len() >= 2);
    for chunk in chunks {
        let map = chunk.as_map_rc().expect("chunk should be a map");
        assert!(map.get(&Value::keyword("text")).unwrap().as_str().is_some());
        let meta = map.get(&Value::keyword("metadata")).unwrap().as_map_rc().unwrap();
        assert_eq!(meta.get(&Value::keyword("source")).unwrap().as_str().unwrap(), "test.txt");
    }
}

#[test]
fn test_document_chunk_adds_chunk_index() {
    let interp = Interpreter::new();
    let result = interp.eval_str(r#"
        (document/chunk
            (document/create "aaaa bbbb cccc dddd" {:source "f.txt"})
            {:size 10})
    "#).unwrap();
    let chunks = result.as_list().unwrap();
    for (i, chunk) in chunks.iter().enumerate() {
        let meta = chunk.as_map_rc().unwrap()
            .get(&Value::keyword("metadata")).unwrap().as_map_rc().unwrap();
        assert_eq!(meta.get(&Value::keyword("chunk-index")).unwrap().as_int().unwrap(), i as i64);
    }
}

#[test]
fn test_document_text() {
    let result = eval(r#"(document/text (document/create "hello" {:source "x"}))"#);
    assert_eq!(result, Value::string("hello"));
}

#[test]
fn test_document_metadata() {
    let result = eval(r#"(document/metadata (document/create "hello" {:source "x"}))"#);
    let meta = result.as_map_rc().unwrap();
    assert_eq!(meta.get(&Value::keyword("source")).unwrap().as_str().unwrap(), "x");
}
```

**Run:** `cargo test -p sema --test integration_test -- test_document`
**Expected:** FAIL

### Step 2: Implement

Add to `register()` in `text.rs`:

```rust
    // (document/create text metadata-map)
    register_fn(env, "document/create", |args| {
        if args.len() != 2 { return Err(SemaError::arity("document/create", "2", args.len())); }
        let text = args[0].as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let metadata = args[1].as_map_rc()
            .ok_or_else(|| SemaError::type_error("map", args[1].type_name()))?;
        let mut doc = std::collections::BTreeMap::new();
        doc.insert(Value::keyword("text"), Value::string(text));
        doc.insert(Value::keyword("metadata"), Value::map((*metadata).clone()));
        Ok(Value::map(doc))
    });

    // (document/text doc)
    register_fn(env, "document/text", |args| {
        if args.len() != 1 { return Err(SemaError::arity("document/text", "1", args.len())); }
        let map = args[0].as_map_rc()
            .ok_or_else(|| SemaError::type_error("map (document)", args[0].type_name()))?;
        map.get(&Value::keyword("text")).cloned().ok_or_else(|| SemaError::eval("not a document: missing :text"))
    });

    // (document/metadata doc)
    register_fn(env, "document/metadata", |args| {
        if args.len() != 1 { return Err(SemaError::arity("document/metadata", "1", args.len())); }
        let map = args[0].as_map_rc()
            .ok_or_else(|| SemaError::type_error("map (document)", args[0].type_name()))?;
        map.get(&Value::keyword("metadata")).cloned().ok_or_else(|| SemaError::eval("not a document: missing :metadata"))
    });

    // (document/chunk doc {:size 1000 :overlap 200})
    register_fn(env, "document/chunk", |args| {
        if args.is_empty() || args.len() > 2 {
            return Err(SemaError::arity("document/chunk", "1-2", args.len()));
        }
        let doc = args[0].as_map_rc()
            .ok_or_else(|| SemaError::type_error("map (document)", args[0].type_name()))?;
        let text = doc.get(&Value::keyword("text"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| SemaError::eval("document/chunk: document missing :text"))?;
        let base_metadata = doc.get(&Value::keyword("metadata"))
            .and_then(|v| v.as_map_rc())
            .map(|m| (*m).clone())
            .unwrap_or_default();

        let mut chunk_size: usize = 1000;
        let mut overlap: usize = 200;
        if let Some(opts) = args.get(1).and_then(|v| v.as_map_rc()) {
            if let Some(v) = opts.get(&Value::keyword("size")).and_then(|v| v.as_int()) {
                chunk_size = v.max(1) as usize;
            }
            if let Some(v) = opts.get(&Value::keyword("overlap")).and_then(|v| v.as_int()) {
                overlap = v.max(0) as usize;
            }
        }
        if overlap >= chunk_size { overlap = 0; }

        let chunks = recursive_chunk(text, chunk_size, overlap);
        let result: Vec<Value> = chunks.into_iter().enumerate().map(|(i, chunk_text)| {
            let mut meta = base_metadata.clone();
            meta.insert(Value::keyword("chunk-index"), Value::int(i as i64));
            meta.insert(Value::keyword("total-chunks"), Value::int(0)); // placeholder
            let mut doc_map = std::collections::BTreeMap::new();
            doc_map.insert(Value::keyword("text"), Value::string(&chunk_text));
            doc_map.insert(Value::keyword("metadata"), Value::map(meta));
            Value::map(doc_map)
        }).collect();

        // Fix total-chunks count
        let total = result.len() as i64;
        let result: Vec<Value> = result.into_iter().map(|chunk| {
            if let Some(map) = chunk.as_map_rc() {
                let mut m = (*map).clone();
                if let Some(meta_val) = m.get(&Value::keyword("metadata")) {
                    if let Some(meta) = meta_val.as_map_rc() {
                        let mut meta = (*meta).clone();
                        meta.insert(Value::keyword("total-chunks"), Value::int(total));
                        m.insert(Value::keyword("metadata"), Value::map(meta));
                    }
                }
                Value::map(m)
            } else { chunk }
        }).collect();

        Ok(Value::list(result))
    });
```

### Step 3: Run tests, commit

**Run:** `cargo test -p sema --test integration_test -- test_document`
**Expected:** PASS

```bash
git add crates/sema-stdlib/src/text.rs crates/sema/tests/integration_test.rs
git commit -m "feat(stdlib): add document type with metadata-preserving chunking"
```
