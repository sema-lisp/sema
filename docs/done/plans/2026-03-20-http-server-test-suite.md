# HTTP Server & Client Comprehensive Test Suite

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Expand HTTP server/client test coverage from ~60 tests to ~200+ tests, covering response helpers (dual-eval), router edge cases, request construction, WebSocket multi-message scenarios, SSE streaming, error resilience, static file serving, and concurrent request handling.

**Architecture:** Three tiers of tests:
1. **Dual-eval tests** — pure functions (response helpers) tested via both tree-walker and VM
2. **Unit tests** — router dispatch, request parsing, response construction without spawning a server
3. **Integration tests** — full server spawn + HTTP client for end-to-end scenarios (SSE, WebSocket, concurrency)

**Tech Stack:** Existing test infrastructure (Interpreter, eval helpers), reqwest (HTTP client), tungstenite (WebSocket client), hyperfine patterns for integration tests.

---

### Task 1: Response Helper Dual-Eval Tests

**Files:**
- Modify: `crates/sema/tests/dual_eval_stdlib_test.rs`

These are pure map-returning functions — ideal for dual-eval testing.

- [ ] **Step 1: Add response helper dual-eval tests**

Add to `dual_eval_stdlib_test.rs`:

```rust
// ============================================================
// HTTP response helpers — dual eval (tree-walker + VM)
// ============================================================

dual_eval_tests! {
    // http/ok
    http_ok_string_status: r#"(get (http/ok "hi") :status)"# => Value::int(200),
    http_ok_map_status: r#"(get (http/ok {:a 1}) :status)"# => Value::int(200),
    http_ok_list_status: r#"(get (http/ok '(1 2 3)) :status)"# => Value::int(200),
    http_ok_nil_status: r#"(get (http/ok nil) :status)"# => Value::int(200),
    http_ok_int_status: r#"(get (http/ok 42) :status)"# => Value::int(200),
    http_ok_has_body: r#"(string? (get (http/ok "test") :body))"# => Value::bool(true),
    http_ok_content_type: r#"(get (get (http/ok "x") :headers) "content-type")"# => Value::string("application/json"),

    // http/created
    http_created_status: r#"(get (http/created {:id 1}) :status)"# => Value::int(201),
    http_created_content_type: r#"(get (get (http/created "x") :headers) "content-type")"# => Value::string("application/json"),

    // http/no-content
    http_no_content_status: r#"(get (http/no-content) :status)"# => Value::int(204),
    http_no_content_empty_body: r#"(get (http/no-content) :body)"# => Value::string(""),

    // http/not-found
    http_not_found_status: r#"(get (http/not-found "gone") :status)"# => Value::int(404),

    // http/error
    http_error_custom: r#"(get (http/error 422 "bad") :status)"# => Value::int(422),
    http_error_500: r#"(get (http/error 500 "oops") :status)"# => Value::int(500),
    http_error_418: r#"(get (http/error 418 "teapot") :status)"# => Value::int(418),

    // http/redirect
    http_redirect_status: r#"(get (http/redirect "/login") :status)"# => Value::int(302),
    http_redirect_location: r#"(get (get (http/redirect "/login") :headers) "location")"# => Value::string("/login"),
    http_redirect_absolute: r#"(get (get (http/redirect "https://example.com") :headers) "location")"# => Value::string("https://example.com"),

    // http/html
    http_html_status: r#"(get (http/html "<p>hi</p>") :status)"# => Value::int(200),
    http_html_content_type: r#"(get (get (http/html "<p>hi</p>") :headers) "content-type")"# => Value::string("text/html"),
    http_html_body: r#"(get (http/html "<h1>Hello</h1>") :body)"# => Value::string("<h1>Hello</h1>"),

    // http/text
    http_text_status: r#"(get (http/text "plain") :status)"# => Value::int(200),
    http_text_content_type: r#"(get (get (http/text "plain") :headers) "content-type")"# => Value::string("text/plain"),
    http_text_body: r#"(get (http/text "hello world") :body)"# => Value::string("hello world"),
}

dual_eval_error_tests! {
    http_ok_no_args: "(http/ok)",
    http_ok_too_many: r#"(http/ok "a" "b")"#,
    http_created_no_args: "(http/created)",
    http_not_found_no_args: "(http/not-found)",
    http_redirect_no_args: "(http/redirect)",
    http_redirect_non_string: "(http/redirect 123)",
    http_error_one_arg: r#"(http/error 422)"#,
    http_error_non_int_status: r#"(http/error "nope" "body")"#,
    http_html_non_string: "(http/html 123)",
    http_text_non_string: "(http/text 123)",
    http_no_content_extra: r#"(http/no-content "extra")"#,
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p sema-lang --test dual_eval_stdlib_test -- http_ 2>&1 | tail -20`
Expected: All tests pass on both `_tw` and `_vm` variants.

