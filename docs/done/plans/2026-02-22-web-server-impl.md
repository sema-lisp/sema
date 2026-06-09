# Web Server Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add an HTTP server to Sema with data-driven routing, middleware composition, SSE streaming, and WebSocket support.

**Architecture:** Channel-bridged event loop — axum runs on a background tokio thread handling concurrent I/O, while Sema handler evaluation stays on the main thread (preserving Rc/single-threaded invariants). Requests flow through an mpsc channel; responses return via oneshot channels.

**Tech Stack:** axum 0.8 (HTTP server + WebSocket), tokio (async runtime, already present), serde_json (already present). New module at `crates/sema-stdlib/src/server.rs`.

**Design doc:** `docs/plans/2026-02-22-web-server-design.md`

---

### Task 1: Add axum dependency

**Files:**
- Modify: `crates/sema-stdlib/Cargo.toml:31-37`
- Modify: `Cargo.toml` (workspace root, add axum to workspace deps)

**Step 1: Add axum to workspace deps**

In root `Cargo.toml`, add to `[workspace.dependencies]`:

```toml
axum = { version = "0.8", features = ["ws"] }
```

**Step 2: Add axum to sema-stdlib's platform-gated deps**

In `crates/sema-stdlib/Cargo.toml`, under `[target.'cfg(not(target_arch = "wasm32"))'.dependencies]` (line 31), add:

```toml
axum.workspace = true
```

**Step 3: Verify it compiles**

Run: `cargo check -p sema-stdlib`
Expected: Clean compilation with no errors.

**Step 4: Commit**

```bash
git add Cargo.toml crates/sema-stdlib/Cargo.toml Cargo.lock
git commit -m "deps: add axum 0.8 for web server support"
```

---

### Task 2: Response helpers (pure functions)

These are simple map constructors — no server needed, easy to test.

**Files:**
- Create: `crates/sema-stdlib/src/server.rs`
- Modify: `crates/sema-stdlib/src/lib.rs:50-51` (add registration)
- Test: `crates/sema/tests/integration_test.rs` (add tests at end)

**Step 1: Write failing integration tests**

Add to `crates/sema/tests/integration_test.rs`:

```rust
// ── http response helpers ──────────────────────────────────────

#[test]
fn test_http_ok_with_string() {
    let result = eval(r#"(http/ok "hello")"#);
    let map = result.as_map_rc().unwrap();
    assert_eq!(map.get(&Value::keyword("status")), Some(&Value::int(200)));
    assert_eq!(
        map.get(&Value::keyword("body")),
        Some(&Value::string("\"hello\""))
    );
}

#[test]
fn test_http_ok_with_map() {
    let result = eval(r#"(http/ok {:msg "hi"})"#);
    let map = result.as_map_rc().unwrap();
    assert_eq!(map.get(&Value::keyword("status")), Some(&Value::int(200)));
    // body should be JSON-encoded
    let body = map.get(&Value::keyword("body")).unwrap();
    assert!(body.as_str().unwrap().contains("msg"));
}

#[test]
fn test_http_not_found() {
    let result = eval(r#"(http/not-found "gone")"#);
    let map = result.as_map_rc().unwrap();
    assert_eq!(map.get(&Value::keyword("status")), Some(&Value::int(404)));
}

#[test]
fn test_http_redirect() {
    let result = eval(r#"(http/redirect "/login")"#);
    let map = result.as_map_rc().unwrap();
    assert_eq!(map.get(&Value::keyword("status")), Some(&Value::int(302)));
    let headers = map.get(&Value::keyword("headers")).unwrap().as_map_rc().unwrap();
    assert_eq!(
        headers.get(&Value::string("location")),
        Some(&Value::string("/login"))
    );
}

#[test]
fn test_http_error_custom_status() {
    let result = eval(r#"(http/error 422 {:errors ["invalid"]})"#);
    let map = result.as_map_rc().unwrap();
    assert_eq!(map.get(&Value::keyword("status")), Some(&Value::int(422)));
}

#[test]
fn test_http_html() {
    let result = eval(r#"(http/html "<h1>Hi</h1>")"#);
    let map = result.as_map_rc().unwrap();
    assert_eq!(map.get(&Value::keyword("status")), Some(&Value::int(200)));
    let headers = map.get(&Value::keyword("headers")).unwrap().as_map_rc().unwrap();
    assert_eq!(
        headers.get(&Value::string("content-type")),
        Some(&Value::string("text/html"))
    );
    assert_eq!(
        map.get(&Value::keyword("body")),
        Some(&Value::string("<h1>Hi</h1>"))
    );
}

#[test]
fn test_http_text() {
    let result = eval(r#"(http/text "plain text")"#);
    let map = result.as_map_rc().unwrap();
    let headers = map.get(&Value::keyword("headers")).unwrap().as_map_rc().unwrap();
    assert_eq!(
        headers.get(&Value::string("content-type")),
        Some(&Value::string("text/plain"))
    );
}

#[test]
fn test_http_created() {
    let result = eval(r#"(http/created {:id 1})"#);
    let map = result.as_map_rc().unwrap();
    assert_eq!(map.get(&Value::keyword("status")), Some(&Value::int(201)));
}

#[test]
fn test_http_no_content() {
    let result = eval(r#"(http/no-content)"#);
    let map = result.as_map_rc().unwrap();
    assert_eq!(map.get(&Value::keyword("status")), Some(&Value::int(204)));
    assert_eq!(
        map.get(&Value::keyword("body")),
        Some(&Value::string(""))
    );
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p sema --test integration_test -- test_http_ok`
Expected: FAIL — `http/ok` is unbound.

**Step 3: Create `server.rs` with response helpers**

Create `crates/sema-stdlib/src/server.rs`:

