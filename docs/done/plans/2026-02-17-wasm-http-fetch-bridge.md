# WASM HTTP via Fetch Bridge — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Enable `http/get`, `http/post`, `http/put`, `http/delete`, `http/request` in the WASM playground using the browser's `fetch()` API.

**Architecture:** Replay-with-cache strategy. WASM HTTP native fns check an in-memory response cache — on cache miss they raise a marker error encoding the request. A new `eval_async` entrypoint (exposed as a JS Promise via `wasm-bindgen-futures`) catches the marker, performs `fetch()` via `web-sys`, caches the response, and replays evaluation. This keeps the Sema evaluator 100% synchronous — all async logic lives at the WASM boundary.

**Tech Stack:** `wasm-bindgen-futures`, `web-sys` (fetch features), existing `sema-wasm` crate, playground JS.

---

## Task 1: Add `wasm-bindgen-futures` and `web-sys` dependencies

**Files:**

- Modify: `crates/sema-wasm/Cargo.toml`

**Step 1: Add dependencies**

Add to `[dependencies]` in `crates/sema-wasm/Cargo.toml`:

```toml
wasm-bindgen-futures = "0.4"
web-sys = { version = "0.3", features = [
    "Window",
    "Request",
    "RequestInit",
    "RequestMode",
    "Headers",
    "Response",
    "AbortController",
    "AbortSignal",
] }
```

**Step 2: Verify it compiles**

Run: `cargo check -p sema-wasm --target wasm32-unknown-unknown`
Expected: compiles without errors

**Step 3: Commit**

```bash
git add crates/sema-wasm/Cargo.toml
git commit -m "feat(wasm): add wasm-bindgen-futures and web-sys deps for HTTP fetch"
```

---

## Task 2: HTTP request cache and marker error infrastructure

**Files:**

- Modify: `crates/sema-wasm/src/lib.rs`

This task adds the cache, marker format, and helper functions. No HTTP fns are changed yet.

**Step 1: Add thread-local HTTP cache and marker constants**

Add a new thread-local after the existing `VFS_DIRS` declaration (around line 23):

```rust
/// HTTP response cache for async replay strategy
static HTTP_CACHE: RefCell<BTreeMap<String, Value>> = const { RefCell::new(BTreeMap::new()) };
```

Add a constant for the marker prefix (after the thread_local block):

```rust
/// Prefix for HTTP await marker errors — eval_async detects this to perform fetch
const HTTP_AWAIT_MARKER: &str = "__SEMA_WASM_HTTP__";
```

**Step 2: Add helper functions for cache key computation and marker creation**

Add these functions after the existing `take_output()` function:

```rust
/// Build a deterministic cache key from HTTP request parameters.
fn http_cache_key(method: &str, url: &str, body: Option<&str>, headers: &[(String, String)]) -> String {
    let mut key = format!("{method}\n{url}\n");
    if let Some(b) = body {
        key.push_str(b);
    }
    key.push('\n');
    for (k, v) in headers {
        key.push_str(k);
        key.push(':');
        key.push_str(v);
        key.push('\n');
    }
    key
}

/// Create an HTTP await marker error containing the serialized request.
fn http_await_marker(
    method: &str,
    url: &str,
    body: Option<&str>,
    headers: &[(String, String)],
    timeout_ms: Option<u64>,
) -> SemaError {
    let headers_json: Vec<String> = headers
        .iter()
        .map(|(k, v)| format!("[\"{}\",\"{}\"]", escape_json(k), escape_json(v)))
        .collect();
    let body_json = match body {
        Some(b) => format!("\"{}\"", escape_json(b)),
        None => "null".to_string(),
    };
    let timeout_json = match timeout_ms {
        Some(ms) => ms.to_string(),
        None => "null".to_string(),
    };
    let key = http_cache_key(method, url, body, headers);
    SemaError::eval(format!(
        "{HTTP_AWAIT_MARKER}{{\"key\":\"{}\",\"method\":\"{method}\",\"url\":\"{}\",\"body\":{body_json},\"headers\":[{headers_list}],\"timeout\":{timeout_json}}}",
        escape_json(&key),
        escape_json(url),
        headers_list = headers_json.join(","),
    ))
}

/// Check if an error is an HTTP await marker.
fn is_http_await_marker(err: &SemaError) -> bool {
    if let SemaError::Eval { message, .. } = err {
        message.starts_with(HTTP_AWAIT_MARKER)
    } else {
        false
    }
}

/// Extract the JSON payload from an HTTP await marker error.
fn parse_http_marker(err: &SemaError) -> Option<String> {
    if let SemaError::Eval { message, .. } = err {
        message.strip_prefix(HTTP_AWAIT_MARKER).map(|s| s.to_string())
    } else {
        None
    }
}

/// Clear the HTTP response cache (called between interpreter sessions if needed).
fn clear_http_cache() {
    HTTP_CACHE.with(|c| c.borrow_mut().clear());
}
```