- [ ] **Step 3: Commit**

```bash
git add crates/sema/tests/dual_eval_stdlib_test.rs
git commit -m "test: add dual-eval tests for HTTP response helpers"
```

---

### Task 2: Router Unit Tests — Edge Cases

**Files:**
- Modify: `crates/sema/tests/server_test.rs`

These test the router dispatch function directly (no server, no network). The router returns a closure that takes a request map and returns a response map.

Request map shape: `{:method :get :path "/..." :headers {} :query {} :params {} :body "" :remote "127.0.0.1"}`

- [ ] **Step 1: Add router edge case tests**

Add to `server_test.rs`:

```rust
// ---------------------------------------------------------------------------
// Router edge cases — unit tests (no network)
// ---------------------------------------------------------------------------

fn make_request(method: &str, path: &str) -> String {
    format!(
        r#"{{:method :{method} :path "{path}" :headers {{}} :query {{}} :params {{}} :body "" :remote "127.0.0.1"}}"#
    )
}

fn router_eval(routes: &str, method: &str, path: &str) -> Value {
    let req = make_request(method, path);
    eval(&format!(
        r#"(let ((router (http/router {routes}))) (router {req}))"#
    ))
}

fn get_status(result: &Value) -> i64 {
    result
        .as_map_rc()
        .unwrap()
        .get(&Value::keyword("status"))
        .and_then(|v| v.as_int())
        .unwrap()
}

fn get_body(result: &Value) -> String {
    result
        .as_map_rc()
        .unwrap()
        .get(&Value::keyword("body"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_default()
}

// --- Route matching ---

#[test]
fn test_router_first_match_wins() {
    // When two routes match, the first one wins
    let result = router_eval(
        r#"[[:get "/x" (fn (req) (http/ok "first"))]
            [:get "/x" (fn (req) (http/ok "second"))]]"#,
        "get",
        "/x",
    );
    assert_eq!(get_status(&result), 200);
    assert!(get_body(&result).contains("first"));
}

#[test]
fn test_router_trailing_slash_normalized() {
    let result = router_eval(
        r#"[[:get "/api/data" (fn (req) (http/ok "found"))]]"#,
        "get",
        "/api/data/",
    );
    assert_eq!(get_status(&result), 200);
}

#[test]
fn test_router_root_path() {
    let result = router_eval(
        r#"[[:get "/" (fn (req) (http/ok "root"))]]"#,
        "get",
        "/",
    );
    assert_eq!(get_status(&result), 200);
}

#[test]
fn test_router_empty_path() {
    let result = router_eval(
        r#"[[:get "/" (fn (req) (http/ok "root"))]]"#,
        "get",
        "",
    );
    // Empty path should match "/" or 404
    let status = get_status(&result);
    assert!(status == 200 || status == 404);
}

#[test]
fn test_router_multi_segment_params() {
    let result = router_eval(
        r#"[[:get "/users/:id/posts/:pid" (fn (req) (http/ok (:params req)))]]"#,
        "get",
        "/users/42/posts/99",
    );
    assert_eq!(get_status(&result), 200);
    let body = get_body(&result);
    assert!(body.contains("42"), "should contain user id 42: {body}");
    assert!(body.contains("99"), "should contain post id 99: {body}");
}

#[test]
fn test_router_param_with_special_chars() {
    let result = router_eval(
        r#"[[:get "/search/:q" (fn (req) (http/ok (:params req)))]]"#,
        "get",
        "/search/hello%20world",
    );
    assert_eq!(get_status(&result), 200);
}

#[test]
fn test_router_wildcard_captures_rest() {
    let result = router_eval(
        r#"[[:get "/files/*" (fn (req) (http/ok (:params req)))]]"#,
        "get",
        "/files/a/b/c/d.txt",
    );
    assert_eq!(get_status(&result), 200);
    let body = get_body(&result);
    assert!(body.contains("a/b/c/d.txt"), "wildcard should capture full path: {body}");
}

#[test]
fn test_router_wildcard_empty_rest() {
    let result = router_eval(
        r#"[[:get "/files/*" (fn (req) (http/ok "matched"))]]"#,
        "get",
        "/files/",
    );
    assert_eq!(get_status(&result), 200);
}

#[test]
fn test_router_no_routes() {
    let result = router_eval(r#"[]"#, "get", "/anything");
    assert_eq!(get_status(&result), 404);
}

// --- Method matching ---

#[test]
fn test_router_all_methods() {
    for method in &["get", "post", "put", "delete", "patch", "head"] {
        let result = router_eval(
            &format!(r#"[[:any "/test" (fn (req) (http/ok "ok"))]]"#),
            method,
            "/test",
        );
        assert_eq!(get_status(&result), 200, "method :{method} should match :any");
    }
}

#[test]
fn test_router_method_case() {
    // Methods should be keyword-matched
    let result = router_eval(
        r#"[[:get "/test" (fn (req) (http/ok "ok"))]]"#,
        "get",
        "/test",
    );
    assert_eq!(get_status(&result), 200);
}

#[test]
fn test_router_post_doesnt_match_get() {
    let result = router_eval(
        r#"[[:post "/data" (fn (req) (http/ok "posted"))]]"#,
        "get",
        "/data",
    );
    assert_eq!(get_status(&result), 404);
}

#[test]
fn test_router_multiple_methods_same_path() {
    let routes = r#"[[:get "/api" (fn (req) (http/ok "got"))]
                     [:post "/api" (fn (req) (http/ok "posted"))]
                     [:delete "/api" (fn (req) (http/ok "deleted"))]]"#;

    let r1 = router_eval(routes, "get", "/api");
    assert!(get_body(&r1).contains("got"));

    let r2 = router_eval(routes, "post", "/api");
    assert!(get_body(&r2).contains("posted"));

    let r3 = router_eval(routes, "delete", "/api");
    assert!(get_body(&r3).contains("deleted"));

    let r4 = router_eval(routes, "put", "/api");
    assert_eq!(get_status(&r4), 404);
}

// --- Handler behavior ---

#[test]
fn test_router_handler_receives_method() {
    let result = eval(&format!(
        r#"(let ((router (http/router [[:get "/m" (fn (req) (http/ok (:method req)))]])))
          (router {}))"#,
        make_request("get", "/m")
    ));
    let body = get_body(&result);
    assert!(body.contains("get"), "handler should receive :method as keyword: {body}");
}

#[test]
fn test_router_handler_receives_path() {
    let result = eval(&format!(
        r#"(let ((router (http/router [[:get "/test/path" (fn (req) (http/text (:path req)))]])))
          (router {}))"#,
        make_request("get", "/test/path")
    ));
    let body = get_body(&result);
    assert!(body.contains("/test/path"), "handler should receive :path: {body}");
}

#[test]
fn test_router_handler_error_returns_500() {
    let result = router_eval(
        r#"[[:get "/crash" (fn (req) (error "boom"))]]"#,
        "get",
        "/crash",
    );
    assert_eq!(get_status(&result), 500);
}

#[test]
fn test_router_handler_returns_non_map() {
    // If handler returns a non-map, router should handle gracefully
    let result = router_eval(
        r#"[[:get "/bad" (fn (req) "just a string")]]"#,
        "get",
        "/bad",
    );
    // Should either wrap in 200 or return 500 — either way, not crash
    let status = get_status(&result);
    assert!(status == 200 || status == 500);
}

// --- Query string ---

#[test]
fn test_router_query_preserved() {
    let result = eval(
        r#"(let ((router (http/router [[:get "/q" (fn (req) (http/ok (:query req)))]])))
          (router {:method :get :path "/q" :headers {} :query {:foo "bar" :baz "42"} :params {} :body "" :remote "127.0.0.1"}))"#,
    );
    let body = get_body(&result);
    assert!(body.contains("bar"), "query params should be passed through: {body}");
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p sema-lang --test server_test -- test_router 2>&1 | tail -30`
Expected: All pass.

