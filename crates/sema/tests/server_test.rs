use sema_core::{SemaError, Value};
use sema_eval::Interpreter;

fn eval(input: &str) -> Value {
    let interp = Interpreter::new();
    interp
        .eval_str(input)
        .unwrap_or_else(|_| panic!("failed to eval: {input}"))
}

fn eval_err(input: &str) -> SemaError {
    let interp = Interpreter::new();
    interp.eval_str(input).unwrap_err()
}

// ---------------------------------------------------------------------------
// Web server integration tests (server-based, require network)
// ---------------------------------------------------------------------------

#[test]
#[ignore] // requires network
fn test_http_serve_json_body_parsing() {
    use std::process::{Command, Stdio};
    use std::time::Duration;

    let mut child = Command::new(env!("CARGO_BIN_EXE_sema"))
        .arg("-e")
        .arg(r#"(http/serve (fn (req) (http/ok (or (:json req) "no json"))) {:port 19880})"#)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn sema");

    std::thread::sleep(Duration::from_millis(1500));

    let client = reqwest::blocking::Client::new();

    // POST with JSON body
    let resp = client
        .post("http://127.0.0.1:19880/test")
        .header("content-type", "application/json")
        .body(r#"{"name":"Ada","age":36}"#)
        .timeout(Duration::from_secs(5))
        .send()
        .expect("Failed to POST");
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().expect("Failed to parse JSON response");
    assert_eq!(body["name"], "Ada");
    assert_eq!(body["age"], 36);

    // GET without JSON body should get "no json"
    let resp = client
        .get("http://127.0.0.1:19880/test")
        .timeout(Duration::from_secs(5))
        .send()
        .expect("Failed to GET");
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().expect("Failed to parse JSON response");
    assert_eq!(body, "no json");

    child.kill().ok();
    child.wait().ok();
}

// STD-6: request body is capped (16 MiB) so a large body returns 413 instead of
// being buffered unbounded into memory.
#[test]
#[ignore] // requires network
fn test_http_serve_rejects_oversized_body() {
    use std::process::{Command, Stdio};
    use std::time::Duration;

    let mut child = Command::new(env!("CARGO_BIN_EXE_sema"))
        .arg("-e")
        .arg(r#"(http/serve (fn (req) (http/ok "ok")) {:port 19890})"#)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn sema");

    std::thread::sleep(Duration::from_millis(1500));

    let client = reqwest::blocking::Client::new();

    // A small body is accepted.
    let ok = client
        .post("http://127.0.0.1:19890/x")
        .body("hello")
        .timeout(Duration::from_secs(5))
        .send()
        .expect("small POST failed");
    assert_eq!(ok.status(), 200);

    // A 17 MiB body exceeds the 16 MiB cap → 413.
    let big = vec![b'a'; 17 * 1024 * 1024];
    let resp = client
        .post("http://127.0.0.1:19890/x")
        .body(big)
        .timeout(Duration::from_secs(10))
        .send()
        .expect("large POST failed");
    assert_eq!(resp.status(), 413, "oversized body should be rejected");

    child.kill().ok();
    child.wait().ok();
}

// STD-7: (ws/close) actually closes the socket even while the handler is still
// running. The handler sends a message, closes, then sleeps 3s; a correct close
// drops the sole sender so the client observes closure well before the sleep
// ends. (Before the fix, close dropped only a clone and the socket stayed open
// until the handler returned.)
#[test]
#[ignore] // requires network
fn test_ws_close_closes_socket_while_handler_runs() {
    use std::net::TcpStream;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let mut child = Command::new(env!("CARGO_BIN_EXE_sema"))
        .arg("-e")
        .arg(
            r#"(http/serve
                 (fn (req)
                   (http/websocket
                     (fn (conn)
                       ((:send conn) "hi")
                       ((:close conn))
                       (sleep 3000))))
                 {:port 19891})"#,
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn sema");

    std::thread::sleep(Duration::from_millis(1500));

    let stream = TcpStream::connect("127.0.0.1:19891").expect("tcp connect");
    // Read timeout MUST exceed the handler's 3s sleep, so that on the buggy
    // (no-op close) path the read blocks until the real close at ~3s rather than
    // timing out early and masquerading as a fast close.
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let (mut socket, _resp) =
        tungstenite::client("ws://127.0.0.1:19891/", stream).expect("ws handshake");

    let start = Instant::now();
    let first = socket.read().expect("read first message");
    assert_eq!(first.into_text().unwrap().as_str(), "hi");

    // The next read must observe the close (Close frame or connection error)
    // quickly — long before the handler's 3s sleep finishes.
    let closed = loop {
        match socket.read() {
            Ok(msg) if msg.is_close() => break true,
            Ok(_) => continue,
            Err(_) => break true, // connection dropped == closed
        }
    };
    let elapsed = start.elapsed();
    assert!(closed, "socket should have closed");
    assert!(
        elapsed < Duration::from_millis(2500),
        "close took {elapsed:?} — handler sleep was not bypassed (ws/close ineffective)"
    );

    child.kill().ok();
    child.wait().ok();
}

#[test]
#[ignore] // requires network
fn test_http_serve_query_string() {
    use std::process::{Command, Stdio};
    use std::time::Duration;

    let mut child = Command::new(env!("CARGO_BIN_EXE_sema"))
        .arg("-e")
        .arg(r#"(http/serve (fn (req) (http/ok (:query req))) {:port 19881})"#)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn sema");

    std::thread::sleep(Duration::from_millis(1500));

    let client = reqwest::blocking::Client::new();
    let resp = client
        .get("http://127.0.0.1:19881/search?q=hello&page=2")
        .timeout(Duration::from_secs(5))
        .send()
        .expect("Failed to GET");
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().expect("Failed to parse JSON response");
    assert_eq!(body["page"], "2");
    assert_eq!(body["q"], "hello");

    child.kill().ok();
    child.wait().ok();
}

#[test]
#[ignore] // requires network
fn test_http_serve_handler_error() {
    use std::process::{Command, Stdio};
    use std::time::Duration;

    let mut child = Command::new(env!("CARGO_BIN_EXE_sema"))
        .arg("-e")
        .arg(r#"(http/serve (fn (req) (error "something broke")) {:port 19882})"#)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn sema");

    std::thread::sleep(Duration::from_millis(1500));

    let client = reqwest::blocking::Client::new();

    // First request: handler errors, should get 500
    let resp = client
        .get("http://127.0.0.1:19882/test")
        .timeout(Duration::from_secs(5))
        .send()
        .expect("Failed to GET");
    assert_eq!(resp.status(), 500);

    // Second request: server should still be running
    let resp = client
        .get("http://127.0.0.1:19882/test2")
        .timeout(Duration::from_secs(5))
        .send()
        .expect("Server should still be running after handler error");
    assert_eq!(resp.status(), 500);

    child.kill().ok();
    child.wait().ok();
}

#[test]
#[ignore] // requires network
fn test_http_serve_method_dispatch() {
    use std::process::{Command, Stdio};
    use std::time::Duration;

    let mut child = Command::new(env!("CARGO_BIN_EXE_sema"))
        .arg("-e")
        .arg(
            r#"
            (http/serve
              (http/router
                [[:get "/data" (fn (req) (http/ok "got"))]
                 [:post "/data" (fn (req) (http/ok "posted"))]])
              {:port 19883})
        "#,
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn sema");

    std::thread::sleep(Duration::from_millis(1500));

    let client = reqwest::blocking::Client::new();

    // GET /data
    let resp = client
        .get("http://127.0.0.1:19883/data")
        .timeout(Duration::from_secs(5))
        .send()
        .expect("Failed to GET /data");
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().unwrap();
    assert_eq!(body, "got");

    // POST /data
    let resp = client
        .post("http://127.0.0.1:19883/data")
        .timeout(Duration::from_secs(5))
        .send()
        .expect("Failed to POST /data");
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().unwrap();
    assert_eq!(body, "posted");

    child.kill().ok();
    child.wait().ok();
}

#[test]
#[ignore] // requires network
fn test_http_serve_custom_headers() {
    use std::process::{Command, Stdio};
    use std::time::Duration;

    let mut child = Command::new(env!("CARGO_BIN_EXE_sema"))
        .arg("-e")
        .arg(
            r#"
            (http/serve
              (fn (req)
                {:status 200
                 :headers {"x-custom" "hello" "content-type" "text/plain"}
                 :body "ok"})
              {:port 19884})
        "#,
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn sema");

    std::thread::sleep(Duration::from_millis(1500));

    let client = reqwest::blocking::Client::new();
    let resp = client
        .get("http://127.0.0.1:19884/test")
        .timeout(Duration::from_secs(5))
        .send()
        .expect("Failed to GET");
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers().get("x-custom").map(|v| v.to_str().unwrap()),
        Some("hello")
    );
    assert_eq!(
        resp.headers()
            .get("content-type")
            .map(|v| v.to_str().unwrap()),
        Some("text/plain")
    );
    let body = resp.text().unwrap();
    assert_eq!(body, "ok");

    child.kill().ok();
    child.wait().ok();
}

#[test]
#[ignore] // requires network
fn test_http_serve_wildcard_route() {
    use std::process::{Command, Stdio};
    use std::time::Duration;

    let mut child = Command::new(env!("CARGO_BIN_EXE_sema"))
        .arg("-e")
        .arg(
            r#"
            (http/serve
              (http/router
                [[:get "/files/*" (fn (req) (http/ok (:* (:params req))))]])
              {:port 19885})
        "#,
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn sema");

    std::thread::sleep(Duration::from_millis(1500));

    let client = reqwest::blocking::Client::new();
    let resp = client
        .get("http://127.0.0.1:19885/files/a/b/c.txt")
        .timeout(Duration::from_secs(5))
        .send()
        .expect("Failed to GET /files/a/b/c.txt");
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().unwrap();
    assert_eq!(body, "a/b/c.txt");

    child.kill().ok();
    child.wait().ok();
}

#[test]
#[ignore] // requires network
fn test_http_serve_concurrent_requests() {
    use std::process::{Command, Stdio};
    use std::time::Duration;

    let mut child = Command::new(env!("CARGO_BIN_EXE_sema"))
        .arg("-e")
        .arg(r#"(http/serve (fn (req) (http/ok {:path (:path req)})) {:port 19886})"#)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn sema");

    std::thread::sleep(Duration::from_millis(1500));

    // Spawn 5 threads, each making a request
    let handles: Vec<_> = (0..5)
        .map(|i| {
            std::thread::spawn(move || {
                let client = reqwest::blocking::Client::new();
                let resp = client
                    .get(format!("http://127.0.0.1:19886/item/{i}"))
                    .timeout(Duration::from_secs(10))
                    .send()
                    .expect("Failed to GET");
                assert_eq!(resp.status(), 200);
                let body: serde_json::Value = resp.json().unwrap();
                assert_eq!(body["path"], format!("/item/{i}"));
            })
        })
        .collect();

    for h in handles {
        h.join().expect("Thread panicked");
    }

    child.kill().ok();
    child.wait().ok();
}

#[test]
#[ignore] // requires network
fn test_http_serve_middleware() {
    use std::process::{Command, Stdio};
    use std::time::Duration;

    let mut child = Command::new(env!("CARGO_BIN_EXE_sema"))
        .arg("-e")
        .arg(
            r#"
            (begin
              (define (add-header handler)
                (fn (req)
                  (let ((resp (handler req)))
                    (let ((headers (or (:headers resp) {})))
                      (assoc resp :headers (assoc headers "x-middleware" "applied"))))))
              (http/serve
                (add-header (fn (req) (http/ok "hello")))
                {:port 19887}))
        "#,
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn sema");

    std::thread::sleep(Duration::from_millis(1500));

    let client = reqwest::blocking::Client::new();
    let resp = client
        .get("http://127.0.0.1:19887/test")
        .timeout(Duration::from_secs(5))
        .send()
        .expect("Failed to GET");
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers()
            .get("x-middleware")
            .map(|v| v.to_str().unwrap()),
        Some("applied")
    );

    child.kill().ok();
    child.wait().ok();
}

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
    let result = router_eval(r#"[[:get "/" (fn (req) (http/ok "root"))]]"#, "get", "/");
    assert_eq!(get_status(&result), 200);
}

#[test]
fn test_router_empty_path() {
    let result = router_eval(r#"[[:get "/" (fn (req) (http/ok "root"))]]"#, "get", "");
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
    assert!(
        body.contains("a/b/c/d.txt"),
        "wildcard should capture full path: {body}"
    );
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
            r#"[[:any "/test" (fn (req) (http/ok "ok"))]]"#,
            method,
            "/test",
        );
        assert_eq!(
            get_status(&result),
            200,
            "method :{method} should match :any"
        );
    }
}

#[test]
fn test_router_method_case() {
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
    let req = make_request("get", "/m");
    let result = eval(&format!(
        r#"(let ((router (http/router [[:get "/m" (fn (req) (http/ok (:method req)))]])))
          (router {req}))"#
    ));
    let body = get_body(&result);
    assert!(
        body.contains("get"),
        "handler should receive :method as keyword: {body}"
    );
}

#[test]
fn test_router_handler_receives_path() {
    let req = make_request("get", "/test/path");
    let result = eval(&format!(
        r#"(let ((router (http/router [[:get "/test/path" (fn (req) (http/text (:path req)))]])))
          (router {req}))"#
    ));
    let body = get_body(&result);
    assert!(
        body.contains("/test/path"),
        "handler should receive :path: {body}"
    );
}

#[test]
fn test_router_handler_error_returns_err() {
    // When a handler calls (error ...), the router propagates the error
    let _err = eval_err(&format!(
        r#"(let ((router (http/router [[:get "/crash" (fn (req) (error "boom"))]])))
              (router {}))"#,
        make_request("get", "/crash")
    ));
}

#[test]
fn test_router_handler_returns_non_map() {
    // If handler returns a non-map, the router returns it as-is (no wrapping)
    let req = make_request("get", "/bad");
    let result = eval(&format!(
        r#"(let ((router (http/router [[:get "/bad" (fn (req) "just a string")]])))
          (router {req}))"#
    ));
    // The handler returns a plain string, which is what we get back
    assert!(result.as_str().is_some() || result.as_map_rc().is_some());
}

// --- Query string ---

#[test]
fn test_router_query_preserved() {
    let result = eval(
        r#"(let ((router (http/router [[:get "/q" (fn (req) (http/ok (:query req)))]])))
          (router {:method :get :path "/q" :headers {} :query {:foo "bar" :baz "42"} :params {} :body "" :remote "127.0.0.1"}))"#,
    );
    let body = get_body(&result);
    assert!(
        body.contains("bar"),
        "query params should be passed through: {body}"
    );
}

// ---------------------------------------------------------------------------
// Router unit tests (no server needed) — original tests
// ---------------------------------------------------------------------------

#[test]
fn test_http_router_multiple_methods_same_path() {
    let get_result = eval(
        r#"
        (let ((router (http/router
                       [[:get "/data" (fn (req) (http/ok "got"))]
                        [:post "/data" (fn (req) (http/ok "posted"))]])))
          (router {:method :get :path "/data" :headers {} :query {} :params {} :body "" :remote "127.0.0.1"}))
    "#,
    );
    let map = get_result.as_map_rc().unwrap();
    let body = map.get(&Value::keyword("body")).unwrap();
    assert!(
        body.as_str().unwrap().contains("got"),
        "GET should return 'got', got: {}",
        body
    );

    let post_result = eval(
        r#"
        (let ((router (http/router
                       [[:get "/data" (fn (req) (http/ok "got"))]
                        [:post "/data" (fn (req) (http/ok "posted"))]])))
          (router {:method :post :path "/data" :headers {} :query {} :params {} :body "" :remote "127.0.0.1"}))
    "#,
    );
    let map = post_result.as_map_rc().unwrap();
    let body = map.get(&Value::keyword("body")).unwrap();
    assert!(
        body.as_str().unwrap().contains("posted"),
        "POST should return 'posted', got: {}",
        body
    );
}

#[test]
fn test_http_router_wildcard() {
    let result = eval(
        r#"
        (let ((router (http/router
                       [[:get "/files/*" (fn (req) (http/ok (:params req)))]])))
          (router {:method :get :path "/files/a/b/c.txt" :headers {} :query {} :params {} :body "" :remote "127.0.0.1"}))
    "#,
    );
    let map = result.as_map_rc().unwrap();
    let body = map.get(&Value::keyword("body")).unwrap();
    assert!(
        body.as_str().unwrap().contains("a/b/c.txt"),
        "wildcard should capture 'a/b/c.txt', got: {}",
        body
    );
}

#[test]
fn test_http_response_helpers_arity() {
    // http/ok requires exactly 1 arg
    let _err = eval_err(r#"(http/ok)"#);
    let _err = eval_err(r#"(http/ok 1 2)"#);
    // http/created requires exactly 1 arg
    let _err = eval_err(r#"(http/created)"#);
    // http/not-found requires exactly 1 arg
    let _err = eval_err(r#"(http/not-found)"#);
    // http/redirect requires exactly 1 string arg
    let _err = eval_err(r#"(http/redirect)"#);
    let _err = eval_err(r#"(http/redirect 123)"#);
    // http/error requires 2 args: integer status + body
    let _err = eval_err(r#"(http/error 422)"#);
    let _err = eval_err(r#"(http/error "not-a-number" "body")"#);
    // http/html requires 1 string arg
    let _err = eval_err(r#"(http/html 123)"#);
    // http/text requires 1 string arg
    let _err = eval_err(r#"(http/text 123)"#);
    // http/no-content requires 0 args
    let _err = eval_err(r#"(http/no-content "extra")"#);
    // http/file requires 1-2 args
    let _err = eval_err(r#"(http/file)"#);
    let _err = eval_err(r#"(http/file "a" "b" "c")"#);
    let _err = eval_err(r#"(http/file 123)"#);
}

#[test]
fn test_http_file_returns_marker() {
    // http/file on an existing file returns a map with __file marker
    let result = eval(r#"(http/file "Cargo.toml")"#);
    let map = result.as_map_rc().unwrap();
    assert_eq!(
        map.get(&Value::keyword("__file")).and_then(|v| v.as_bool()),
        Some(true)
    );
    assert!(map
        .get(&Value::keyword("__file_path"))
        .and_then(|v| v.as_str())
        .unwrap()
        .contains("Cargo.toml"));
    assert_eq!(
        map.get(&Value::keyword("__file_content_type"))
            .and_then(|v| v.as_str()),
        Some("text/x-toml")
    );
}

#[test]
fn test_http_file_custom_content_type() {
    let result = eval(r#"(http/file "Cargo.toml" "application/json")"#);
    let map = result.as_map_rc().unwrap();
    assert_eq!(
        map.get(&Value::keyword("__file_content_type"))
            .and_then(|v| v.as_str()),
        Some("application/json")
    );
}

#[test]
fn test_http_file_nonexistent() {
    let err = eval_err(r#"(http/file "nonexistent-file-12345.txt")"#);
    let msg = err.to_string();
    assert!(
        msg.contains("http/file"),
        "error should mention http/file: {msg}"
    );
}

#[test]
fn test_http_router_static_route() {
    // Create a temp directory with a test file
    let tmp = std::env::temp_dir().join("sema-static-route-test");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    std::fs::write(tmp.join("hello.txt"), "hello world").unwrap();
    let dir = tmp.to_string_lossy().replace('\\', "/");

    let result = eval(&format!(
        r#"
        (let ((router (http/router
                       [[:static "/assets" "{dir}"]])))
          (router {{:method :get :path "/assets/hello.txt" :headers {{}} :query {{}} :params {{}} :body "" :remote "127.0.0.1"}}))
    "#
    ));
    let map = result.as_map_rc().unwrap();
    assert_eq!(
        map.get(&Value::keyword("__file")).and_then(|v| v.as_bool()),
        Some(true),
        "should return a file marker for existing file"
    );
    assert!(map
        .get(&Value::keyword("__file_path"))
        .and_then(|v| v.as_str())
        .unwrap()
        .contains("hello.txt"));
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn test_http_router_static_fallthrough() {
    let tmp = std::env::temp_dir().join("sema-static-fallthrough-test");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    std::fs::write(tmp.join("exists.txt"), "yes").unwrap();
    let dir = tmp.to_string_lossy().replace('\\', "/");

    let result = eval(&format!(
        r#"
        (let ((router (http/router
                       [[:static "/assets" "{dir}"]
                        [:get "/*" (fn (req) (http/html "<h1>SPA</h1>"))]])))
          (router {{:method :get :path "/assets/nonexistent.xyz" :headers {{}} :query {{}} :params {{}} :body "" :remote "127.0.0.1"}}))
    "#
    ));
    let map = result.as_map_rc().unwrap();
    let body = map
        .get(&Value::keyword("body"))
        .and_then(|v| v.as_str())
        .unwrap();
    assert!(
        body.contains("SPA"),
        "non-existent static file should fall through to SPA route, got: {body}"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn test_http_router_static_path_traversal() {
    let tmp = std::env::temp_dir().join("sema-static-traversal-test");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    std::fs::write(tmp.join("safe.txt"), "safe").unwrap();
    let dir = tmp.to_string_lossy().replace('\\', "/");

    let result = eval(&format!(
        r#"
        (let ((router (http/router
                       [[:static "/assets" "{dir}"]])))
          (router {{:method :get :path "/assets/../etc/passwd" :headers {{}} :query {{}} :params {{}} :body "" :remote "127.0.0.1"}}))
    "#
    ));
    let map = result.as_map_rc().unwrap();
    let status = map
        .get(&Value::keyword("status"))
        .and_then(|v| v.as_int())
        .unwrap();
    assert_eq!(status, 400, "path traversal should return 400");
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn test_http_router_static_post_rejected() {
    let tmp = std::env::temp_dir().join("sema-static-post-test");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    std::fs::write(tmp.join("file.txt"), "content").unwrap();
    let dir = tmp.to_string_lossy().replace('\\', "/");

    let result = eval(&format!(
        r#"
        (let ((router (http/router
                       [[:static "/assets" "{dir}"]])))
          (router {{:method :post :path "/assets/file.txt" :headers {{}} :query {{}} :params {{}} :body "" :remote "127.0.0.1"}}))
    "#
    ));
    let map = result.as_map_rc().unwrap();
    let status = map
        .get(&Value::keyword("status"))
        .and_then(|v| v.as_int())
        .unwrap();
    assert_eq!(status, 404, "POST to static should 404");
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
#[ignore] // requires network
fn test_http_serve_static_files() {
    use std::process::{Command, Stdio};
    use std::time::Duration;

    // Create a temp directory with test files
    let tmp = std::env::temp_dir().join("sema-static-test");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    std::fs::write(tmp.join("index.html"), "<h1>Hello</h1>").unwrap();
    std::fs::write(tmp.join("style.css"), "body { color: red; }").unwrap();
    std::fs::write(tmp.join("app.js"), "console.log('hi');").unwrap();

    let sema_code = format!(
        r#"(http/serve
             (http/router
               [[:static "/static" "{dir}"]
                [:get "/*" (fn (_) (http/file "{dir}/index.html"))]])
             {{:port 19895}})"#,
        dir = tmp.to_string_lossy().replace('\\', "/")
    );

    let mut child = Command::new(env!("CARGO_BIN_EXE_sema"))
        .arg("-e")
        .arg(&sema_code)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn sema");

    std::thread::sleep(Duration::from_millis(1500));

    let client = reqwest::blocking::Client::new();

    // Serve HTML
    let resp = client
        .get("http://127.0.0.1:19895/static/index.html")
        .timeout(Duration::from_secs(5))
        .send()
        .expect("Failed to GET html");
    assert_eq!(resp.status(), 200);
    assert!(resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .contains("text/html"));
    assert_eq!(resp.text().unwrap(), "<h1>Hello</h1>");

    // Serve CSS with correct MIME type
    let resp = client
        .get("http://127.0.0.1:19895/static/style.css")
        .timeout(Duration::from_secs(5))
        .send()
        .expect("Failed to GET css");
    assert_eq!(resp.status(), 200);
    assert!(resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .contains("text/css"));

    // Serve JS with correct MIME type
    let resp = client
        .get("http://127.0.0.1:19895/static/app.js")
        .timeout(Duration::from_secs(5))
        .send()
        .expect("Failed to GET js");
    assert_eq!(resp.status(), 200);
    assert!(resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .contains("javascript"));

    // Non-existent static file falls through to SPA
    let resp = client
        .get("http://127.0.0.1:19895/about")
        .timeout(Duration::from_secs(5))
        .send()
        .expect("Failed to GET spa");
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().unwrap(), "<h1>Hello</h1>");

    // 404 for non-existent static file (no SPA match for /static/ prefix)
    let resp = client
        .get("http://127.0.0.1:19895/static/nonexistent.txt")
        .timeout(Duration::from_secs(5))
        .send()
        .expect("Failed to GET nonexistent");
    // Falls through static, then matches SPA catch-all
    assert_eq!(resp.status(), 200);

    child.kill().ok();
    child.wait().ok();
    let _ = std::fs::remove_dir_all(&tmp);
}

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
    // Should serve index.html via the file marker
    let has_file = map
        .get(&Value::keyword("__file"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if has_file {
        let path = map
            .get(&Value::keyword("__file_path"))
            .and_then(|v| v.as_str())
            .unwrap();
        assert!(
            path.contains("index.html"),
            "should serve index.html for directory: {path}"
        );
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
        let status = map
            .get(&Value::keyword("status"))
            .and_then(|v| v.as_int())
            .unwrap_or(0);
        assert!(
            status == 400 || status == 404,
            "path traversal {path} should be blocked, got {status}"
        );
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
        ("test.js", "javascript"),
        ("test.json", "application/json"),
        ("test.png", "image/png"),
        ("test.svg", "svg"),
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
        let has_file = map
            .get(&Value::keyword("__file"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        assert!(has_file, "{name} should return file marker");
        let ct = map
            .get(&Value::keyword("__file_content_type"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert!(
            ct.contains(expected_mime),
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

// ---------------------------------------------------------------------------
// Router/stream/websocket construction errors
// ---------------------------------------------------------------------------

#[test]
fn test_router_invalid_method() {
    // Invalid method keyword: router constructs fine, but the method never matches
    let result = router_eval(r#"[[:banana "/x" (fn (req) (http/ok "x"))]]"#, "get", "/x");
    assert_eq!(get_status(&result), 404);
}

#[test]
fn test_router_empty_pattern() {
    let result = router_eval(r#"[[:get "" (fn (req) (http/ok "empty"))]]"#, "get", "/");
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
fn test_http_websocket_no_args() {
    let _ = eval_err(r#"(http/websocket)"#);
}

#[test]
fn test_http_stream_non_function() {
    let _ = eval_err(r#"(http/stream 42)"#);
}

#[test]
fn test_http_websocket_non_function() {
    let _ = eval_err(r#"(http/websocket 42)"#);
}

// ---------------------------------------------------------------------------
// WebSocket multi-message integration tests (require network)
// ---------------------------------------------------------------------------

#[test]
#[ignore] // requires network
fn test_websocket_multi_message() {
    use std::process::{Command, Stdio};
    use std::time::Duration;

    let mut child = Command::new(env!("CARGO_BIN_EXE_sema"))
        .arg("-e")
        .arg(
            r#"
            (http/serve
              (http/router
                [[:ws "/chat" (fn (conn)
                  (let loop ()
                    (let ((msg ((:recv conn))))
                      (when msg
                        ((:send conn) (string-append "re:" msg))
                        (loop)))))]])
              {:port 19900})
        "#,
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn");

    std::thread::sleep(Duration::from_millis(1500));

    let (mut ws, _) = tungstenite::connect("ws://127.0.0.1:19900/chat").expect("WS connect");

    // Send multiple messages and verify each echo
    for i in 0..5 {
        let msg = format!("msg{i}");
        ws.send(tungstenite::Message::Text(msg.clone().into()))
            .unwrap();
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
        .arg(
            r#"
            (http/serve
              (http/router
                [[:ws "/once" (fn (conn)
                  (let ((msg ((:recv conn))))
                    (when msg
                      ((:send conn) "goodbye")
                      ((:close conn)))))]])
              {:port 19901})
        "#,
        )
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
    assert!(
        next.is_err() || matches!(next.as_ref().unwrap(), tungstenite::Message::Close(_)),
        "should get close or error after server closes: {next:?}"
    );

    child.kill().ok();
    child.wait().ok();
}

// ---------------------------------------------------------------------------
// SSE streaming integration tests (require network)
// ---------------------------------------------------------------------------

#[test]
#[ignore] // requires network
fn test_sse_multiple_events() {
    use std::process::{Command, Stdio};
    use std::time::Duration;

    let mut child = Command::new(env!("CARGO_BIN_EXE_sema"))
        .arg("-e")
        .arg(
            r#"
            (http/serve
              (http/router
                [[:get "/events"
                  (fn (req)
                    (http/stream (fn (send)
                      (send "event1")
                      (send "event2")
                      (send "event3"))))]])
              {:port 19902})
        "#,
        )
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
        .arg(
            r#"
            (http/serve
              (http/router
                [[:get "/sse"
                  (fn (req)
                    (http/stream (fn (send) (send "data"))))]])
              {:port 19903})
        "#,
        )
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

    let ct = resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(
        ct.contains("text/event-stream"),
        "SSE should have event-stream content-type: {ct}"
    );

    child.kill().ok();
    child.wait().ok();
}

// ---------------------------------------------------------------------------
// Error resilience & concurrency integration tests (require network)
// ---------------------------------------------------------------------------

#[test]
#[ignore] // requires network
fn test_server_survives_handler_panic() {
    use std::process::{Command, Stdio};
    use std::time::Duration;

    let mut child = Command::new(env!("CARGO_BIN_EXE_sema"))
        .arg("-e")
        .arg(
            r#"
            (http/serve
              (http/router
                [[:get "/crash" (fn (req) (error "kaboom"))]
                 [:get "/ok" (fn (req) (http/ok "alive"))]])
              {:port 19904})
        "#,
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn");

    std::thread::sleep(Duration::from_millis(1500));

    let client = reqwest::blocking::Client::new();

    // Crash the handler
    let resp = client
        .get("http://127.0.0.1:19904/crash")
        .timeout(Duration::from_secs(5))
        .send()
        .unwrap();
    assert_eq!(resp.status(), 500);

    // Server should still be alive
    let resp = client
        .get("http://127.0.0.1:19904/ok")
        .timeout(Duration::from_secs(5))
        .send()
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().unwrap();
    assert_eq!(body, "alive");

    // Crash again
    let resp = client
        .get("http://127.0.0.1:19904/crash")
        .timeout(Duration::from_secs(5))
        .send()
        .unwrap();
    assert_eq!(resp.status(), 500);

    // Still alive
    let resp = client
        .get("http://127.0.0.1:19904/ok")
        .timeout(Duration::from_secs(5))
        .send()
        .unwrap();
    assert_eq!(resp.status(), 200);

    child.kill().ok();
    child.wait().ok();
}

#[test]
#[ignore] // requires network
fn test_server_concurrent_requests() {
    use std::process::{Command, Stdio};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    use std::time::Duration;

    let mut child = Command::new(env!("CARGO_BIN_EXE_sema"))
        .arg("-e")
        .arg(
            r#"
            (http/serve
              (fn (req) (http/ok (:path req)))
              {:port 19905})
        "#,
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn");

    std::thread::sleep(Duration::from_millis(1500));

    let success_count = Arc::new(AtomicUsize::new(0));
    let threads: Vec<_> = (0..10)
        .map(|i| {
            let count = success_count.clone();
            std::thread::spawn(move || {
                let client = reqwest::blocking::Client::new();
                let resp = client
                    .get(format!("http://127.0.0.1:19905/req/{i}"))
                    .timeout(Duration::from_secs(10))
                    .send();
                if let Ok(r) = resp {
                    if r.status() == 200 {
                        count.fetch_add(1, Ordering::SeqCst);
                    }
                }
            })
        })
        .collect();

    for t in threads {
        t.join().unwrap();
    }

    let successes = success_count.load(Ordering::SeqCst);
    assert!(
        successes >= 8,
        "at least 8/10 concurrent requests should succeed, got {successes}"
    );

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
        .arg(
            r#"
            (http/serve
              (fn (req) (http/ok (string-length (or (:body req) ""))))
              {:port 19906})
        "#,
        )
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
        .arg(
            r#"
            (http/serve
              (fn (req) {:status 200
                         :headers {"x-custom" "hello"
                                   "x-request-id" "abc-123"}
                         :body "ok"})
              {:port 19907})
        "#,
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn");

    std::thread::sleep(Duration::from_millis(1500));

    let client = reqwest::blocking::Client::new();
    let resp = client
        .get("http://127.0.0.1:19907/test")
        .timeout(Duration::from_secs(5))
        .send()
        .unwrap();

    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers().get("x-custom").unwrap().to_str().unwrap(),
        "hello"
    );
    assert_eq!(
        resp.headers()
            .get("x-request-id")
            .unwrap()
            .to_str()
            .unwrap(),
        "abc-123"
    );

    child.kill().ok();
    child.wait().ok();
}

// ---------------------------------------------------------------------------
// Middleware pattern integration tests (require network)
// ---------------------------------------------------------------------------

#[test]
#[ignore] // requires network
fn test_middleware_cors() {
    use std::process::{Command, Stdio};
    use std::time::Duration;

    let mut child = Command::new(env!("CARGO_BIN_EXE_sema"))
        .arg("-e")
        .arg(
            r#"
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
        "#,
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn");

    std::thread::sleep(Duration::from_millis(1500));

    let client = reqwest::blocking::Client::new();
    let resp = client
        .get("http://127.0.0.1:19908/api")
        .timeout(Duration::from_secs(5))
        .send()
        .unwrap();

    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers()
            .get("access-control-allow-origin")
            .unwrap()
            .to_str()
            .unwrap(),
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
    let resp = client
        .get("http://127.0.0.1:19909/test")
        .timeout(Duration::from_secs(5))
        .send()
        .unwrap();
    assert_eq!(resp.status(), 200);

    child.kill().ok();
    child.wait().ok();
}

#[test]
#[ignore] // requires network (binds localhost sockets)
fn test_http_serve_port_fallback_and_on_listen() {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};
    use std::time::Duration;

    // Occupy the target port with a plain (no-fallback) server.
    let mut occupier = Command::new(env!("CARGO_BIN_EXE_sema"))
        .arg("-e")
        .arg(r#"(http/serve (fn (req) (http/ok "a")) {:port 19910})"#)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn occupier");
    std::thread::sleep(Duration::from_millis(1200));

    // A second server requests the same port with :port-fallback + :on-listen.
    // It must land on a different port and report it via the callback.
    let mut child = Command::new(env!("CARGO_BIN_EXE_sema"))
        .arg("-e")
        .arg(
            r#"(http/serve (fn (req) (http/ok "b"))
                  {:port 19910
                   :port-fallback true
                   :on-listen (fn (info)
                     (println (string-append "BOUND:" (number->string (:port info)))))})"#,
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn fallback server");

    // Read the reported port from the on-listen callback's stdout line.
    let stdout = child.stdout.take().expect("piped stdout");
    let mut bound_port: Option<u16> = None;
    for line in BufReader::new(stdout).lines() {
        let line = line.unwrap_or_default();
        if let Some(rest) = line.strip_prefix("BOUND:") {
            bound_port = rest.trim().parse::<u16>().ok();
            break;
        }
    }

    let bound_port = bound_port.expect("fallback server should report a bound port");
    assert_ne!(
        bound_port, 19910,
        "fallback must not reuse the occupied port"
    );
    assert!(
        bound_port > 19910,
        "fallback advances upward from the start port"
    );

    // Prove the fallback server actually serves on the new port.
    let client = reqwest::blocking::Client::new();
    let resp = client
        .get(format!("http://127.0.0.1:{bound_port}/"))
        .timeout(Duration::from_secs(5))
        .send()
        .expect("request to fallback port");
    assert_eq!(resp.status(), 200);

    child.kill().ok();
    child.wait().ok();
    occupier.kill().ok();
    occupier.wait().ok();
}

#[test]
#[ignore] // requires network (binds localhost sockets)
fn test_http_serve_without_fallback_fails_on_taken_port() {
    use std::process::{Command, Stdio};
    use std::time::Duration;

    // Occupy the port.
    let mut occupier = Command::new(env!("CARGO_BIN_EXE_sema"))
        .arg("-e")
        .arg(r#"(http/serve (fn (req) (http/ok "a")) {:port 19912})"#)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn occupier");
    std::thread::sleep(Duration::from_millis(1200));

    // Without fallback, binding the taken port must fail fast (opt-in contract).
    let output = Command::new(env!("CARGO_BIN_EXE_sema"))
        .arg("-e")
        .arg(r#"(http/serve (fn (req) (http/ok "b")) {:port 19912})"#)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn second server")
        .wait_with_output()
        .expect("second server should exit, not hang");

    assert!(
        !output.status.success(),
        "http/serve on a taken port without fallback must error"
    );
    let stderr = String::from_utf8_lossy(&output.stderr).to_lowercase();
    assert!(
        stderr.contains("bind") || stderr.contains("in use") || stderr.contains("address"),
        "error should mention the bind failure, got: {stderr}"
    );

    occupier.kill().ok();
    occupier.wait().ok();
}