```rust
use std::collections::BTreeMap;

use sema_core::{check_arity, Value, SemaError};

/// Build a JSON response map: {:status N :headers {...} :body "..."}
fn json_response(status: i64, body: &Value) -> Result<Value, SemaError> {
    let json_body = crate::json::value_to_json(body)?;
    let body_str = serde_json::to_string(&json_body)
        .map_err(|e| SemaError::eval(format!("http response: json encode: {e}")))?;

    let mut headers = BTreeMap::new();
    headers.insert(
        Value::string("content-type"),
        Value::string("application/json"),
    );

    let mut map = BTreeMap::new();
    map.insert(Value::keyword("status"), Value::int(status));
    map.insert(Value::keyword("headers"), Value::map(headers));
    map.insert(Value::keyword("body"), Value::string(&body_str));
    Ok(Value::map(map))
}

/// Build a response with custom content-type
fn typed_response(status: i64, content_type: &str, body: &str) -> Value {
    let mut headers = BTreeMap::new();
    headers.insert(
        Value::string("content-type"),
        Value::string(content_type),
    );

    let mut map = BTreeMap::new();
    map.insert(Value::keyword("status"), Value::int(status));
    map.insert(Value::keyword("headers"), Value::map(headers));
    map.insert(Value::keyword("body"), Value::string(body));
    Value::map(map)
}

pub fn register(env: &sema_core::Env) {
    // http/ok — JSON response with status 200
    crate::register_fn(env, "http/ok", |args| {
        check_arity!(args, "http/ok", 1);
        json_response(200, &args[0])
    });

    // http/created — JSON response with status 201
    crate::register_fn(env, "http/created", |args| {
        check_arity!(args, "http/created", 1);
        json_response(201, &args[0])
    });

    // http/no-content — empty response with status 204
    crate::register_fn(env, "http/no-content", |args| {
        check_arity!(args, "http/no-content", 0);
        let mut map = BTreeMap::new();
        map.insert(Value::keyword("status"), Value::int(204));
        map.insert(Value::keyword("headers"), Value::map(BTreeMap::new()));
        map.insert(Value::keyword("body"), Value::string(""));
        Ok(Value::map(map))
    });

    // http/not-found — JSON response with status 404
    crate::register_fn(env, "http/not-found", |args| {
        check_arity!(args, "http/not-found", 1);
        json_response(404, &args[0])
    });

    // http/redirect — 302 with location header
    crate::register_fn(env, "http/redirect", |args| {
        check_arity!(args, "http/redirect", 1);
        let url = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let mut headers = BTreeMap::new();
        headers.insert(Value::string("location"), Value::string(url));
        let mut map = BTreeMap::new();
        map.insert(Value::keyword("status"), Value::int(302));
        map.insert(Value::keyword("headers"), Value::map(headers));
        map.insert(Value::keyword("body"), Value::string(""));
        Ok(Value::map(map))
    });

    // http/error — JSON response with custom status
    crate::register_fn(env, "http/error", |args| {
        check_arity!(args, "http/error", 2);
        let status = args[0]
            .as_int()
            .ok_or_else(|| SemaError::type_error("integer", args[0].type_name()))?;
        json_response(status, &args[1])
    });

    // http/html — HTML response with status 200
    crate::register_fn(env, "http/html", |args| {
        check_arity!(args, "http/html", 1);
        let content = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        Ok(typed_response(200, "text/html", content))
    });

    // http/text — plain text response with status 200
    crate::register_fn(env, "http/text", |args| {
        check_arity!(args, "http/text", 1);
        let content = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        Ok(typed_response(200, "text/plain", content))
    });
}
```

**Step 4: Register the module in lib.rs**

In `crates/sema-stdlib/src/lib.rs`, add the module declaration (near line 10 with other cfg-gated modules):

```rust
#[cfg(not(target_arch = "wasm32"))]
mod server;
```

And in `register_stdlib()`, after the `http::register` call (line 51):

```rust
#[cfg(not(target_arch = "wasm32"))]
server::register(env);
```

Note: Response helpers don't need sandbox gating — they're pure map constructors.

**Step 5: Run tests to verify they pass**

Run: `cargo test -p sema --test integration_test -- test_http_ok test_http_not_found test_http_redirect test_http_error test_http_html test_http_text test_http_created test_http_no_content`
Expected: All PASS.

**Step 6: Commit**

```bash
git add crates/sema-stdlib/src/server.rs crates/sema-stdlib/src/lib.rs crates/sema/tests/integration_test.rs
git commit -m "feat: add http response helpers (http/ok, http/error, http/html, etc.)"
```

---

### Task 3: Route matching engine

Path pattern matching logic — pure Rust, no Sema Values yet.

**Files:**
- Modify: `crates/sema-stdlib/src/server.rs` (add route matching)

**Step 1: Write Rust unit tests for route matching**

Add to the bottom of `server.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_match_exact_path() {
        let params = match_path("/users", "/users");
        assert!(params.is_some());
        assert!(params.unwrap().is_empty());
    }

    #[test]
    fn test_match_root() {
        assert!(match_path("/", "/").is_some());
    }

    #[test]
    fn test_no_match_different_path() {
        assert!(match_path("/users", "/posts").is_none());
    }

    #[test]
    fn test_match_param() {
        let params = match_path("/users/:id", "/users/42").unwrap();
        assert_eq!(params.len(), 1);
        assert_eq!(params[0], ("id", "42"));
    }

    #[test]
    fn test_match_multiple_params() {
        let params = match_path("/users/:uid/posts/:pid", "/users/1/posts/99").unwrap();
        assert_eq!(params.len(), 2);
        assert_eq!(params[0], ("uid", "1"));
        assert_eq!(params[1], ("pid", "99"));
    }

    #[test]
    fn test_no_match_too_few_segments() {
        assert!(match_path("/users/:id", "/users").is_none());
    }

    #[test]
    fn test_no_match_too_many_segments() {
        assert!(match_path("/users", "/users/42").is_none());
    }

    #[test]
    fn test_match_wildcard() {
        let params = match_path("/files/*", "/files/a/b/c").unwrap();
        assert_eq!(params.len(), 1);
        assert_eq!(params[0], ("*", "a/b/c"));
    }

    #[test]
    fn test_match_trailing_slash_normalized() {
        assert!(match_path("/users", "/users/").is_some());
    }
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p sema-stdlib -- server::tests`
Expected: FAIL — `match_path` not defined.