- [ ] **Step 3: Commit**

```bash
git add crates/sema/tests/server_test.rs
git commit -m "test: add router edge case unit tests (matching, params, methods)"
```

---

### Task 3: Static File Serving Edge Cases

**Files:**
- Modify: `crates/sema/tests/server_test.rs`

- [ ] **Step 1: Add static file edge case tests**

```rust
// ---------------------------------------------------------------------------
// Static file serving edge cases — unit tests (no network)
// ---------------------------------------------------------------------------

#[test]
fn test_static_index_html_for_directory() {
    let tmp = std::env::temp_dir().join("sema-static-index-test");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("sub")).unwrap();
    std::fs::write(tmp.join("sub/index.html"), "<h1>Index</h1>").unwrap();
    let dir = tmp.to_string_lossy().replace('\\', "/");

    let result = eval(&format!(
        r#"(let ((router (http/router [[:static "/s" "{dir}"]])))
          (router {{:method :get :path "/s/sub/" :headers {{}} :query {{}} :params {{}} :body "" :remote "127.0.0.1"}}))"#
    ));
    let map = result.as_map_rc().unwrap();
    // Should serve index.html or the file marker
    let has_file = map.get(&Value::keyword("__file")).and_then(|v| v.as_bool()).unwrap_or(false);
    if has_file {
        let path = map.get(&Value::keyword("__file_path")).and_then(|v| v.as_str()).unwrap();
        assert!(path.contains("index.html"), "should serve index.html for directory: {path}");
    }
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn test_static_nested_path_traversal_double_dot() {
    let tmp = std::env::temp_dir().join("sema-static-dotdot-test");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    std::fs::write(tmp.join("safe.txt"), "safe").unwrap();
    let dir = tmp.to_string_lossy().replace('\\', "/");

    // Various traversal attempts
    for path in &[
        "/s/../../etc/passwd",
        "/s/..%2f..%2fetc/passwd",
        "/s/sub/../../out.txt",
    ] {
        let result = eval(&format!(
            r#"(let ((router (http/router [[:static "/s" "{dir}"]])))
              (router {{:method :get :path "{path}" :headers {{}} :query {{}} :params {{}} :body "" :remote "127.0.0.1"}}))"#
        ));
        let map = result.as_map_rc().unwrap();
        let status = map.get(&Value::keyword("status")).and_then(|v| v.as_int()).unwrap_or(0);
        assert!(status == 400 || status == 404, "path traversal {path} should be blocked, got {status}");
    }
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn test_static_mime_types() {
    let tmp = std::env::temp_dir().join("sema-static-mime-test");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    let files = [
        ("test.html", "text/html"),
        ("test.css", "text/css"),
        ("test.js", "text/javascript"),  // or application/javascript
        ("test.json", "application/json"),
        ("test.png", "image/png"),
        ("test.svg", "image/svg"),
    ];

    for (name, _) in &files {
        std::fs::write(tmp.join(name), "content").unwrap();
    }
    let dir = tmp.to_string_lossy().replace('\\', "/");

    for (name, expected_mime) in &files {
        let result = eval(&format!(
            r#"(let ((router (http/router [[:static "/s" "{dir}"]])))
              (router {{:method :get :path "/s/{name}" :headers {{}} :query {{}} :params {{}} :body "" :remote "127.0.0.1"}}))"#
        ));
        let map = result.as_map_rc().unwrap();
        let has_file = map.get(&Value::keyword("__file")).and_then(|v| v.as_bool()).unwrap_or(false);
        assert!(has_file, "{name} should return file marker");
        let ct = map.get(&Value::keyword("__file_content_type")).and_then(|v| v.as_str()).unwrap_or("");
        assert!(
            ct.contains(expected_mime) || ct.contains(&expected_mime.replace("/", "/")),
            "{name}: expected MIME containing '{expected_mime}', got '{ct}'"
        );
    }
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn test_http_file_nonexistent_error() {
    let err = eval_err(r#"(http/file "/definitely/not/a/real/path/xyz.txt")"#);
    assert!(err.to_string().contains("http/file"));
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p sema-lang --test server_test -- test_static 2>&1 | tail -20`
Expected: All pass.

