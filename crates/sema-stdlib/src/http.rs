use std::collections::BTreeMap;
use std::time::Duration;

use sema_core::{check_arity, Caps, SemaError, Value, ValueView};

/// A process-wide `reqwest::Client` (`Send + Sync + Clone`) reused for every
/// request — sync and offloaded alike — so connections pool across threads and
/// overlapping tasks.
#[cfg(not(target_arch = "wasm32"))]
static HTTP_SHARED_CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();

/// Get (initializing on first use) the process-wide shared client.
#[cfg(not(target_arch = "wasm32"))]
fn http_shared_client() -> reqwest::Client {
    HTTP_SHARED_CLIENT.get_or_init(reqwest::Client::new).clone()
}

/// The response facts that cross the thread boundary back from the I/O pool
/// to the VM thread. Only plain `Send` data — never a `Value`/`Rc`.
/// Decoded into the same `Value` shape as the sync path on the VM thread.
#[cfg(not(target_arch = "wasm32"))]
struct RawHttpResponse {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

/// Resolve the method/url/headers/body `Value`s into a fully-built
/// `reqwest::RequestBuilder`. Shared by both the synchronous `block_on` path and
/// the offloaded (async) path, so both build the request identically.
fn build_request(
    client: &reqwest::Client,
    method: &str,
    url: &str,
    body: Option<&Value>,
    opts: Option<&Value>,
) -> Result<reqwest::RequestBuilder, SemaError> {
    let mut builder = match method {
        "GET" => client.get(url),
        "POST" => client.post(url),
        "PUT" => client.put(url),
        "DELETE" => client.delete(url),
        "PATCH" => client.patch(url),
        "HEAD" => client.head(url),
        other => return Err(SemaError::eval(format!("http: unknown method {other}"))),
    };

    // Apply options
    if let Some(opts_val) = opts {
        if let Some(opts_map) = opts_val.as_map_rc() {
            if let Some(headers_val) = opts_map.get(&Value::keyword("headers")) {
                if let Some(headers) = headers_val.as_map_rc() {
                    for (k, v) in headers.iter() {
                        let key = match k.view() {
                            ValueView::String(s) => s.to_string(),
                            ValueView::Keyword(s) => sema_core::resolve(s),
                            _ => k.to_string(),
                        };
                        let val = match v.as_str() {
                            Some(s) => s.to_string(),
                            None => v.to_string(),
                        };
                        builder = builder.header(key, val);
                    }
                }
            }
            if let Some(timeout_val) = opts_map.get(&Value::keyword("timeout")) {
                if let Some(ms) = timeout_val.as_int() {
                    builder = builder.timeout(Duration::from_millis(ms as u64));
                }
            }
        }
    }

    // Apply body
    if let Some(body_val) = body {
        if let Some(s) = body_val.as_str() {
            builder = builder.body(s.to_string());
        } else if body_val.as_map_rc().is_some() {
            let json = sema_core::value_to_json_lossy(body_val);
            let json_str = serde_json::to_string(&json)
                .map_err(|e| SemaError::eval(format!("http: json encode: {e}")))?;
            builder = builder
                .header("Content-Type", "application/json")
                .body(json_str);
        } else {
            builder = builder.body(body_val.to_string());
        }
    }

    Ok(builder)
}

/// Decode the response facts (status/headers/body) into the Sema response
/// `Value` map. Identical shape for the sync and async paths.
fn build_response_value(status: u16, headers: &[(String, String)], body: &str) -> Value {
    let mut headers_map = BTreeMap::new();
    for (k, v) in headers {
        headers_map.insert(Value::keyword(k), Value::string(v));
    }
    let mut result = BTreeMap::new();
    result.insert(Value::keyword("status"), Value::int(status as i64));
    result.insert(Value::keyword("headers"), Value::map(headers_map));
    result.insert(Value::keyword("body"), Value::string(body));
    Value::map(result)
}

fn http_request(
    method: &str,
    url: &str,
    body: Option<&Value>,
    opts: Option<&Value>,
) -> Result<Value, SemaError> {
    // Inside an `async/spawn`'d task: offload the round-trip onto the process-wide
    // I/O pool and yield `AwaitIo` so the scheduler can run sibling tasks while
    // this request is in flight. The request is built and the response decoded on
    // the VM thread; only `Send` facts cross the boundary.
    #[cfg(not(target_arch = "wasm32"))]
    if sema_core::in_async_context() {
        return http_request_async(method, url, body, opts);
    }

    // Top-level (not in a scheduler task): the synchronous path. `io_block_on`
    // drives the round-trip ON THIS (VM) thread using THE pool's reactor —
    // observable behavior identical to a dedicated blocking runtime.
    let client = http_shared_client();
    sema_io::io_block_on(async {
        let builder = build_request(&client, method, url, body, opts)?;

        let response = builder
            .send()
            .await
            .map_err(|e| SemaError::Io(format!("http {method} {url}: {e}")))?;

        let status = response.status().as_u16();
        let mut headers = Vec::new();
        for (k, v) in response.headers() {
            if let Ok(val) = v.to_str() {
                headers.push((k.as_str().to_string(), val.to_string()));
            }
        }
        let body_text = response
            .text()
            .await
            .map_err(|e| SemaError::Io(format!("http {method} {url}: read body: {e}")))?;

        Ok(build_response_value(status, &headers, &body_text))
    })
}

/// The offloaded (async-context) path: build the request on the VM thread,
/// `io_spawn` the send+read on the process-wide I/O pool, and yield an `AwaitIo`
/// handle whose poll closure decodes the `Send` response facts into the identical
/// `Value` shape the sync path returns. Returns `Ok(nil)` after arming the
/// yield signal; the scheduler delivers the real value on resume.
#[cfg(not(target_arch = "wasm32"))]
fn http_request_async(
    method: &str,
    url: &str,
    body: Option<&Value>,
    opts: Option<&Value>,
) -> Result<Value, SemaError> {
    use std::rc::Rc;
    use tokio::sync::oneshot::error::TryRecvError;

    // Vestigial under CALL_NATIVE (the scheduler delivers the resume value via
    // `replace_stack_top`, not by re-invoking this native), but kept for
    // symmetry with the shipped `async/await` yield pattern.
    if let Some(v) = sema_core::take_resume_value() {
        return Ok(v);
    }

    let client = http_shared_client();
    let builder = build_request(&client, method, url, body, opts)?;

    // Owned strings for the error-message format, which must match the sync
    // path's `http {method} {url}: ...` shape exactly so behavior is identical.
    let method_owned = method.to_string();
    let url_owned = url.to_string();

    let (tx, mut rx) = tokio::sync::oneshot::channel::<Result<RawHttpResponse, String>>();

    let abort = sema_io::io_spawn(async move {
        let result = async {
            let response = builder
                .send()
                .await
                .map_err(|e| format!("http {method_owned} {url_owned}: {e}"))?;
            let status = response.status().as_u16();
            let mut headers = Vec::new();
            for (k, v) in response.headers() {
                if let Ok(val) = v.to_str() {
                    headers.push((k.as_str().to_string(), val.to_string()));
                }
            }
            let body = response
                .text()
                .await
                .map_err(|e| format!("http {method_owned} {url_owned}: read body: {e}"))?;
            Ok(RawHttpResponse {
                status,
                headers,
                body,
            })
        }
        .await;
        let _ = tx.send(result);
        // Wake the parked VM thread so it re-polls promptly.
        sema_core::notify_io_complete();
    });

    // True cancellation: on cancel/timeout the scheduler calls the abort hook (the
    // seam's one-shot AbortHook), which aborts the spawned task → drops the in-flight
    // reqwest future → the connection is torn down (no wasted round-trip). Never
    // called on normal completion.
    let handle = Rc::new(sema_core::IoHandle::with_abort(
        move || match rx.try_recv() {
            Err(TryRecvError::Empty) => sema_core::IoPoll::Pending,
            Ok(Ok(raw)) => sema_core::IoPoll::Ready(Ok(build_response_value(
                raw.status,
                &raw.headers,
                &raw.body,
            ))),
            Ok(Err(msg)) => sema_core::IoPoll::Ready(Err(msg)),
            Err(TryRecvError::Closed) => {
                sema_core::IoPoll::Ready(Err("http: request worker dropped".to_string()))
            }
        },
        abort,
    ));
    sema_core::set_yield_signal(sema_core::YieldReason::AwaitIo(handle));
    Ok(Value::nil())
}

pub fn register(env: &sema_core::Env, sandbox: &sema_core::Sandbox) {
    crate::register_fn_gated(env, sandbox, Caps::NETWORK, "http/get", |args| {
        check_arity!(args, "http/get", 1..=2);
        let url = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let opts = args.get(1);
        http_request("GET", url, None, opts)
    });

    crate::register_fn_gated(env, sandbox, Caps::NETWORK, "http/post", |args| {
        check_arity!(args, "http/post", 2..=3);
        let url = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let body = &args[1];
        let opts = args.get(2);
        http_request("POST", url, Some(body), opts)
    });

    crate::register_fn_gated(env, sandbox, Caps::NETWORK, "http/put", |args| {
        check_arity!(args, "http/put", 2..=3);
        let url = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let body = &args[1];
        let opts = args.get(2);
        http_request("PUT", url, Some(body), opts)
    });

    crate::register_fn_gated(env, sandbox, Caps::NETWORK, "http/delete", |args| {
        check_arity!(args, "http/delete", 1..=2);
        let url = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let opts = args.get(1);
        http_request("DELETE", url, None, opts)
    });

    crate::register_fn_gated(env, sandbox, Caps::NETWORK, "http/request", |args| {
        check_arity!(args, "http/request", 2..=4);
        let method = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?
            .to_uppercase();
        let url = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        let opts = args.get(2);
        let body = args.get(3);
        http_request(&method, url, body, opts)
    });
}