**Step 3: Verify it compiles**

Run: `cargo check -p sema-wasm --target wasm32-unknown-unknown`
Expected: compiles (some dead-code warnings OK at this stage)

**Step 4: Commit**

```bash
git add crates/sema-wasm/src/lib.rs
git commit -m "feat(wasm): add HTTP cache and marker infrastructure for fetch bridge"
```

---

## Task 3: Replace HTTP stubs with cache-aware native functions

**Files:**

- Modify: `crates/sema-wasm/src/lib.rs`

Replace the 5 HTTP stub registrations (lines ~507-557) with cache-aware versions.

**Step 1: Add a shared helper that all HTTP fns delegate to**

Add this function before `register_wasm_io`:

```rust
/// Shared HTTP handler for WASM: checks cache, returns marker on miss.
/// Parses args the same way as native stdlib http.rs.
fn wasm_http_request(
    method: &str,
    url: &str,
    body: Option<&Value>,
    opts: Option<&Value>,
) -> Result<Value, SemaError> {
    // Parse headers and timeout from opts
    let mut headers: Vec<(String, String)> = Vec::new();
    let mut timeout_ms: Option<u64> = None;
    if let Some(opts_val) = opts {
        if let Some(opts_map) = opts_val.as_map_rc() {
            if let Some(headers_val) = opts_map.get(&Value::keyword("headers")) {
                if let Some(hmap) = headers_val.as_map_rc() {
                    for (k, v) in hmap.iter() {
                        let key = if let Some(s) = k.as_str() {
                            s.to_string()
                        } else if let Some(spur) = k.as_keyword_spur() {
                            sema_core::resolve(spur)
                        } else {
                            k.to_string()
                        };
                        let val = match v.as_str() {
                            Some(s) => s.to_string(),
                            None => v.to_string(),
                        };
                        headers.push((key, val));
                    }
                }
            }
            if let Some(timeout_val) = opts_map.get(&Value::keyword("timeout")) {
                if let Some(ms) = timeout_val.as_int() {
                    timeout_ms = Some(ms as u64);
                }
            }
        }
    }

    // Serialize body to string
    let body_str: Option<String> = body.map(|b| {
        if let Some(s) = b.as_str() {
            s.to_string()
        } else if b.as_map_rc().is_some() {
            // JSON-encode map bodies, add Content-Type header
            match serde_json::to_string(&value_to_json_for_body(b)) {
                Ok(json) => json,
                Err(_) => b.to_string(),
            }
        } else {
            b.to_string()
        }
    });

    // Add Content-Type for map bodies if not already set
    if let Some(b) = body {
        if b.as_map_rc().is_some()
            && !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-type"))
        {
            headers.push(("Content-Type".to_string(), "application/json".to_string()));
        }
    }

    // Sort headers for deterministic cache key
    headers.sort_by(|a, b| a.0.cmp(&b.0));

    let key = http_cache_key(method, url, body_str.as_deref(), &headers);

    // Check cache
    let cached = HTTP_CACHE.with(|c| c.borrow().get(&key).cloned());
    if let Some(response) = cached {
        return Ok(response);
    }

    // Cache miss — return marker for eval_async to handle
    Err(http_await_marker(method, url, body_str.as_deref(), &headers, timeout_ms))
}

/// Convert a Value to serde_json::Value for HTTP body serialization.
/// Simplified version — only needs to handle map/list/string/number/bool/nil.
fn value_to_json_for_body(val: &Value) -> serde_json::Value {
    use sema_core::ValueView;
    match val.view() {
        ValueView::Nil => serde_json::Value::Null,
        ValueView::Bool(b) => serde_json::Value::Bool(b),
        ValueView::Int(n) => serde_json::Value::Number(n.into()),
        ValueView::Float(f) => serde_json::Number::from_f64(f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        ValueView::String(s) => serde_json::Value::String(s.to_string()),
        ValueView::Keyword(s) => serde_json::Value::String(sema_core::resolve(s)),
        ValueView::List(items) => {
            serde_json::Value::Array(items.iter().map(value_to_json_for_body).collect())
        }
        ValueView::Map(map) => {
            let mut obj = serde_json::Map::new();
            for (k, v) in map.iter() {
                let key = if let Some(s) = k.as_str() {
                    s.to_string()
                } else if let Some(spur) = k.as_keyword_spur() {
                    sema_core::resolve(spur)
                } else {
                    k.to_string()
                };
                obj.insert(key, value_to_json_for_body(v));
            }
            serde_json::Value::Object(obj)
        }
        _ => serde_json::Value::String(val.to_string()),
    }
}
```