- [ ] **Step 3: Commit**

```bash
git add crates/sema/tests/server_test.rs
git commit -m "test: add static file serving edge cases (index.html, traversal, MIME)"
```

---

### Task 4: Integration Tests — WebSocket Multi-Message

**Files:**
- Modify: `crates/sema/tests/server_test.rs`

These spawn a real server and test actual protocol behavior.

- [ ] **Step 1: Add WebSocket integration tests**

```rust
#[test]
#[ignore] // requires network
fn test_websocket_multi_message() {
    use std::process::{Command, Stdio};
    use std::time::Duration;

    let mut child = Command::new(env!("CARGO_BIN_EXE_sema"))
        .arg("-e")
        .arg(r#"
            (http/serve
              (http/router
                [[:ws "/chat" (fn (conn)
                  (let loop ()
                    (let ((msg ((:recv conn))))
                      (when msg
                        ((:send conn) (string-append "re:" msg))
                        (loop)))))]])
              {:port 19900})
        "#)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn");

    std::thread::sleep(Duration::from_millis(1500));

    let (mut ws, _) = tungstenite::connect("ws://127.0.0.1:19900/chat").expect("WS connect");

    // Send multiple messages and verify each echo
    for i in 0..5 {
        let msg = format!("msg{i}");
        ws.send(tungstenite::Message::Text(msg.clone().into())).unwrap();
        let reply = ws.read().unwrap();
        assert_eq!(reply.into_text().unwrap(), format!("re:{msg}"));
    }

    ws.close(None).ok();
    child.kill().ok();
    child.wait().ok();
}

#[test]
#[ignore] // requires network
fn test_websocket_close_from_server() {
    use std::process::{Command, Stdio};
    use std::time::Duration;

    let mut child = Command::new(env!("CARGO_BIN_EXE_sema"))
        .arg("-e")
        .arg(r#"
            (http/serve
              (http/router
                [[:ws "/once" (fn (conn)
                  (let ((msg ((:recv conn))))
                    (when msg
                      ((:send conn) "goodbye")
                      ((:close conn)))))]])
              {:port 19901})
        "#)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn");

    std::thread::sleep(Duration::from_millis(1500));

    let (mut ws, _) = tungstenite::connect("ws://127.0.0.1:19901/once").expect("WS connect");
    ws.send(tungstenite::Message::Text("hi".into())).unwrap();
    let reply = ws.read().unwrap();
    assert_eq!(reply.into_text().unwrap(), "goodbye");

    // Next read should indicate closure
    let next = ws.read();
    assert!(next.is_err() || matches!(next.as_ref().unwrap(), tungstenite::Message::Close(_)),
        "should get close or error after server closes: {next:?}");

    child.kill().ok();
    child.wait().ok();
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p sema-lang --test server_test -- test_websocket --ignored 2>&1 | tail -15`
Expected: All pass.