**Step 3: Implement route matching**

Add to `server.rs` above `pub fn register`:

```rust
/// Match a path pattern against a request path.
/// Returns Some(vec of (param_name, value)) on match, None on no match.
/// Patterns: "/users/:id" matches "/users/42" with id="42"
///           "/files/*" matches "/files/a/b/c" with *="a/b/c"
fn match_path<'a>(pattern: &str, path: &'a str) -> Option<Vec<(&str, &'a str)>> {
    let pattern = pattern.trim_end_matches('/');
    let path = path.trim_end_matches('/');

    // Handle root path
    if pattern.is_empty() && path.is_empty() {
        return Some(vec![]);
    }

    let pat_segments: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
    let path_segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

    let mut params = Vec::new();

    for (i, pat_seg) in pat_segments.iter().enumerate() {
        if *pat_seg == "*" {
            // Wildcard: capture rest of path
            let rest = path_segments[i..].join("/");
            // We need to return a static-ish reference, so use a trick:
            // Actually, let's return owned strings instead.
            // We'll adjust the return type.
            params.push(("*", i));
            // Match succeeds regardless of remaining segments
            // We'll handle the owned string version below
            break;
        }

        if i >= path_segments.len() {
            return None;
        }

        if pat_seg.starts_with(':') {
            params.push((&pat_seg[1..], i));
        } else if *pat_seg != path_segments[i] {
            return None;
        }
    }

    // If no wildcard, segment counts must match
    if !pat_segments.last().map_or(false, |s| *s == "*") && pat_segments.len() != path_segments.len()
    {
        return None;
    }

    // Resolve indices to actual values
    Some(
        params
            .into_iter()
            .map(|(name, idx)| {
                if name == "*" {
                    (name, &path[path_segments[..idx].iter().map(|s| s.len() + 1).sum::<usize>()..])
                } else {
                    (name, path_segments[idx])
                }
            })
            .collect(),
    )
}
```

Note: The exact implementation may need adjustment during development — the test-first approach will catch issues. The key contract is: returns `None` for no match, `Some(params)` for a match with extracted parameters.

Actually, let's simplify with owned strings to avoid lifetime complications:

```rust
/// Match a path pattern against a request path.
/// Returns Some(vec of (param_name, value)) on match, None on no match.
fn match_path(pattern: &str, path: &str) -> Option<Vec<(String, String)>> {
    let pat_segs: Vec<&str> = pattern.trim_matches('/').split('/').filter(|s| !s.is_empty()).collect();
    let path_segs: Vec<&str> = path.trim_matches('/').split('/').filter(|s| !s.is_empty()).collect();

    // Handle root
    if pat_segs.is_empty() && path_segs.is_empty() {
        return Some(vec![]);
    }
    if pat_segs.is_empty() {
        return None;
    }

    let mut params = Vec::new();
    for (i, pat) in pat_segs.iter().enumerate() {
        if *pat == "*" {
            let rest = path_segs[i..].join("/");
            params.push(("*".to_string(), rest));
            return Some(params);
        }
        if i >= path_segs.len() {
            return None;
        }
        if pat.starts_with(':') {
            params.push((pat[1..].to_string(), path_segs[i].to_string()));
        } else if *pat != path_segs[i] {
            return None;
        }
    }

    if pat_segs.len() != path_segs.len() {
        return None;
    }

    Some(params)
}
```

Update the tests to use `String` tuples accordingly.

**Step 4: Run tests to verify they pass**

Run: `cargo test -p sema-stdlib -- server::tests`
Expected: All PASS.

**Step 5: Commit**

```bash
git add crates/sema-stdlib/src/server.rs
git commit -m "feat: add route path matching with params and wildcards"
```

---

### Task 4: http/router — data-driven routing

`http/router` takes a route table and returns a handler function that dispatches requests.

**Files:**
- Modify: `crates/sema-stdlib/src/server.rs` (add router)
- Test: `crates/sema/tests/integration_test.rs`

**Step 1: Write failing integration tests**

Add to integration tests:

```rust
#[test]
fn test_http_router_basic() {
    let result = eval(r#"
        (let [router (http/router
                       [[:get "/" (fn (req) (http/ok "home"))]])]
          (router {:method :get :path "/" :headers {} :query {} :params {} :body "" :remote "127.0.0.1"}))
    "#);
    let map = result.as_map_rc().unwrap();
    assert_eq!(map.get(&Value::keyword("status")), Some(&Value::int(200)));
}

#[test]
fn test_http_router_params() {
    let result = eval(r#"
        (let [router (http/router
                       [[:get "/users/:id" (fn (req) (http/ok (:params req)))]])]
          (router {:method :get :path "/users/42" :headers {} :query {} :params {} :body "" :remote "127.0.0.1"}))
    "#);
    let map = result.as_map_rc().unwrap();
    let body = map.get(&Value::keyword("body")).unwrap();
    // Body should contain the param id=42
    assert!(body.as_str().unwrap().contains("42"));
}

#[test]
fn test_http_router_404() {
    let result = eval(r#"
        (let [router (http/router
                       [[:get "/" (fn (req) (http/ok "home"))]])]
          (router {:method :get :path "/missing" :headers {} :query {} :params {} :body "" :remote "127.0.0.1"}))
    "#);
    let map = result.as_map_rc().unwrap();
    assert_eq!(map.get(&Value::keyword("status")), Some(&Value::int(404)));
}

#[test]
fn test_http_router_method_matching() {
    let result = eval(r#"
        (let [router (http/router
                       [[:post "/data" (fn (req) (http/ok "posted"))]])]
          (router {:method :get :path "/data" :headers {} :query {} :params {} :body "" :remote "127.0.0.1"}))
    "#);
    let map = result.as_map_rc().unwrap();
    assert_eq!(map.get(&Value::keyword("status")), Some(&Value::int(404)));
}

#[test]
fn test_http_router_any_method() {
    let result = eval(r#"
        (let [router (http/router
                       [[:any "/health" (fn (req) (http/ok "up"))]])]
          (router {:method :delete :path "/health" :headers {} :query {} :params {} :body "" :remote "127.0.0.1"}))
    "#);
    let map = result.as_map_rc().unwrap();
    assert_eq!(map.get(&Value::keyword("status")), Some(&Value::int(200)));
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p sema --test integration_test -- test_http_router`
Expected: FAIL — `http/router` is unbound.