**Step 2: Replace the 5 HTTP stub registrations**

Replace the entire `// --- HTTP stubs` block (lines ~507-557) with:

```rust
    // --- HTTP via fetch bridge (cache-aware, used with eval_async) ---

    register(
        "http/get",
        Box::new(|args: &[Value]| {
            if args.is_empty() || args.len() > 2 {
                return Err(SemaError::arity("http/get", "1 or 2", args.len()));
            }
            let url = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
            let opts = args.get(1);
            wasm_http_request("GET", url, None, opts)
        }),
    );

    register(
        "http/post",
        Box::new(|args: &[Value]| {
            if args.len() < 2 || args.len() > 3 {
                return Err(SemaError::arity("http/post", "2 or 3", args.len()));
            }
            let url = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
            let body = &args[1];
            let opts = args.get(2);
            wasm_http_request("POST", url, Some(body), opts)
        }),
    );

    register(
        "http/put",
        Box::new(|args: &[Value]| {
            if args.len() < 2 || args.len() > 3 {
                return Err(SemaError::arity("http/put", "2 or 3", args.len()));
            }
            let url = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
            let body = &args[1];
            let opts = args.get(2);
            wasm_http_request("PUT", url, Some(body), opts)
        }),
    );

    register(
        "http/delete",
        Box::new(|args: &[Value]| {
            if args.is_empty() || args.len() > 2 {
                return Err(SemaError::arity("http/delete", "1 or 2", args.len()));
            }
            let url = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
            let opts = args.get(1);
            wasm_http_request("DELETE", url, None, opts)
        }),
    );

    register(
        "http/request",
        Box::new(|args: &[Value]| {
            if args.len() < 2 || args.len() > 4 {
                return Err(SemaError::arity("http/request", "2-4", args.len()));
            }
            let method = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?
                .to_uppercase();
            let url = args[1]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
            let opts = args.get(2);
            let body = args.get(3);
            wasm_http_request(&method, url, body, opts)
        }),
    );
```

**Step 3: Verify it compiles**

Run: `cargo check -p sema-wasm --target wasm32-unknown-unknown`
Expected: compiles without errors

**Step 4: Commit**

```bash
git add crates/sema-wasm/src/lib.rs
git commit -m "feat(wasm): replace HTTP stubs with cache-aware native fns"
```

---

## Task 4: Implement `eval_async` with fetch and replay loop

**Files:**

- Modify: `crates/sema-wasm/src/lib.rs`

This is the core task. Add async evaluation methods that catch HTTP markers, perform `fetch()`, cache results, and replay.

**Step 1: Add the fetch helper function**

Add this async function (can go near the other helper functions):