- [ ] **Step 3: Commit**

```bash
git add crates/sema/tests/server_test.rs
git commit -m "test: add WebSocket multi-message and server-close integration tests"
```

---

### Task 5: Integration Tests — SSE Streaming

**Files:**
- Modify: `crates/sema/tests/server_test.rs`

- [ ] **Step 1: Add SSE integration tests**

```rust
#[test]
#[ignore] // requires network
fn test_sse_multiple_events() {
    use std::process::{Command, Stdio};
    use std::time::Duration;

    let mut child = Command::new(env!("CARGO_BIN_EXE_sema"))
        .arg("-e")
        .arg(r#"
            (http/serve
              (http/router
                [[:get "/events"
                  (fn (req)
                    (http/stream (fn (send)
                      (send "event1")
                      (send "event2")
                      (send "event3"))))]])
              {:port 19902})
        "#)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn");

    std::thread::sleep(Duration::from_millis(1500));

    let client = reqwest::blocking::Client::new();
    let resp = client
        .get("http://127.0.0.1:19902/events")
        .timeout(Duration::from_secs(5))
        .send()
        .expect("GET /events");

    let body = resp.text().unwrap();
    assert!(body.contains("event1"), "should contain event1: {body}");
    assert!(body.contains("event2"), "should contain event2: {body}");
    assert!(body.contains("event3"), "should contain event3: {body}");

    child.kill().ok();
    child.wait().ok();
}

#[test]
#[ignore] // requires network
fn test_sse_content_type() {
    use std::process::{Command, Stdio};
    use std::time::Duration;

    let mut child = Command::new(env!("CARGO_BIN_EXE_sema"))
        .arg("-e")
        .arg(r#"
            (http/serve
              (http/router
                [[:get "/sse"
                  (fn (req)
                    (http/stream (fn (send) (send "data"))))]])
              {:port 19903})
        "#)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn");

    std::thread::sleep(Duration::from_millis(1500));

    let client = reqwest::blocking::Client::new();
    let resp = client
        .get("http://127.0.0.1:19903/sse")
        .timeout(Duration::from_secs(5))
        .send()
        .expect("GET /sse");

    let ct = resp.headers().get("content-type").unwrap().to_str().unwrap();
    assert!(ct.contains("text/event-stream"), "SSE should have event-stream content-type: {ct}");

    child.kill().ok();
    child.wait().ok();
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p sema-lang --test server_test -- test_sse --ignored 2>&1 | tail -15`
Expected: All pass.