**Step 3: Implement http/router**

The router needs `NativeFn::with_ctx` because it must call handler lambdas via `call_callback`. Add to `server.rs`:

```rust
use std::any::Any;
use std::rc::Rc;
use sema_core::{EvalContext, NativeFn, ValueView, call_callback, intern};

/// A stored route: (method_keyword, path_pattern, handler_value)
struct Route {
    method: String,   // "get", "post", "any", etc.
    pattern: String,  // "/users/:id"
    handler: Value,   // Lambda or NativeFn
}

fn register_router(env: &sema_core::Env) {
    // http/router — takes route table, returns handler function
    env.set(
        intern("http/router"),
        Value::native_fn(NativeFn::with_ctx("http/router", |ctx, args| {
            check_arity!(args, "http/router", 1);

            // Parse route table: list of [method path handler] vectors
            let route_list = args[0]
                .as_list()
                .ok_or_else(|| SemaError::type_error("list", args[0].type_name()))?;

            let mut routes = Vec::new();
            for route_vec in route_list.iter() {
                let parts = route_vec
                    .as_list()
                    .ok_or_else(|| SemaError::eval("http/router: each route must be a vector [method path handler]"))?;
                if parts.len() < 3 {
                    return Err(SemaError::eval(
                        "http/router: each route must be [method path handler]",
                    ));
                }

                let method = match parts[0].view() {
                    ValueView::Keyword(s) => sema_core::resolve(s).to_lowercase(),
                    _ => return Err(SemaError::type_error("keyword", parts[0].type_name())),
                };
                let pattern = parts[1]
                    .as_str()
                    .ok_or_else(|| SemaError::type_error("string", parts[1].type_name()))?
                    .to_string();
                let handler = parts[2].clone();

                routes.push(Route { method, pattern, handler });
            }

            // Return a NativeFn that dispatches requests through the route table
            let routes = Rc::new(routes);
            Ok(Value::native_fn(NativeFn::with_ctx("http/router:dispatch", move |ctx, args| {
                check_arity!(args, "router", 1);
                let req = &args[0];

                // Extract method and path from request map
                let req_map = req
                    .as_map_rc()
                    .ok_or_else(|| SemaError::type_error("map", req.type_name()))?;

                let method = match req_map.get(&Value::keyword("method")) {
                    Some(v) => match v.view() {
                        ValueView::Keyword(s) => sema_core::resolve(s).to_lowercase(),
                        ValueView::String(s) => s.to_lowercase(),
                        _ => return Err(SemaError::eval("router: :method must be keyword or string")),
                    },
                    None => return Err(SemaError::eval("router: request missing :method")),
                };

                let path = match req_map.get(&Value::keyword("path")) {
                    Some(v) => v
                        .as_str()
                        .ok_or_else(|| SemaError::eval("router: :path must be string"))?
                        .to_string(),
                    None => return Err(SemaError::eval("router: request missing :path")),
                };

                // Find matching route
                for route in routes.iter() {
                    if route.method != "any" && route.method != method {
                        continue;
                    }
                    if let Some(params) = match_path(&route.pattern, &path) {
                        // Build params map and merge into request
                        let mut params_map = BTreeMap::new();
                        for (k, v) in &params {
                            params_map.insert(Value::keyword(k), Value::string(v));
                        }
                        let mut new_req = req_map.as_ref().clone();
                        new_req.insert(Value::keyword("params"), Value::map(params_map));

                        return call_callback(ctx, &route.handler, &[Value::map(new_req)]);
                    }
                }

                // No match — return 404
                json_response(404, &Value::string("Not Found"))
            })))
        })),
    );
}
```

Call `register_router(env)` from the `register()` function.

**Step 4: Run tests**

Run: `cargo test -p sema --test integration_test -- test_http_router`
Expected: All PASS.

**Step 5: Commit**

```bash
git add crates/sema-stdlib/src/server.rs crates/sema/tests/integration_test.rs
git commit -m "feat: add http/router with data-driven routing and param extraction"
```

---

### Task 5: http/serve — the server loop

The core server: spawns axum on a background thread, runs the evaluator loop on the main thread.

**Files:**
- Modify: `crates/sema-stdlib/src/server.rs` (add server)
- Modify: `crates/sema-stdlib/src/lib.rs` (update registration to pass sandbox)
- Test: `crates/sema/tests/integration_test.rs` (integration test with child process)

**Step 1: Implement request/response conversion**

Add to `server.rs`:

```rust
use tokio::sync::{mpsc, oneshot};

/// Message sent from the axum thread to the evaluator thread
pub(crate) enum ServerRequest {
    Http {
        request: Value,
        respond: oneshot::Sender<Value>,
    },
}

/// Convert method string to Sema keyword
fn method_keyword(method: &str) -> Value {
    Value::keyword(&method.to_lowercase())
}

/// Convert query string "a=1&b=2" to Sema map {:a "1" :b "2"}
fn parse_query_string(query: Option<&str>) -> Value {
    let mut map = BTreeMap::new();
    if let Some(qs) = query {
        for pair in qs.split('&') {
            if let Some((k, v)) = pair.split_once('=') {
                map.insert(Value::keyword(k), Value::string(v));
            }
        }
    }
    Value::map(map)
}

/// Convert a Sema response map to an axum response
fn sema_to_axum_response(val: &Value) -> axum::response::Response {
    use axum::http::{StatusCode, header};
    use axum::response::IntoResponse;

    let map = match val.as_map_rc() {
        Some(m) => m,
        None => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "Handler returned non-map").into_response();
        }
    };

    let status = map
        .get(&Value::keyword("status"))
        .and_then(|v| v.as_int())
        .unwrap_or(200) as u16;

    let body = map
        .get(&Value::keyword("body"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let mut response = axum::response::Response::builder()
        .status(StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR));

    if let Some(headers_val) = map.get(&Value::keyword("headers")) {
        if let Some(headers) = headers_val.as_map_rc() {
            for (k, v) in headers.iter() {
                let key = k.as_str().unwrap_or_default();
                let val = v.as_str().unwrap_or_default();
                if let (Ok(name), Ok(value)) = (
                    header::HeaderName::from_bytes(key.as_bytes()),
                    header::HeaderValue::from_str(val),
                ) {
                    response = response.header(name, value);
                }
            }
        }
    }

    response.body(axum::body::Body::from(body)).unwrap_or_else(|_| {
        (StatusCode::INTERNAL_SERVER_ERROR, "Failed to build response").into_response()
    })
}
```