```rust
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

/// Perform an HTTP fetch and return the response as a Sema Value map {:status :headers :body}.
async fn perform_fetch(
    method: &str,
    url: &str,
    body: Option<&str>,
    headers: &[(String, String)],
    timeout_ms: Option<u64>,
) -> Result<Value, SemaError> {
    let window = web_sys::window()
        .ok_or_else(|| SemaError::eval("http: no window object available"))?;

    let mut opts = web_sys::RequestInit::new();
    opts.method(method);
    opts.mode(web_sys::RequestMode::Cors);

    // Set body for methods that support it
    if let Some(body_str) = body {
        opts.body(Some(&JsValue::from_str(body_str)));
    }

    // Set up abort controller for timeout
    let abort_controller = if timeout_ms.is_some() {
        let controller = web_sys::AbortController::new()
            .map_err(|e| SemaError::eval(format!("http: abort controller: {e:?}")))?;
        opts.signal(Some(&controller.signal()));
        Some(controller)
    } else {
        None
    };

    // Build request with headers
    let request = web_sys::Request::new_with_str_and_init(url, &opts)
        .map_err(|e| SemaError::eval(format!("http: request creation failed: {e:?}")))?;

    let req_headers = request.headers();
    for (k, v) in headers {
        req_headers
            .set(k, v)
            .map_err(|e| SemaError::eval(format!("http: set header {k}: {e:?}")))?;
    }

    // Set timeout if specified
    if let Some(ms) = timeout_ms {
        let controller = abort_controller.as_ref().unwrap().clone();
        let closure = wasm_bindgen::closure::Closure::once(move || {
            controller.abort();
        });
        window
            .set_timeout_with_callback_and_timeout_and_arguments_0(
                closure.as_ref().unchecked_ref(),
                ms as i32,
            )
            .map_err(|e| SemaError::eval(format!("http: set timeout: {e:?}")))?;
        closure.forget(); // prevent drop
    }

    // Perform fetch
    let resp_jsvalue = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|e| {
            let msg = js_sys::Object::try_from(&e)
                .and_then(|obj| js_sys::Reflect::get(obj, &JsValue::from_str("message")).ok())
                .and_then(|v| v.as_string())
                .unwrap_or_else(|| format!("{e:?}"));
            SemaError::Io(format!("http {method} {url}: {msg}"))
        })?;

    let response: web_sys::Response = resp_jsvalue
        .dyn_into()
        .map_err(|_| SemaError::eval("http: response is not a Response object"))?;

    let status = response.status() as i64;

    // Read response headers
    let mut resp_headers = BTreeMap::new();
    // Response.headers() returns a Headers object; iterate via js_sys
    let headers_obj = response.headers();
    if let Ok(entries) = js_sys::try_iter(&headers_obj) {
        if let Some(iter) = entries {
            for entry in iter {
                if let Ok(entry) = entry {
                    let pair = js_sys::Array::from(&entry);
                    if pair.length() == 2 {
                        if let (Some(k), Some(v)) = (pair.get(0).as_string(), pair.get(1).as_string()) {
                            resp_headers.insert(Value::keyword(&k), Value::string(&v));
                        }
                    }
                }
            }
        }
    }

    // Read body text
    let body_promise = response
        .text()
        .map_err(|e| SemaError::eval(format!("http: reading body: {e:?}")))?;
    let body_jsvalue = JsFuture::from(body_promise)
        .await
        .map_err(|e| SemaError::Io(format!("http {method} {url}: read body: {e:?}")))?;
    let body_text = body_jsvalue.as_string().unwrap_or_default();

    // Build result map matching native format: {:status :headers :body}
    let mut result = BTreeMap::new();
    result.insert(Value::keyword("status"), Value::int(status));
    result.insert(Value::keyword("headers"), Value::map(resp_headers));
    result.insert(Value::keyword("body"), Value::string(&body_text));
    Ok(Value::map(result))
}
```

**Step 2: Add `eval_async` and `eval_vm_async` methods on `WasmInterpreter`**

Add these methods inside the `#[wasm_bindgen] impl WasmInterpreter` block, after the existing `eval_vm` method:

```rust
    /// Async evaluation (tree-walker) — supports HTTP via fetch.
    /// Returns same JSON format as eval_global.
    /// Called from JS as: `await interp.eval_async(code)`
    pub async fn eval_async(&self, code: &str) -> String {
        const MAX_REPLAYS: usize = 50;
        clear_http_cache();

        for _attempt in 0..MAX_REPLAYS {
            OUTPUT.with(|o| o.borrow_mut().clear());
            LINE_BUF.with(|b| b.borrow_mut().clear());

            match sema_eval::eval_string(&self.inner.ctx, code, &self.inner.global_env) {
                Ok(val) => {
                    let output = take_output();
                    let val_str = if val.is_nil() {
                        "null".to_string()
                    } else {
                        format!("\"{}\"", escape_json(&format!("{val}")))
                    };
                    return format!(
                        "{{\"value\":{},\"output\":[{}],\"error\":null}}",
                        val_str,
                        output.iter().map(|s| format!("\"{}\"", escape_json(s))).collect::<Vec<_>>().join(",")
                    );
                }
                Err(e) => {
                    if is_http_await_marker(e.inner()) {
                        if let Some(json_str) = parse_http_marker(e.inner()) {
                            match perform_fetch_from_marker(&json_str).await {
                                Ok((key, value)) => {
                                    HTTP_CACHE.with(|c| c.borrow_mut().insert(key, value));
                                    continue; // replay
                                }
                                Err(fetch_err) => {
                                    let output = take_output();
                                    return format!(
                                        "{{\"value\":null,\"output\":[{}],\"error\":\"{}\"}}",
                                        output.iter().map(|s| format!("\"{}\"", escape_json(s))).collect::<Vec<_>>().join(","),
                                        escape_json(&format!("{fetch_err}"))
                                    );
                                }
                            }
                        }
                    }
                    // Real error — not an HTTP marker
                    let output = take_output();
                    let mut err_str = format!("{}", e.inner());
                    if let Some(trace) = e.stack_trace() {
                        err_str.push_str(&format!("\n{trace}"));
                    }
                    if let Some(hint) = e.hint() {
                        err_str.push_str(&format!("\n  hint: {hint}"));
                    }
                    if let Some(note) = e.note() {
                        err_str.push_str(&format!("\n  note: {note}"));
                    }
                    return format!(
                        "{{\"value\":null,\"output\":[{}],\"error\":\"{}\"}}",
                        output.iter().map(|s| format!("\"{}\"", escape_json(s))).collect::<Vec<_>>().join(","),
                        escape_json(&err_str)
                    );
                }
            }
        }

        // Exceeded max replays
        let output = take_output();
        format!(
            "{{\"value\":null,\"output\":[{}],\"error\":\"{}\"}}",
            output.iter().map(|s| format!("\"{}\"", escape_json(s))).collect::<Vec<_>>().join(","),
            escape_json("http: exceeded maximum number of HTTP requests (50) in a single evaluation")
        )
    }

    /// Async evaluation (bytecode VM) — supports HTTP via fetch.
    pub async fn eval_vm_async(&self, code: &str) -> String {
        const MAX_REPLAYS: usize = 50;
        clear_http_cache();

        for _attempt in 0..MAX_REPLAYS {
            OUTPUT.with(|o| o.borrow_mut().clear());
            LINE_BUF.with(|b| b.borrow_mut().clear());

            match self.inner.eval_str_compiled(code) {
                Ok(val) => {
                    let output = take_output();
                    let val_str = if val.is_nil() {
                        "null".to_string()
                    } else {
                        format!("\"{}\"", escape_json(&format!("{val}")))
                    };
                    return format!(
                        "{{\"value\":{},\"output\":[{}],\"error\":null}}",
                        val_str,
                        output.iter().map(|s| format!("\"{}\"", escape_json(s))).collect::<Vec<_>>().join(",")
                    );
                }
                Err(e) => {
                    if is_http_await_marker(e.inner()) {
                        if let Some(json_str) = parse_http_marker(e.inner()) {
                            match perform_fetch_from_marker(&json_str).await {
                                Ok((key, value)) => {
                                    HTTP_CACHE.with(|c| c.borrow_mut().insert(key, value));
                                    continue;
                                }
                                Err(fetch_err) => {
                                    let output = take_output();
                                    return format!(
                                        "{{\"value\":null,\"output\":[{}],\"error\":\"{}\"}}",
                                        output.iter().map(|s| format!("\"{}\"", escape_json(s))).collect::<Vec<_>>().join(","),
                                        escape_json(&format!("{fetch_err}"))
                                    );
                                }
                            }
                        }
                    }
                    let output = take_output();
                    let mut err_str = format!("{}", e.inner());
                    if let Some(trace) = e.stack_trace() {
                        err_str.push_str(&format!("\n{trace}"));
                    }
                    if let Some(hint) = e.hint() {
                        err_str.push_str(&format!("\n  hint: {hint}"));
                    }
                    if let Some(note) = e.note() {
                        err_str.push_str(&format!("\n  note: {note}"));
                    }
                    return format!(
                        "{{\"value\":null,\"output\":[{}],\"error\":\"{}\"}}",
                        output.iter().map(|s| format!("\"{}\"", escape_json(s))).collect::<Vec<_>>().join(","),
                        escape_json(&err_str)
                    );
                }
            }
        }

        let output = take_output();
        format!(
            "{{\"value\":null,\"output\":[{}],\"error\":\"{}\"}}",
            output.iter().map(|s| format!("\"{}\"", escape_json(s))).collect::<Vec<_>>().join(","),
            escape_json("http: exceeded maximum number of HTTP requests (50) in a single evaluation")
        )
    }
```