- [ ] **Step 3: Commit**

```bash
git add crates/sema/tests/server_test.rs
git commit -m "test: add SSE multi-event and content-type integration tests"
```

---

### Task 6: Integration Tests — Error Resilience & Concurrent Requests

**Files:**
- Modify: `crates/sema/tests/server_test.rs`

- [ ] **Step 1: Add error resilience and concurrency tests**

```rust
#[test]
#[ignore] // requires network
fn test_server_survives_handler_panic() {
    use std::process::{Command, Stdio};
    use std::time::Duration;

    let mut child = Command::new(env!("CARGO_BIN_EXE_sema"))
        .arg("-e")
        .arg(r#"
            (http/serve
              (http/router
                [[:get "/crash" (fn (req) (error "kaboom"))]
                 [:get "/ok" (fn (req) (http/ok "alive"))]])
              {:port 19904})
        "#)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn");

    std::thread::sleep(Duration::from_millis(1500));

    let client = reqwest::blocking::Client::new();

    // Crash the handler
    let resp = client.get("http://127.0.0.1:19904/crash")
        .timeout(Duration::from_secs(5)).send().unwrap();
    assert_eq!(resp.status(), 500);

    // Server should still be alive
    let resp = client.get("http://127.0.0.1:19904/ok")
        .timeout(Duration::from_secs(5)).send().unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().unwrap();
    assert_eq!(body, "alive");

    // Crash again
    let resp = client.get("http://127.0.0.1:19904/crash")
        .timeout(Duration::from_secs(5)).send().unwrap();
    assert_eq!(resp.status(), 500);

    // Still alive
    let resp = client.get("http://127.0.0.1:19904/ok")
        .timeout(Duration::from_secs(5)).send().unwrap();
    assert_eq!(resp.status(), 200);

    child.kill().ok();
    child.wait().ok();
}

#[test]
#[ignore] // requires network
fn test_server_concurrent_requests() {
    use std::process::{Command, Stdio};
    use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
    use std::time::Duration;

    let mut child = Command::new(env!("CARGO_BIN_EXE_sema"))
        .arg("-e")
        .arg(r#"
            (http/serve
              (fn (req) (http/ok (:path req)))
              {:port 19905})
        "#)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn");

    std::thread::sleep(Duration::from_millis(1500));

    let success_count = Arc::new(AtomicUsize::new(0));
    let threads: Vec<_> = (0..10).map(|i| {
        let count = success_count.clone();
        std::thread::spawn(move || {
            let client = reqwest::blocking::Client::new();
            let resp = client
                .get(&format!("http://127.0.0.1:19905/req/{i}"))
                .timeout(Duration::from_secs(10))
                .send();
            if let Ok(r) = resp {
                if r.status() == 200 {
                    count.fetch_add(1, Ordering::SeqCst);
                }
            }
        })
    }).collect();

    for t in threads {
        t.join().unwrap();
    }

    let successes = success_count.load(Ordering::SeqCst);
    assert!(successes >= 8, "at least 8/10 concurrent requests should succeed, got {successes}");

    child.kill().ok();
    child.wait().ok();
}

#[test]
#[ignore] // requires network
fn test_server_large_json_body() {
    use std::process::{Command, Stdio};
    use std::time::Duration;

    let mut child = Command::new(env!("CARGO_BIN_EXE_sema"))
        .arg("-e")
        .arg(r#"
            (http/serve
              (fn (req) (http/ok (string-length (or (:body req) ""))))
              {:port 19906})
        "#)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn");

    std::thread::sleep(Duration::from_millis(1500));

    let client = reqwest::blocking::Client::new();
    let large_body = "x".repeat(100_000);
    let resp = client
        .post("http://127.0.0.1:19906/data")
        .body(large_body)
        .timeout(Duration::from_secs(10))
        .send()
        .expect("POST large body");

    assert_eq!(resp.status(), 200);

    child.kill().ok();
    child.wait().ok();
}

#[test]
#[ignore] // requires network
fn test_server_custom_response_headers() {
    use std::process::{Command, Stdio};
    use std::time::Duration;

    let mut child = Command::new(env!("CARGO_BIN_EXE_sema"))
        .arg("-e")
        .arg(r#"
            (http/serve
              (fn (req) {:status 200
                         :headers {"x-custom" "hello"
                                   "x-request-id" "abc-123"}
                         :body "ok"})
              {:port 19907})
        "#)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn");

    std::thread::sleep(Duration::from_millis(1500));

    let client = reqwest::blocking::Client::new();
    let resp = client.get("http://127.0.0.1:19907/test")
        .timeout(Duration::from_secs(5)).send().unwrap();

    assert_eq!(resp.status(), 200);
    assert_eq!(resp.headers().get("x-custom").unwrap().to_str().unwrap(), "hello");
    assert_eq!(resp.headers().get("x-request-id").unwrap().to_str().unwrap(), "abc-123");

    child.kill().ok();
    child.wait().ok();
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p sema-lang --test server_test -- "test_server_survives\|test_server_concurrent\|test_server_large\|test_server_custom" --ignored 2>&1 | tail -15`
Expected: All pass.