**Step 2: Implement http/serve**

Add the server function in `server.rs`. This needs both sandbox gating (NETWORK cap) and eval context (to call handlers):

```rust
fn register_serve(env: &sema_core::Env, sandbox: &sema_core::Sandbox) {
    use sema_core::Caps;

    let sandbox = sandbox.clone();
    env.set(
        intern("http/serve"),
        Value::native_fn(NativeFn::with_ctx("http/serve", move |ctx, args| {
            // Check sandbox permission
            if !sandbox.is_unrestricted() {
                sandbox.check(Caps::NETWORK, "http/serve")?;
            }

            check_arity!(args, "http/serve", 1..=2);
            let handler = args[0].clone();

            // Parse options
            let port: u16 = args
                .get(1)
                .and_then(|v| v.as_map_rc())
                .and_then(|m| m.get(&Value::keyword("port")))
                .and_then(|v| v.as_int())
                .map(|n| n as u16)
                .unwrap_or(3000);

            let host = args
                .get(1)
                .and_then(|v| v.as_map_rc())
                .and_then(|m| m.get(&Value::keyword("host")))
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .unwrap_or_else(|| "0.0.0.0".to_string());

            let (tx, mut rx) = mpsc::channel::<ServerRequest>(256);

            // Spawn tokio + axum on a background thread
            let bind_addr = format!("{host}:{port}");
            let bind_addr_clone = bind_addr.clone();

            // Use a oneshot to signal when the server is ready (or failed to bind)
            let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();

            std::thread::spawn(move || {
                let rt = tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                    .expect("Failed to create tokio runtime for http/serve");

                rt.block_on(async move {
                    let tx = tx;
                    let app = axum::Router::new().fallback(move |req: axum::extract::Request| {
                        let tx = tx.clone();
                        async move {
                            // Convert axum request to Sema Value map
                            let method = req.method().to_string();
                            let uri = req.uri().clone();
                            let path = uri.path().to_string();
                            let query = uri.query().map(|s| s.to_string());

                            let mut headers_map = BTreeMap::new();
                            for (k, v) in req.headers() {
                                if let Ok(val) = v.to_str() {
                                    headers_map
                                        .insert(Value::string(k.as_str()), Value::string(val));
                                }
                            }

                            // Read body
                            let body_bytes = axum::body::to_bytes(req.into_body(), 10 * 1024 * 1024)
                                .await
                                .unwrap_or_default();
                            let body_str = String::from_utf8_lossy(&body_bytes).to_string();

                            // Build request map
                            let mut req_map = BTreeMap::new();
                            req_map.insert(Value::keyword("method"), method_keyword(&method));
                            req_map.insert(Value::keyword("path"), Value::string(&path));
                            req_map.insert(
                                Value::keyword("headers"),
                                Value::map(headers_map.clone()),
                            );
                            req_map.insert(
                                Value::keyword("query"),
                                parse_query_string(query.as_deref()),
                            );
                            req_map.insert(Value::keyword("params"), Value::map(BTreeMap::new()));
                            req_map.insert(Value::keyword("body"), Value::string(&body_str));

                            // Auto-parse JSON body
                            let is_json = headers_map
                                .iter()
                                .any(|(k, v)| {
                                    k.as_str().map_or(false, |s| s == "content-type")
                                        && v.as_str().map_or(false, |s| s.contains("json"))
                                });
                            if is_json && !body_str.is_empty() {
                                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&body_str)
                                {
                                    req_map.insert(
                                        Value::keyword("json"),
                                        crate::json::json_to_value(&json),
                                    );
                                }
                            }

                            let sema_req = Value::map(req_map);

                            // Send to evaluator thread and wait for response
                            let (resp_tx, resp_rx) = oneshot::channel();
                            if tx
                                .send(ServerRequest::Http {
                                    request: sema_req,
                                    respond: resp_tx,
                                })
                                .await
                                .is_err()
                            {
                                return axum::response::Response::builder()
                                    .status(500)
                                    .body(axum::body::Body::from("Server shutting down"))
                                    .unwrap();
                            }

                            match resp_rx.await {
                                Ok(val) => sema_to_axum_response(&val),
                                Err(_) => axum::response::Response::builder()
                                    .status(500)
                                    .body(axum::body::Body::from("Handler failed"))
                                    .unwrap(),
                            }
                        }
                    });

                    match tokio::net::TcpListener::bind(&bind_addr_clone).await {
                        Ok(listener) => {
                            let _ = ready_tx.send(Ok(()));
                            let _ = axum::serve(listener, app).await;
                        }
                        Err(e) => {
                            let _ = ready_tx.send(Err(format!("Failed to bind {bind_addr_clone}: {e}")));
                        }
                    }
                });
            });

            // Wait for server to be ready or fail
            match ready_rx.recv() {
                Ok(Ok(())) => {
                    eprintln!("Listening on {bind_addr}");
                }
                Ok(Err(e)) => {
                    return Err(SemaError::Io(e));
                }
                Err(_) => {
                    return Err(SemaError::Io("Server thread died before binding".to_string()));
                }
            }

            // Main thread: evaluator loop
            while let Some(server_req) = rx.blocking_recv() {
                match server_req {
                    ServerRequest::Http { request, respond } => {
                        let result = call_callback(ctx, &handler, &[request]);
                        let response = match result {
                            Ok(val) => val,
                            Err(e) => {
                                eprintln!("Handler error: {e}");
                                json_response(500, &Value::string("Internal server error"))
                                    .unwrap_or_else(|_| Value::nil())
                            }
                        };
                        let _ = respond.send(response);
                    }
                }
            }

            Ok(Value::nil())
        })),
    );
}
```