**Step 3: Add `perform_fetch_from_marker` helper**

This parses the marker JSON and calls `perform_fetch`:

```rust
/// Parse an HTTP marker JSON string and perform the actual fetch.
/// Returns (cache_key, response_value).
async fn perform_fetch_from_marker(json_str: &str) -> Result<(String, Value), SemaError> {
    let parsed: serde_json::Value = serde_json::from_str(json_str)
        .map_err(|e| SemaError::eval(format!("http: invalid marker JSON: {e}")))?;

    let key = parsed["key"]
        .as_str()
        .ok_or_else(|| SemaError::eval("http: marker missing key"))?
        .to_string();
    let method = parsed["method"]
        .as_str()
        .ok_or_else(|| SemaError::eval("http: marker missing method"))?;
    let url = parsed["url"]
        .as_str()
        .ok_or_else(|| SemaError::eval("http: marker missing url"))?;
    let body = parsed["body"].as_str();
    let timeout_ms = parsed["timeout"].as_u64();

    let mut headers: Vec<(String, String)> = Vec::new();
    if let Some(arr) = parsed["headers"].as_array() {
        for pair in arr {
            if let Some(pair_arr) = pair.as_array() {
                if pair_arr.len() == 2 {
                    if let (Some(k), Some(v)) = (pair_arr[0].as_str(), pair_arr[1].as_str()) {
                        headers.push((k.to_string(), v.to_string()));
                    }
                }
            }
        }
    }

    let response = perform_fetch(method, url, body, &headers, timeout_ms).await?;
    Ok((key, response))
}
```

**Step 4: Verify it compiles**

Run: `cargo check -p sema-wasm --target wasm32-unknown-unknown`
Expected: compiles without errors

**Step 5: Commit**

```bash
git add crates/sema-wasm/src/lib.rs
git commit -m "feat(wasm): implement eval_async with fetch bridge and replay loop"
```

---

## Task 5: Update playground JS to use async evaluation

**Files:**

- Modify: `playground/src/app.js`

**Step 1: Make `run()` async and use `eval_async`/`eval_vm_async`**

Replace the `run()` function with:

```javascript
async function run() {
  if (!interp) return;
  const code = document.getElementById("editor").value;
  if (!code.trim()) return;

  const engine = useVM ? "vm" : "tree";
  const runBtn = document.getElementById("run-btn");
  runBtn.disabled = true;

  const t0 = performance.now();
  const raw = useVM
    ? await interp.eval_vm_async(code)
    : await interp.eval_async(code);
  const elapsed = performance.now() - t0;

  runBtn.disabled = false;

  let result;
  try {
    result = JSON.parse(raw);
  } catch {
    result = { value: null, output: [], error: raw };
  }

  const out = document.getElementById("output");
  out.innerHTML = "";

  // Print output lines
  if (result.output && result.output.length > 0) {
    for (const line of result.output) {
      const div = document.createElement("div");
      div.className = "output-line";
      div.textContent = line;
      out.appendChild(div);
    }
  }

  // Print result or error
  if (result.error) {
    const div = document.createElement("div");
    div.className = "output-error";
    div.textContent = result.error;
    out.appendChild(div);
  } else if (result.value !== null) {
    const div = document.createElement("div");
    div.className = "output-value";
    div.textContent = `=> ${result.value}`;
    out.appendChild(div);
  }

  // Timing
  const timing = document.createElement("div");
  timing.className = "output-timing";
  timing.textContent = `Evaluated in ${elapsed.toFixed(1)}ms · ${engine === "vm" ? "bytecode VM" : "tree-walker"}`;
  out.appendChild(timing);
}
```