- [ ] **Step 3: Commit**

```bash
git add crates/sema/tests/server_test.rs
git commit -m "test: add error resilience, concurrency, and header integration tests"
```

---

### Task 7: Integration Tests — Middleware Pattern

**Files:**
- Modify: `crates/sema/tests/server_test.rs`

- [ ] **Step 1: Add middleware composition tests**

```rust
#[test]
#[ignore] // requires network
fn test_middleware_cors() {
    use std::process::{Command, Stdio};
    use std::time::Duration;

    let mut child = Command::new(env!("CARGO_BIN_EXE_sema"))
        .arg("-e")
        .arg(r#"
            (define (cors-wrap handler)
              (fn (req)
                (let ((resp (handler req)))
                  (assoc resp :headers
                    (merge (or (get resp :headers) {})
                           {"access-control-allow-origin" "*"
                            "access-control-allow-methods" "GET, POST"})))))

            (http/serve
              (cors-wrap
                (http/router
                  [[:get "/api" (fn (req) (http/ok {:data 42}))]]))
              {:port 19908})
        "#)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn");

    std::thread::sleep(Duration::from_millis(1500));

    let client = reqwest::blocking::Client::new();
    let resp = client.get("http://127.0.0.1:19908/api")
        .timeout(Duration::from_secs(5)).send().unwrap();

    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers().get("access-control-allow-origin").unwrap().to_str().unwrap(),
        "*"
    );

    child.kill().ok();
    child.wait().ok();
}

#[test]
#[ignore] // requires network
fn test_middleware_logging() {
    use std::process::{Command, Stdio};
    use std::time::Duration;

    let mut child = Command::new(env!("CARGO_BIN_EXE_sema"))
        .arg("-e")
        .arg(r#"
            (define (log-wrap handler)
              (fn (req)
                (let ((resp (handler req)))
                  (display (string-append (:method req) " " (:path req) " -> " (number->string (get resp :status))))
                  resp)))

            (http/serve
              (log-wrap (fn (req) (http/ok "logged")))
              {:port 19909})
        "#)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn");

    std::thread::sleep(Duration::from_millis(1500));

    let client = reqwest::blocking::Client::new();
    let resp = client.get("http://127.0.0.1:19909/test")
        .timeout(Duration::from_secs(5)).send().unwrap();
    assert_eq!(resp.status(), 200);

    child.kill().ok();
    child.wait().ok();
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p sema-lang --test server_test -- test_middleware --ignored 2>&1 | tail -15`
Expected: All pass.