**Step 3: Update registration**

Change `server::register` to accept sandbox:

```rust
pub fn register(env: &sema_core::Env, sandbox: &sema_core::Sandbox) {
    // Response helpers (no gating needed)
    // ... existing register_fn calls ...

    // Router
    register_router(env);

    // Server (needs sandbox for NETWORK cap)
    register_serve(env, sandbox);
}
```

In `lib.rs`, update the call:
```rust
#[cfg(not(target_arch = "wasm32"))]
server::register(env, sandbox);
```

**Step 4: Write integration test (child process based)**

Add to integration tests:

```rust
#[test]
#[ignore] // requires network
fn test_http_serve_basic() {
    use std::process::{Command, Stdio};
    use std::time::Duration;

    let mut child = Command::new(env!("CARGO_BIN_EXE_sema"))
        .arg("-e")
        .arg(r#"(http/serve (fn (req) (http/ok {:path (:path req)})) {:port 19876})"#)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn sema");

    // Wait for server to start
    std::thread::sleep(Duration::from_millis(500));

    // Make request
    let resp = reqwest::blocking::get("http://127.0.0.1:19876/test")
        .expect("Failed to GET");
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().expect("Failed to parse JSON");
    assert_eq!(body["path"], "/test");

    child.kill().ok();
    child.wait().ok();
}
```

**Step 5: Run tests**

Run: `cargo test -p sema --test integration_test -- test_http_serve_basic --ignored`
Expected: PASS (server starts, responds, gets killed).

**Step 6: Commit**

```bash
git add crates/sema-stdlib/src/server.rs crates/sema-stdlib/src/lib.rs crates/sema/tests/integration_test.rs
git commit -m "feat: add http/serve with channel-bridged event loop"
```

---

### Task 6: SSE streaming support

**Files:**
- Modify: `crates/sema-stdlib/src/server.rs`
- Test: `crates/sema/tests/integration_test.rs`

**Step 1: Add SSE variant to ServerRequest**

Extend the `ServerRequest` enum:

```rust
pub(crate) enum ServerRequest {
    Http {
        request: Value,
        respond: oneshot::Sender<Value>,
    },
    Sse {
        request: Value,
        stream_handler: Value,  // the fn passed to http/stream
        sender: mpsc::Sender<String>,
        done: oneshot::Sender<()>,
    },
}
```

**Step 2: Implement http/stream marker**

`http/stream` returns a special map with a `__stream_handler` key that the server loop detects:

```rust
crate::register_fn(env, "http/stream", |args| {
    check_arity!(args, "http/stream", 1);
    let mut map = BTreeMap::new();
    map.insert(Value::keyword("__stream"), Value::bool(true));
    map.insert(Value::keyword("__stream_handler"), args[0].clone());
    Ok(Value::map(map))
});
```

**Step 3: Detect stream responses in the server loop**

In the `http/serve` evaluator loop, after calling the handler, check if the response is an SSE stream marker:

```rust
ServerRequest::Http { request, respond } => {
    let result = call_callback(ctx, &handler, &[request.clone()]);
    match result {
        Ok(val) => {
            // Check for SSE stream marker
            if let Some(map) = val.as_map_rc() {
                if map.get(&Value::keyword("__stream")).map_or(false, |v| v.is_truthy()) {
                    if let Some(stream_handler) = map.get(&Value::keyword("__stream_handler")) {
                        // Create channel for SSE events
                        let (sse_tx, sse_rx) = mpsc::channel::<String>(256);
                        let (done_tx, done_rx) = oneshot::channel::<()>();

                        // Send the SSE channel info back to axum
                        // We need a different approach: send the sse_rx to the axum side
                        // ... (see step 4 for the full approach)
                    }
                }
            }
            let _ = respond.send(val);
        }
        // ...
    }
}
```

**Step 4: Full SSE implementation**

The tricky part: the axum handler needs to return a streaming response, but the Sema evaluator produces events synchronously. Solution:

1. When the evaluator detects a stream marker, it sends back a special response through the oneshot that includes the `mpsc::Receiver<String>` for the SSE event stream.
2. The Sema handler's `send` function pushes to `mpsc::Sender<String>`, which the axum side reads and streams as SSE.
3. This requires wrapping the response in an enum that can carry either a Value or an SSE channel.

Use `Arc<Mutex<Option<mpsc::Receiver<String>>>>` to pass the receiver across threads. Alternatively, redesign the response type:

```rust
pub(crate) enum ServerResponse {
    Value(Value),
    Sse(mpsc::Receiver<String>),
}

pub(crate) enum ServerRequest {
    Http {
        request: Value,
        respond: oneshot::Sender<ServerResponse>,
    },
}
```

The evaluator loop becomes:
```rust
ServerRequest::Http { request, respond } => {
    let result = call_callback(ctx, &handler, &[request]);
    match result {
        Ok(val) => {
            if let Some(map) = val.as_map_rc() {
                if map.get(&Value::keyword("__stream")).map_or(false, |v| v.is_truthy()) {
                    if let Some(stream_handler) = map.get(&Value::keyword("__stream_handler")) {
                        let (sse_tx, sse_rx) = mpsc::channel::<String>(256);
                        let _ = respond.send(ServerResponse::Sse(sse_rx));

                        // Create a send function for the Sema handler
                        let send_fn = Value::native_fn(NativeFn::simple("sse-send", move |args| {
                            check_arity!(args, "send", 1);
                            let msg = args[0].as_str()
                                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
                            let _ = sse_tx.blocking_send(msg.to_string());
                            Ok(Value::nil())
                        }));

                        let _ = call_callback(ctx, stream_handler, &[send_fn]);
                        continue; // stream is done
                    }
                }
            }
            let _ = respond.send(ServerResponse::Value(val));
        }
        Err(e) => {
            eprintln!("Handler error: {e}");
            let err_resp = json_response(500, &Value::string("Internal server error"))
                .unwrap_or(Value::nil());
            let _ = respond.send(ServerResponse::Value(err_resp));
        }
    }
}
```

