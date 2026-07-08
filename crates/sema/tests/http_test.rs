//! HTTP client tests. These run against an in-process mock HTTP/1.1 server
//! (no external network, no httpbin), so they are deterministic and not
//! `#[ignore]`d.
//!
//! The mock echoes each request as JSON — `{:method :path :query :args :headers
//! :body :json :content_type}` (header names lowercased) — plus a few special
//! endpoints: `/status/NNN`, `/bytes/N` (raw bytes), `/redirect/1`, and `/delay`.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};

use sema_core::Value;
use sema_eval::Interpreter;

fn eval(input: &str) -> Value {
    let interp = Interpreter::new();
    interp
        .eval_str(input)
        .unwrap_or_else(|e| panic!("failed to eval: {input}\nerror: {e}"))
}

fn eval_err(input: &str) -> sema_core::SemaError {
    let interp = Interpreter::new();
    interp.eval_str(input).unwrap_err()
}

// ── In-process mock server ────────────────────────────────────────────────

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Bind a mock server on an ephemeral port and return its base URL
/// (`http://127.0.0.1:PORT`). One detached thread per connection so a slow
/// endpoint (`/delay`) never blocks other requests.
fn mock_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || handle_conn(stream));
        }
    });
    format!("http://{addr}")
}

fn write_response(stream: &mut TcpStream, status: u16, content_type: &str, body: &[u8]) {
    let reason = match status {
        200 => "OK",
        302 => "Found",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "OK",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
}

fn parse_query(q: &str) -> serde_json::Value {
    let mut m = serde_json::Map::new();
    for pair in q.split('&').filter(|s| !s.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        m.insert(k.to_string(), serde_json::Value::String(v.to_string()));
    }
    serde_json::Value::Object(m)
}

fn handle_conn(mut stream: TcpStream) {
    // Read until the header terminator, then the body per Content-Length.
    let mut buf = Vec::new();
    let mut tmp = [0u8; 8192];
    let header_end = loop {
        match stream.read(&mut tmp) {
            Ok(0) => return,
            Ok(n) => {
                buf.extend_from_slice(&tmp[..n]);
                if let Some(p) = find(&buf, b"\r\n\r\n") {
                    break p + 4;
                }
                if buf.len() > 8 << 20 {
                    return;
                }
            }
            Err(_) => return,
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut rl = lines.next().unwrap_or("").split_whitespace();
    let method = rl.next().unwrap_or("GET").to_string();
    let target = rl.next().unwrap_or("/").to_string();
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (target.clone(), String::new()),
    };

    let mut headers = serde_json::Map::new();
    let mut content_length = 0usize;
    let mut content_type = String::new();
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            let k = k.trim().to_ascii_lowercase();
            let v = v.trim().to_string();
            if k == "content-length" {
                content_length = v.parse().unwrap_or(0);
            }
            if k == "content-type" {
                content_type = v.clone();
            }
            headers.insert(k, serde_json::Value::String(v));
        }
    }

    let mut body = buf[header_end..].to_vec();
    while body.len() < content_length {
        match stream.read(&mut tmp) {
            Ok(0) => break,
            Ok(n) => body.extend_from_slice(&tmp[..n]),
            Err(_) => break,
        }
    }
    if content_length > 0 && body.len() > content_length {
        body.truncate(content_length);
    }

    // Special endpoints.
    if let Some(rest) = path.strip_prefix("/status/") {
        let code: u16 = rest.parse().unwrap_or(200);
        write_response(
            &mut stream,
            code,
            "text/plain",
            format!("status {code}").as_bytes(),
        );
        return;
    }
    if let Some(rest) = path.strip_prefix("/bytes/") {
        let n: usize = rest.parse().unwrap_or(0);
        let data: Vec<u8> = (0..n).map(|i| (i % 256) as u8).collect();
        write_response(&mut stream, 200, "application/octet-stream", &data);
        return;
    }
    if path == "/redirect/1" {
        let resp = "HTTP/1.1 302 Found\r\nLocation: /echo\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        let _ = stream.write_all(resp.as_bytes());
        return;
    }
    if path == "/delay" {
        std::thread::sleep(std::time::Duration::from_secs(5));
        write_response(&mut stream, 200, "text/plain", b"delayed");
        return;
    }

    // HEAD: headers only, no body (so `:body` is "").
    if method == "HEAD" {
        let resp = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        let _ = stream.write_all(resp.as_bytes());
        return;
    }

    // Default: echo the request as JSON.
    let json_body = if content_type.contains("application/json") {
        serde_json::from_slice::<serde_json::Value>(&body).ok()
    } else {
        None
    };
    let resp = serde_json::json!({
        "method": method,
        "path": path,
        "query": query,
        "args": parse_query(&query),
        "headers": headers,
        "body": String::from_utf8_lossy(&body),
        "json": json_body,
        "content_type": content_type,
    });
    write_response(
        &mut stream,
        200,
        "application/json",
        &serde_json::to_vec(&resp).unwrap(),
    );
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[test]
fn test_http_get() {
    let base = mock_server();
    let result = eval(&format!(r#"(http/get "{base}/echo")"#));
    let m = result.as_map_rc().expect("expected map");
    assert_eq!(m.get(&Value::keyword("status")), Some(&Value::int(200)));
}

#[test]
fn test_http_post() {
    let base = mock_server();
    let result = eval(&format!(r#"(http/post "{base}/echo" {{:name "sema"}})"#));
    let m = result.as_map_rc().expect("expected map");
    assert_eq!(m.get(&Value::keyword("status")), Some(&Value::int(200)));
}

#[test]
fn test_http_request_generic() {
    let base = mock_server();
    let result = eval(&format!(
        r#"(http/request "PATCH" "{base}/echo" {{}} "data")"#
    ));
    let m = result.as_map_rc().expect("expected map");
    assert_eq!(m.get(&Value::keyword("status")), Some(&Value::int(200)));
}

#[test]
fn test_http_response_has_body() {
    let base = mock_server();
    assert_eq!(
        eval(&format!(r#"(string? (:body (http/get "{base}/echo")))"#)),
        Value::bool(true)
    );
    assert_eq!(
        eval(&format!(
            r#"(> (string-length (:body (http/get "{base}/echo"))) 0)"#
        )),
        Value::bool(true)
    );
}

#[test]
fn test_http_response_has_headers() {
    let base = mock_server();
    assert_eq!(
        eval(&format!(r#"(map? (:headers (http/get "{base}/echo")))"#)),
        Value::bool(true)
    );
    assert_eq!(
        eval(&format!(
            r#"(string? (get (:headers (http/get "{base}/echo")) :content-type))"#
        )),
        Value::bool(true)
    );
}

#[test]
fn test_http_response_body_json_decode() {
    let base = mock_server();
    let result = eval(&format!(
        r#"(get (json/decode (:body (http/get "{base}/echo"))) :path)"#
    ));
    assert_eq!(result, Value::string("/echo"));
}

#[test]
fn test_http_put() {
    let base = mock_server();
    let result = eval(&format!(
        r#"
        (let ((resp (http/put "{base}/echo" {{:name "sema"}})))
          (list (:status resp) (get (get (json/decode (:body resp)) :json) :name)))
    "#
    ));
    assert_eq!(
        result,
        Value::list(vec![Value::int(200), Value::string("sema")])
    );
}

#[test]
fn test_http_delete() {
    let base = mock_server();
    assert_eq!(
        eval(&format!(r#"(:status (http/delete "{base}/echo"))"#)),
        Value::int(200)
    );
}

#[test]
fn test_http_head() {
    let base = mock_server();
    let result = eval(&format!(
        r#"
        (let ((resp (http/request "HEAD" "{base}/echo")))
          (list (:status resp) (:body resp)))
    "#
    ));
    assert_eq!(
        result,
        Value::list(vec![Value::int(200), Value::string("")])
    );
}

#[test]
fn test_http_patch_with_string_body() {
    let base = mock_server();
    let result = eval(&format!(
        r#"
        (let ((resp (http/request "PATCH" "{base}/echo" {{}} "{{\"key\":\"val\"}}")))
          (list (:status resp) (get (json/decode (:body resp)) :body)))
    "#
    ));
    assert_eq!(
        result,
        Value::list(vec![Value::int(200), Value::string("{\"key\":\"val\"}")])
    );
}

#[test]
fn test_http_custom_headers() {
    let base = mock_server();
    let result = eval(&format!(
        r#"
        (let ((resp (http/get "{base}/echo" {{:headers {{:x-custom-header "sema-test"}}}})))
          (get (get (json/decode (:body resp)) :headers) :x-custom-header))
    "#
    ));
    assert_eq!(result, Value::string("sema-test"));
}

#[test]
fn test_http_multiple_headers() {
    let base = mock_server();
    let result = eval(&format!(
        r#"
        (let ((hdrs (get (json/decode (:body (http/get "{base}/echo"
                          {{:headers {{:x-first "one" :x-second "two"}}}}))) :headers)))
          (list (get hdrs :x-first) (get hdrs :x-second)))
    "#
    ));
    assert_eq!(
        result,
        Value::list(vec![Value::string("one"), Value::string("two")])
    );
}

#[test]
fn test_http_post_map_body_echoed() {
    let base = mock_server();
    let result = eval(&format!(
        r#"(get (get (json/decode (:body (http/post "{base}/echo" {{:name "sema" :version 1}}))) :json) :name)"#
    ));
    assert_eq!(result, Value::string("sema"));
}

#[test]
fn test_http_post_string_body() {
    let base = mock_server();
    let result = eval(&format!(
        r#"
        (get (json/decode (:body (http/post "{base}/echo" "raw-body-data"
              {{:headers {{:content-type "text/plain"}}}}))) :body)
    "#
    ));
    assert_eq!(result, Value::string("raw-body-data"));
}

#[test]
fn test_http_post_nested_map() {
    let base = mock_server();
    let result = eval(&format!(
        r#"(get (get (get (json/decode (:body (http/post "{base}/echo" {{:user {{:name "test"}}}}))) :json) :user) :name)"#
    ));
    assert_eq!(result, Value::string("test"));
}

#[test]
fn test_http_post_empty_string_body() {
    let base = mock_server();
    assert_eq!(
        eval(&format!(r#"(:status (http/post "{base}/echo" ""))"#)),
        Value::int(200)
    );
}

#[test]
fn test_http_status_404() {
    let base = mock_server();
    assert_eq!(
        eval(&format!(r#"(:status (http/get "{base}/status/404"))"#)),
        Value::int(404)
    );
}

#[test]
fn test_http_status_500() {
    let base = mock_server();
    assert_eq!(
        eval(&format!(r#"(:status (http/get "{base}/status/500"))"#)),
        Value::int(500)
    );
}

#[test]
fn test_http_get_with_query_params() {
    let base = mock_server();
    let result = eval(&format!(
        r#"(get (get (json/decode (:body (http/get "{base}/echo?foo=bar&baz=42"))) :args) :foo)"#
    ));
    assert_eq!(result, Value::string("bar"));
}

#[test]
fn test_http_timeout() {
    let base = mock_server();
    // Server delays 5s; client aborts at 500ms.
    let _err = eval_err(&format!(r#"(http/get "{base}/delay" {{:timeout 500}})"#));
}

#[test]
fn test_http_invalid_url() {
    // `.invalid` is a reserved TLD (RFC 2606) — DNS always fails, no network needed.
    let _err = eval_err(r#"(http/get "http://invalid.invalid.invalid")"#);
}

#[test]
fn test_http_unicode_body() {
    let base = mock_server();
    let result = eval(&format!(
        r#"(get (get (json/decode (:body (http/post "{base}/echo" {{:text "Hello 世界"}}))) :json) :text)"#
    ));
    assert_eq!(result, Value::string("Hello 世界"));
}

#[test]
fn test_http_redirect() {
    let base = mock_server();
    // 302 → /echo, followed automatically by the client.
    assert_eq!(
        eval(&format!(r#"(:status (http/get "{base}/redirect/1"))"#)),
        Value::int(200)
    );
}

#[test]
fn test_http_request_minimal_args() {
    let base = mock_server();
    assert_eq!(
        eval(&format!(r#"(:status (http/request "GET" "{base}/echo"))"#)),
        Value::int(200)
    );
}

#[test]
fn test_http_post_integer_body() {
    let base = mock_server();
    // Non-string/non-map body → to_string() fallback.
    let result = eval(&format!(
        r#"(get (json/decode (:body (http/post "{base}/echo" 42))) :body)"#
    ));
    assert_eq!(result, Value::string("42"));
}

#[test]
fn test_http_post_map_sets_content_type_json() {
    let base = mock_server();
    // A map body auto-sets the request Content-Type to application/json.
    let result = eval(&format!(
        r#"(string/starts-with?
             (get (get (json/decode (:body (http/post "{base}/echo" {{:a 1}}))) :headers) :content-type)
             "application/json")"#
    ));
    assert_eq!(result, Value::bool(true));
}

#[test]
fn test_http_headers_with_string_keys() {
    let base = mock_server();
    let result = eval(&format!(
        r#"(get (get (json/decode (:body (http/get "{base}/echo"
              {{:headers {{"X-String-Key" "string-val"}}}}))) :headers) :x-string-key)"#
    ));
    assert_eq!(result, Value::string("string-val"));
}

#[test]
fn test_http_get_opts_non_map_ignored() {
    let base = mock_server();
    assert_eq!(
        eval(&format!(
            r#"(:status (http/get "{base}/echo" "not-a-map"))"#
        )),
        Value::int(200)
    );
}

#[test]
fn test_http_delete_with_opts() {
    let base = mock_server();
    assert_eq!(
        eval(&format!(
            r#"(:status (http/delete "{base}/echo" {{:headers {{:x-delete-test "yes"}}}}))"#
        )),
        Value::int(200)
    );
}

// ── Binary bodies & downloads ─────────────────────────────────────────────

#[test]
fn test_http_response_as_bytes() {
    let base = mock_server();
    // `{:as :bytes}` returns the body as a bytevector, not a lossy string.
    let result = eval(&format!(
        r#"
        (let ((resp (http/get "{base}/bytes/16" {{:as :bytes}})))
          (list (bytevector? (:body resp)) (bytevector-length (:body resp)) (:status resp)))
    "#
    ));
    assert_eq!(
        result,
        Value::list(vec![Value::bool(true), Value::int(16), Value::int(200)])
    );
}

#[test]
fn test_http_response_default_is_text() {
    let base = mock_server();
    assert_eq!(
        eval(&format!(r#"(string? (:body (http/get "{base}/echo")))"#)),
        Value::bool(true)
    );
}

#[test]
fn test_http_bytevector_request_body() {
    let base = mock_server();
    // A bytevector body is sent as raw bytes and echoed back verbatim.
    let result = eval(&format!(
        r#"(get (json/decode (:body (http/post "{base}/echo" (string->bytevector "hello-bytes")))) :body)"#
    ));
    assert_eq!(result, Value::string("hello-bytes"));
}

// ── Multipart / file uploads ──────────────────────────────────────────────

#[test]
fn test_http_multipart_upload() {
    let base = mock_server();
    // The raw multipart body echoed back must contain the field name, filename,
    // and both the text field and the file bytes.
    let result = eval(&format!(
        r#"
        (let ((resp (http/post "{base}/echo" {{}}
                      {{:multipart (list
                         {{:name "field1" :content "value1"}}
                         {{:name "file" :filename "hi.txt"
                           :content (string->bytevector "FILEDATA") :content-type "text/plain"}})}})))
          (let ((b (get (json/decode (:body resp)) :body)))
            (list (string/contains? b "value1")
                  (string/contains? b "FILEDATA")
                  (string/contains? b "hi.txt")
                  (:status resp))))
    "#
    ));
    assert_eq!(
        result,
        Value::list(vec![
            Value::bool(true),
            Value::bool(true),
            Value::bool(true),
            Value::int(200)
        ])
    );
}

// ── QUERY method (RFC 10008) ──────────────────────────────────────────────

#[test]
fn test_http_query_method() {
    let base = mock_server();
    // http/query sends the QUERY method with a body; the server echoes both.
    let result = eval(&format!(
        r#"
        (let ((resp (http/query "{base}/echo" "q=search-terms")))
          (let ((data (json/decode (:body resp))))
            (list (:status resp) (get data :method) (get data :body))))
    "#
    ));
    assert_eq!(
        result,
        Value::list(vec![
            Value::int(200),
            Value::string("QUERY"),
            Value::string("q=search-terms")
        ])
    );
}

#[test]
fn test_http_request_custom_method() {
    let base = mock_server();
    // http/request accepts any valid method token (e.g. QUERY, OPTIONS).
    let result = eval(&format!(
        r#"(get (json/decode (:body (http/request "QUERY" "{base}/echo" {{}} "body"))) :method)"#
    ));
    assert_eq!(result, Value::string("QUERY"));
}