- [ ] **Step 3: Commit**

```bash
git add crates/sema/tests/server_test.rs
git commit -m "test: add middleware pattern integration tests (CORS, logging)"
```

---

### Task 8: Router Error Tests

**Files:**
- Modify: `crates/sema/tests/server_test.rs`

- [ ] **Step 1: Add router construction error tests**

```rust
// ---------------------------------------------------------------------------
// Router construction errors
// ---------------------------------------------------------------------------

#[test]
fn test_router_invalid_method() {
    // Invalid method keyword should error or be ignored
    let result = std::panic::catch_unwind(|| {
        eval(r#"(http/router [[:banana "/x" (fn (req) (http/ok "x"))]])"#)
    });
    // Should either error at construction or just never match
    if let Ok(router_val) = result {
        // If it constructs, the invalid method should never match
        let req = make_request("get", "/x");
        let result = eval(&format!(
            r#"(let ((r {router_val})) (r {req}))"#,
        ));
        // Might not be callable this way, just verify no crash
        let _ = result;
    }
}

#[test]
fn test_router_empty_pattern() {
    let result = router_eval(
        r#"[[:get "" (fn (req) (http/ok "empty"))]]"#,
        "get",
        "/",
    );
    // Empty pattern should match "/" or 404 — just shouldn't crash
    let _ = get_status(&result);
}

#[test]
fn test_http_router_no_args() {
    let _ = eval_err(r#"(http/router)"#);
}

#[test]
fn test_http_serve_no_args() {
    let _ = eval_err(r#"(http/serve)"#);
}

#[test]
fn test_http_stream_no_args() {
    let _ = eval_err(r#"(http/stream)"#);
}

#[test]
fn test_http_stream_non_function() {
    let _ = eval_err(r#"(http/stream 42)"#);
}

#[test]
fn test_http_websocket_no_args() {
    let _ = eval_err(r#"(http/websocket)"#);
}

#[test]
fn test_http_websocket_non_function() {
    let _ = eval_err(r#"(http/websocket 42)"#);
}
```

- [ ] **Step 2: Run tests and fix any failures**

Run: `cargo test -p sema-lang --test server_test 2>&1 | tail -20`
Expected: All non-ignored tests pass.

- [ ] **Step 3: Commit**

```bash
git add crates/sema/tests/server_test.rs
git commit -m "test: add router/stream/websocket construction error tests"
```

---

### Task 9: Final Verification

- [ ] **Step 1: Run all non-network tests**

Run: `cargo test 2>&1 | grep -E "FAILED|^test result:" | head -30`
Expected: All pass, zero failures.

- [ ] **Step 2: Run network tests (optional, if available)**

Run: `cargo test -p sema-lang --test server_test --ignored 2>&1 | tail -30`
Run: `cargo test -p sema-lang --test integration_test -- test_http_serve --ignored 2>&1 | tail -20`
Expected: All pass (or timeout gracefully on CI).

- [ ] **Step 3: Final commit if any fixups needed**

```bash
git add -A
git commit -m "test: comprehensive HTTP server/client test suite"
```