On the axum side, handle `ServerResponse::Sse`:
```rust
match resp_rx.await {
    Ok(ServerResponse::Value(val)) => sema_to_axum_response(&val),
    Ok(ServerResponse::Sse(mut sse_rx)) => {
        use axum::response::sse::{Event, Sse};
        use futures::stream::Stream;
        use tokio_stream::wrappers::ReceiverStream;

        let stream = ReceiverStream::new(sse_rx)
            .map(|data| Ok::<_, std::convert::Infallible>(Event::default().data(data)));
        Sse::new(stream).into_response()
    }
    Err(_) => { /* 500 error */ }
}
```

Note: This may need `tokio-stream` as a dependency. Add it if needed:
```toml
tokio-stream = "0.1"
```

**Step 5: Integration test (ignored, requires network)**

```rust
#[test]
#[ignore]
fn test_http_serve_sse() {
    use std::process::{Command, Stdio};
    use std::time::Duration;
    use std::io::Read;

    let mut child = Command::new(env!("CARGO_BIN_EXE_sema"))
        .arg("-e")
        .arg(r#"
            (http/serve
              (http/router
                [[:get "/stream"
                  (fn (req)
                    (http/stream (fn (send)
                      (send "hello")
                      (send "world"))))]])
              {:port 19877})
        "#)
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn sema");

    std::thread::sleep(Duration::from_millis(500));

    let resp = reqwest::blocking::get("http://127.0.0.1:19877/stream")
        .expect("Failed to GET");
    let body = resp.text().unwrap();
    assert!(body.contains("hello"));
    assert!(body.contains("world"));

    child.kill().ok();
    child.wait().ok();
}
```

**Step 6: Run tests and commit**

Run: `cargo test -p sema --test integration_test -- test_http_serve_sse --ignored`

```bash
git add crates/sema-stdlib/src/server.rs crates/sema/tests/integration_test.rs Cargo.toml crates/sema-stdlib/Cargo.toml
git commit -m "feat: add SSE streaming via http/stream"
```

---

### Task 7: WebSocket support

**Files:**
- Modify: `crates/sema-stdlib/src/server.rs`
- Test: `crates/sema/tests/integration_test.rs`

**Step 1: Add WebSocket route type**

Routes with `:ws` method type get special treatment. The router needs to signal to the axum fallback that a WebSocket upgrade is needed.

Design: When the router matches a `:ws` route, it returns a special marker response (like SSE) with the handler and a `__websocket` flag. The server loop handles it differently.

**Step 2: Add WS variant to ServerRequest**

```rust
pub(crate) enum ServerRequest {
    Http {
        request: Value,
        respond: oneshot::Sender<ServerResponse>,
    },
    WebSocket {
        request: Value,
        handler: Value,
        incoming: mpsc::Receiver<String>,
        outgoing: mpsc::Sender<String>,
        done: oneshot::Sender<()>,
    },
}
```

**Step 3: Implement WebSocket handling**

This is the most complex part. The axum side needs to:
1. Detect WebSocket upgrade requests
2. Perform the HTTP upgrade
3. Bridge the WebSocket connection to mpsc channels
4. The evaluator thread runs the Sema WS handler with `{:send fn :recv fn :close fn}`

The approach:
- The axum fallback handler checks the route match for `:ws` type
- If matched, it uses `axum::extract::ws::WebSocketUpgrade` to upgrade
- After upgrade, it bridges the WS connection to channels
- The evaluator thread calls the handler with a connection map

Implementation sketch:

```rust
// In the router dispatch, if method is "ws":
if route.method == "ws" {
    let mut map = BTreeMap::new();
    map.insert(Value::keyword("__websocket"), Value::bool(true));
    map.insert(Value::keyword("__ws_handler"), route.handler.clone());
    map.insert(Value::keyword("params"), Value::map(params_map));
    return Ok(Value::map(map));
}
```

On the axum side, the fallback needs to handle WebSocket upgrades:

```rust
// This requires a more complex axum setup:
// Instead of a simple fallback, use a handler that can extract WebSocketUpgrade
async fn handle_request(
    ws: Option<axum::extract::WebSocketUpgrade>,
    req: axum::extract::Request,
    tx: mpsc::Sender<ServerRequest>,
) -> axum::response::Response {
    // ... existing logic to send to evaluator and get response ...
    // If response has __websocket flag AND ws upgrade is available:
    if is_ws_response && let Some(ws) = ws {
        let (ws_in_tx, ws_in_rx) = mpsc::channel::<String>(256);
        let (ws_out_tx, ws_out_rx) = mpsc::channel::<String>(256);
        let (done_tx, done_rx) = oneshot::channel();

        // Send WS request to evaluator
        let _ = tx.send(ServerRequest::WebSocket {
            request: sema_req,
            handler: ws_handler,
            incoming: ws_in_rx,
            outgoing: ws_out_tx,
            done: done_tx,
        }).await;

        return ws.on_upgrade(move |socket| async move {
            let (mut ws_sender, mut ws_receiver) = socket.split();
            // Forward incoming WS messages to evaluator
            tokio::spawn(async move {
                while let Some(Ok(msg)) = ws_receiver.next().await {
                    if let axum::extract::ws::Message::Text(text) = msg {
                        let _ = ws_in_tx.send(text.to_string()).await;
                    }
                }
            });
            // Forward outgoing messages from evaluator to WS
            let mut ws_out_rx = ws_out_rx;
            while let Some(msg) = ws_out_rx.recv().await {
                let _ = ws_sender.send(axum::extract::ws::Message::Text(msg.into())).await;
            }
        });
    }
}
```

On the evaluator side, create the connection map:

```rust
ServerRequest::WebSocket { request, handler, incoming, outgoing, done } => {
    let outgoing_clone = outgoing.clone();
    let send_fn = Value::native_fn(NativeFn::simple("ws-send", move |args| {
        check_arity!(args, "send", 1);
        let msg = args[0].as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        outgoing_clone.blocking_send(msg.to_string())
            .map_err(|_| SemaError::Io("WebSocket closed".to_string()))?;
        Ok(Value::nil())
    }));

    let recv_fn = Value::native_fn(NativeFn::simple("ws-recv", move |args| {
        check_arity!(args, "recv", 0);
        match incoming.blocking_recv() {
            Some(msg) => Ok(Value::string(&msg)),
            None => Ok(Value::nil()),  // connection closed
        }
    }));

    let close_fn = Value::native_fn(NativeFn::simple("ws-close", move |_args| {
        drop(outgoing);
        Ok(Value::nil())
    }));

    let mut conn = BTreeMap::new();
    conn.insert(Value::keyword("send"), send_fn);
    conn.insert(Value::keyword("recv"), recv_fn);
    conn.insert(Value::keyword("close"), close_fn);

    let _ = call_callback(ctx, &handler, &[Value::map(conn)]);
    let _ = done.send(());
}
```

Note: The `recv_fn` captures `incoming` by move, but `incoming` is an `mpsc::Receiver` which is not `Clone`. This needs careful handling — `Rc<RefCell<mpsc::Receiver>>` or similar. Adjust during implementation.

**Step 4: Integration test**

```rust
#[test]
#[ignore]
fn test_http_serve_websocket() {
    use std::process::{Command, Stdio};
    use std::time::Duration;
    use tungstenite::connect;

    let mut child = Command::new(env!("CARGO_BIN_EXE_sema"))
        .arg("-e")
        .arg(r#"
            (http/serve
              (http/router
                [[:ws "/echo" (fn (conn)
                  (let [msg ((:recv conn))]
                    ((:send conn) (string-append "echo:" msg))))]])
              {:port 19878})
        "#)
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn sema");

    std::thread::sleep(Duration::from_millis(500));

    let (mut ws, _) = connect("ws://127.0.0.1:19878/echo").expect("WS connect failed");
    ws.send(tungstenite::Message::Text("hello".into())).unwrap();
    let reply = ws.read().unwrap();
    assert_eq!(reply.to_text().unwrap(), "echo:hello");

    child.kill().ok();
    child.wait().ok();
}
```

Note: This test needs `tungstenite` as a dev-dependency in `crates/sema/Cargo.toml`. May need to add it or use a different WS client library for testing.

**Step 5: Commit**

```bash
git add crates/sema-stdlib/src/server.rs crates/sema/tests/integration_test.rs crates/sema/Cargo.toml
git commit -m "feat: add WebSocket support via :ws route type"
```

---

### Task 8: Example file and documentation

**Files:**
- Create: `examples/web-server.sema`
- Modify: `crates/sema/tests/integration_test.rs` (if any fixes needed)

**Step 1: Create example file**

Create `examples/web-server.sema`:

```scheme
#!/usr/bin/env sema

;; Simple web server example
;; Run: cargo run -- examples/web-server.sema
;; Test: curl http://localhost:3000/hello

(define (handle-home req)
  (http/html "<h1>Welcome to Sema</h1><p>A Lisp with superpowers.</p>"))

(define (handle-greet req)
  (let [name (or (:name (:params req)) "world")]
    (http/ok {:greeting (string-append "Hello, " name "!")})))

(define (handle-echo req)
  (http/ok {:method (:method req)
            :path   (:path req)
            :query  (:query req)
            :body   (:body req)}))

(define (handle-health _)
  (http/ok {:status "up"}))

;; Middleware: add CORS headers
(define (with-cors handler)
  (fn (req)
    (let [resp (handler req)]
      (if (map? resp)
        (let [headers (or (:headers resp) {})
              new-headers (map/merge headers
                {"access-control-allow-origin" "*"
                 "access-control-allow-methods" "GET, POST, PUT, DELETE"})]
          (assoc resp :headers new-headers))
        resp))))

;; Middleware: request logging
(define (with-logging handler)
  (fn (req)
    (let [resp (handler req)]
      (println (:method req) (:path req) "->" (:status resp))
      resp)))

;; Routes
(define routes
  [[:get  "/"            handle-home]
   [:get  "/health"      handle-health]
   [:get  "/greet/:name" handle-greet]
   [:any  "/echo"        handle-echo]])

;; Build app with middleware
(define app
  (-> (http/router routes)
      with-cors
      with-logging))

(println "Starting server on http://localhost:3000")
(http/serve app {:port 3000})
```

**Step 2: Verify the example runs**

Run: `cargo run -- examples/web-server.sema`
Then in another terminal: `curl -s http://localhost:3000/greet/Ada | jq .`
Expected: `{"greeting": "Hello, Ada!"}`

**Step 3: Commit**

```bash
git add examples/web-server.sema
git commit -m "feat: add web server example"
```

---

### Task 9: Run full test suite and cleanup

**Step 1: Run all tests**

```bash
cargo test -p sema-stdlib
cargo test -p sema --test integration_test
cargo clippy -p sema-stdlib -- -D warnings
cargo fmt --check
```

Fix any warnings or failures.

**Step 2: Run HTTP-specific tests**

```bash
cargo test -p sema --test integration_test -- test_http_serve --ignored
```

**Step 3: Final commit if any fixes**

```bash
git add -A
git commit -m "fix: address test failures and clippy warnings"
```

---

## Summary

| Task | What | Estimated Effort |
|------|------|-----------------|
| 1 | Add axum dependency | Small |
| 2 | Response helpers | Small |
| 3 | Route matching engine | Small |
| 4 | http/router | Medium |
| 5 | http/serve | Large |
| 6 | SSE streaming | Medium |
| 7 | WebSocket support | Large |
| 8 | Example + docs | Small |
| 9 | Test suite + cleanup | Small |

Tasks 1-4 are low-risk, well-isolated, and independently testable. Task 5 is the core complexity. Tasks 6-7 build on 5. Task 8-9 are polish.