Key changes:

- `function run()` → `async function run()`
- `interp.eval_global(code)` → `await interp.eval_async(code)`
- `interp.eval_vm(code)` → `await interp.eval_vm_async(code)`
- Disable run button during async eval, re-enable after

**Step 2: Verify the keyboard shortcut still works**

The `keydown` handler calls `run()` — since `run()` is now async, calling it without `await` is fine (it returns a Promise that's fire-and-forget). No change needed.

**Step 3: Commit**

```bash
git add playground/src/app.js
git commit -m "feat(playground): use async eval for HTTP fetch support"
```

---

## Task 6: Build the WASM crate and smoke test locally

**Step 1: Build the WASM crate**

Run: `cd playground && npm run build` (or however the playground builds the WASM pkg)

Check the playground build process:

```bash
ls playground/package.json
cat playground/package.json | grep -A5 '"scripts"'
```

Run the build script that compiles WASM and bundles the playground.

Expected: Builds successfully, no errors.

**Step 2: Smoke test locally**

Start the playground dev server and test in browser:

1. Open the playground
2. Enter: `(http/get "https://httpbin.org/get")`
3. Click Run
4. Expected: Returns a map with `:status 200`, `:headers {...}`, `:body "..."`

Also test:

- `(:status (http/get "https://httpbin.org/get"))` → `200`
- `(http/post "https://httpbin.org/post" {:name "sema"})` → map with status 200
- `(http/get "https://invalid.example.test")` → fetch error (not marker leak)
- VM mode: same tests with VM toggle enabled

**Step 3: Commit**

```bash
git commit -m "test: smoke test WASM HTTP fetch bridge"
```

---

## Task 7: Add Playwright tests for HTTP in playground

**Files:**

- Modify: `playground/tests/playground.spec.ts`

**Step 1: Add HTTP-specific tests**

Add these tests at the end of the test file:

```typescript
// ── HTTP fetch tests ──

test("http/get works in playground", async ({ page }) => {
  await setEditorCode(page, '(:status (http/get "https://httpbin.org/get"))');
  await clickRunAndWait(page);

  const value = await page.$eval(
    "#output .output-value",
    (el) => el.textContent,
  );
  expect(value).toContain("200");

  const errorEl = await page.$("#output .output-error");
  expect(errorEl).toBeNull();
});

test("http/get works with VM engine", async ({ page }) => {
  await setEditorCode(page, '(:status (http/get "https://httpbin.org/get"))');
  await selectVM(page);
  await clickRunAndWait(page);

  const value = await page.$eval(
    "#output .output-value",
    (el) => el.textContent,
  );
  expect(value).toContain("200");
});

test("http/post sends body", async ({ page }) => {
  await setEditorCode(
    page,
    '(:status (http/post "https://httpbin.org/post" "hello"))',
  );
  await clickRunAndWait(page);

  const value = await page.$eval(
    "#output .output-value",
    (el) => el.textContent,
  );
  expect(value).toContain("200");
});

test("http/get returns response with body", async ({ page }) => {
  await setEditorCode(
    page,
    '(string? (:body (http/get "https://httpbin.org/get")))',
  );
  await clickRunAndWait(page);

  const value = await page.$eval(
    "#output .output-value",
    (el) => el.textContent,
  );
  expect(value).toContain("#t");
});

test("http/get CORS error shows useful message", async ({ page }) => {
  // Most non-API sites block CORS
  await setEditorCode(page, '(http/get "https://example.com")');
  await clickRunAndWait(page);

  // Should show an error, not a marker leak
  const errorEl = await page.$("#output .output-error");
  if (errorEl) {
    const errorText = await errorEl.textContent();
    expect(errorText).not.toContain("__SEMA_WASM_HTTP__");
  }
});

test("run button disabled during async eval", async ({ page }) => {
  await setEditorCode(page, '(http/get "https://httpbin.org/delay/1")');

  const runBtn = page.getByTestId("run-btn");
  await runBtn.click();

  // Button should be disabled while fetching
  await expect(runBtn).toBeDisabled();

  // Wait for completion
  await page.waitForSelector("#output .output-timing", { timeout: 30000 });
  await expect(runBtn).toBeEnabled();
});
```

**Step 2: Run the tests**

Run: `cd playground && npx playwright test`
Expected: All tests pass (HTTP tests depend on network access to httpbin.org)

**Step 3: Commit**

```bash
git add playground/tests/playground.spec.ts
git commit -m "test(playground): add Playwright tests for HTTP fetch bridge"
```

---

## Task 8: Update playground docs

**Files:**

- Modify: `website/docs/stdlib/playground.md`

**Step 1: Move HTTP from "Not Available" to a working section**

Remove `http/get`, `http/post`, etc. from the "Not Available in WASM" table.

Replace the HTTP row and the "Future: HTTP Support" info box with:

In the "Not Available" table, remove the `http/get, http/post, ...` row entirely.

Remove the `::: info Future: HTTP Support` block entirely.

Add a new section after "Terminal Styling" and before "Not Available in WASM":

````markdown
### HTTP Functions

HTTP functions work in the playground via the browser's `fetch()` API. They return the same `{:status :headers :body}` map as the native CLI.

```scheme
(define resp (http/get "https://httpbin.org/get"))
(:status resp)    ; => 200
(:body resp)      ; => "{\"args\": {}, ...}"

(http/post "https://httpbin.org/post" {:name "sema"})
; => {:status 200 :headers {...} :body "..."}
```
````

All HTTP functions are supported: `http/get`, `http/post`, `http/put`, `http/delete`, `http/request`.

::: warning CORS Restrictions
Browser security rules (CORS) may block requests to servers that don't include `Access-Control-Allow-Origin` headers. Public APIs like httpbin.org work fine. If you get a network error, the target server likely doesn't allow cross-origin requests.
:::

````

**Step 2: Commit**

```bash
git add website/docs/stdlib/playground.md
git commit -m "docs: update playground docs — HTTP now works via fetch bridge"
````

---

## Task 9: Update the WASM shims design doc

**Files:**

- Modify: `docs/plans/2026-02-14-wasm-shims-design.md`

**Step 1: Update status and mark Phase 2 as complete**

At the top, update status line to:

```
**Status:** HTTP via fetch bridge implemented — 2026-02-17
```

In the "Tier 3: HTTP Stubs" section, update to:

```markdown
### Tier 3: HTTP via Fetch Bridge ✅ (Implemented 2026-02-17)

`http/get`, `http/post`, `http/put`, `http/delete`, `http/request` work via browser `fetch()` API using a replay-with-cache strategy. WASM HTTP fns check an in-memory cache; on miss they raise a marker error caught by `eval_async`, which performs the actual `fetch()`, caches the response, and replays evaluation.
```

In the Future Roadmap, update Phase 2:

```markdown
### Phase 2: HTTP via fetch ✅ (Implemented 2026-02-17)

Implemented via replay-with-cache strategy using `eval_async`/`eval_vm_async` + `web-sys` fetch.
```

**Step 2: Commit**

```bash
git add docs/plans/2026-02-14-wasm-shims-design.md
git commit -m "docs: mark HTTP fetch bridge as implemented in WASM shims design doc"
```

---

## Summary of changes

| File                                         | Change                                                                                        |
| -------------------------------------------- | --------------------------------------------------------------------------------------------- |
| `crates/sema-wasm/Cargo.toml`                | Add `wasm-bindgen-futures`, `web-sys` deps                                                    |
| `crates/sema-wasm/src/lib.rs`                | HTTP cache, marker infra, `wasm_http_request`, `perform_fetch`, `eval_async`, `eval_vm_async` |
| `playground/src/app.js`                      | Make `run()` async, use `eval_async`/`eval_vm_async`                                          |
| `playground/tests/playground.spec.ts`        | Add HTTP Playwright tests                                                                     |
| `website/docs/stdlib/playground.md`          | Move HTTP to working section, remove "coming soon"                                            |
| `docs/plans/2026-02-14-wasm-shims-design.md` | Mark Phase 2 complete                                                                         |

**No changes to:** `sema-core`, `sema-eval`, `sema-stdlib`, `sema-vm` — the entire feature lives in the WASM boundary layer.
