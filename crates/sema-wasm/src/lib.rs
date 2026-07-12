use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};

use js_sys::Date;
use sema_core::{pretty_print, Env, NativeFn, SemaError, Value, ValueView};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

thread_local! {
    /// Completed lines of output (flushed by println/newline)
    static OUTPUT: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    /// Current line being built by display/print (not yet flushed)
    static LINE_BUF: RefCell<String> = const { RefCell::new(String::new()) };
    /// Start time for sys/elapsed (milliseconds since epoch)
    static WASM_START_MS: f64 = Date::now();
    /// In-memory virtual filesystem for WASM
    static VFS: RefCell<BTreeMap<String, String>> = const { RefCell::new(BTreeMap::new()) };
    /// Virtual directories (tracked for file/mkdir, file/is-directory?)
    static VFS_DIRS: RefCell<BTreeSet<String>> = RefCell::new({
        let mut s = BTreeSet::new();
        s.insert("/".to_string());
        s
    });
    /// In-memory HTTP response cache for the replay-with-cache strategy
    static HTTP_CACHE: RefCell<BTreeMap<String, Value>> = const { RefCell::new(BTreeMap::new()) };
    /// Total bytes currently stored in the VFS
    static VFS_TOTAL_BYTES: Cell<usize> = const { Cell::new(0) };
    /// Int32Array view over the control SharedArrayBuffer used for real
    /// `Atomics.wait` sleep when running inside a Web Worker (installed via
    /// `installAtomicsSleep`). `None` on the main thread (sleep stays an
    /// instant virtual-clock advance — `Atomics.wait` is illegal there anyway).
    static SLEEP_I32: RefCell<Option<js_sys::Int32Array>> = const { RefCell::new(None) };
    /// Optional sink called with each completed output line as it is produced
    /// (installed via `setOutputSink`). The Web Worker uses it to stream
    /// `println` output to the main thread live, so a long-running program
    /// (e.g. one that really sleeps) shows output as it happens instead of all
    /// at once at the end. `None` on the main thread (output is batched).
    static OUTPUT_SINK: RefCell<Option<js_sys::Function>> = const { RefCell::new(None) };
}

/// Blocking-sleep callback installed in the Web Worker: block this (worker)
/// thread for `ms` real milliseconds via `Atomics.wait` on the control SAB. The
/// cell value stays 0, so the wait simply times out after `ms` (a later cancel
/// can store a non-zero value + `Atomics.notify` to wake it early — see M6).
/// A plain `fn` (no captures) so it fits `sema_core::BlockingSleepFn`; it reads
/// the SAB view from the thread-local. Never called on the main thread.
fn worker_atomics_sleep(ms: u64) {
    SLEEP_I32.with(|s| {
        if let Some(arr) = s.borrow().as_ref() {
            // Slot 0 == 0 → block for `ms`; a cancel stores 1 + notifies, which
            // wakes this wait immediately so a Stop interrupts a sleep promptly.
            let _ = js_sys::Atomics::wait_with_timeout(arr, 0, 0, ms as f64);
        }
    });
}

/// Interrupt callback installed in the Web Worker: the main thread requests a
/// cancel by storing a non-zero value in control slot 0 (+ `Atomics.notify`).
/// The VM loop guard polls this so a Stop button aborts a running program.
fn worker_check_interrupt() -> bool {
    SLEEP_I32.with(|s| {
        s.borrow()
            .as_ref()
            .is_some_and(|arr| js_sys::Atomics::load(arr, 0).unwrap_or(0) != 0)
    })
}

/// Active debug session state for cooperative VM execution.
struct DebugSession {
    vm: sema_vm::VM,
    debug: sema_vm::DebugState,
}

thread_local! {
    static DEBUG_SESSION: RefCell<Option<DebugSession>> = const { RefCell::new(None) };
}

const VFS_MAX_TOTAL_BYTES: usize = 16 * 1024 * 1024; // 16 MB total
const VFS_MAX_FILE_BYTES: usize = 1024 * 1024; // 1 MB per file
const VFS_MAX_FILES: usize = 256;

fn vfs_check_quota(file_name: &str, new_content_len: usize) -> Result<(), SemaError> {
    if new_content_len > VFS_MAX_FILE_BYTES {
        return Err(SemaError::eval(format!(
            "VFS quota exceeded: file '{}' is {} bytes, max {} bytes per file",
            file_name, new_content_len, VFS_MAX_FILE_BYTES
        )));
    }

    VFS.with(|vfs| {
        let map = vfs.borrow();
        let old_len = map.get(file_name).map_or(0, |s| s.len());
        let is_new_file = !map.contains_key(file_name);

        if is_new_file && map.len() >= VFS_MAX_FILES {
            return Err(SemaError::eval(format!(
                "VFS quota exceeded: max {} files",
                VFS_MAX_FILES
            )));
        }

        let total = VFS_TOTAL_BYTES.with(|t| t.get());
        let new_total = total
            .saturating_add(new_content_len)
            .saturating_sub(old_len);
        if new_total > VFS_MAX_TOTAL_BYTES {
            return Err(SemaError::eval(format!(
                "VFS quota exceeded: would use {} bytes, max {} bytes total",
                new_total, VFS_MAX_TOTAL_BYTES
            )));
        }

        Ok(())
    })
}

/// Normalize a VFS path to canonical form: always starts with "/",
/// no trailing slash (except root), collapsed "//", resolved "." segments,
/// ".." rejected (no parent traversal in sandbox).
fn normalize_path(path: &str) -> Result<String, SemaError> {
    let path = path.trim();
    if path.is_empty() || path == "/" {
        return Ok("/".to_string());
    }

    let mut segments: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => continue,
            ".." => {
                return Err(SemaError::eval(
                    "VFS path error: '..' parent traversal not allowed",
                ));
            }
            s => segments.push(s),
        }
    }

    if segments.is_empty() {
        return Ok("/".to_string());
    }

    let mut result = String::with_capacity(path.len() + 1);
    for seg in &segments {
        result.push('/');
        result.push_str(seg);
    }
    Ok(result)
}

/// Append text to the current line buffer (no newline).
fn append_output(s: &str) {
    LINE_BUF.with(|b| b.borrow_mut().push_str(s));
}

/// Flush the current line buffer as a completed line.
fn flush_line() {
    let line = LINE_BUF.with(|b| {
        let line = b.borrow().clone();
        b.borrow_mut().clear();
        line
    });
    OUTPUT.with(|o| o.borrow_mut().push(line.clone()));
    // Stream the line live if a sink is installed (Web Worker path).
    OUTPUT_SINK.with(|s| {
        if let Some(f) = s.borrow().as_ref() {
            let _ = f.call1(&JsValue::NULL, &JsValue::from_str(&line));
        }
    });
}

/// Take all completed output lines, flushing any partial line first.
fn take_output() -> Vec<String> {
    // Flush any trailing partial line
    LINE_BUF.with(|b| {
        let buf = b.borrow();
        if !buf.is_empty() {
            let line = buf.clone();
            drop(buf);
            b.borrow_mut().clear();
            OUTPUT.with(|o| o.borrow_mut().push(line));
        }
    });
    OUTPUT.with(|o| o.borrow_mut().drain(..).collect())
}

const HTTP_AWAIT_MARKER: &str = "__SEMA_WASM_HTTP__";
const MAX_REPLAYS: usize = 50;

/// Instruction budget per cooperative VM yield. The VM will execute up to this
/// many instructions before yielding back to the browser event loop.
const WASM_DEBUG_INSTRUCTION_BUDGET: u32 = 500_000;

/// Build a deterministic cache key from HTTP request parameters.
fn http_cache_key(
    method: &str,
    url: &str,
    body: Option<&str>,
    headers: &[(String, String)],
) -> String {
    use std::fmt::Write;
    let mut key = format!("{method}\n{url}\n");
    match body {
        Some(b) => {
            write!(key, "{b}").unwrap();
        }
        None => {
            key.push_str("<nil>");
        }
    }
    key.push('\n');
    for (k, v) in headers {
        writeln!(key, "{k}:{v}").unwrap();
    }
    key
}

/// Create a marker error whose message encodes an HTTP request as JSON.
fn http_await_marker(
    method: &str,
    url: &str,
    body: Option<&str>,
    headers: &[(String, String)],
    timeout_ms: Option<i64>,
) -> SemaError {
    let key = http_cache_key(method, url, body, headers);
    let body_json = match body {
        Some(b) => format!("\"{}\"", escape_json(b)),
        None => "null".to_string(),
    };
    let timeout_json = match timeout_ms {
        Some(t) => format!("{t}"),
        None => "null".to_string(),
    };
    let headers_json = headers
        .iter()
        .map(|(k, v)| format!("[\"{}\",\"{}\"]", escape_json(k), escape_json(v)))
        .collect::<Vec<_>>()
        .join(",");
    let payload = format!(
        "{}{{\"key\":\"{}\",\"method\":\"{}\",\"url\":\"{}\",\"body\":{},\"headers\":[{}],\"timeout\":{}}}",
        HTTP_AWAIT_MARKER,
        escape_json(&key),
        escape_json(method),
        escape_json(url),
        body_json,
        headers_json,
        timeout_json,
    );
    SemaError::eval(payload)
}

/// Check whether an error is an HTTP await marker.
fn is_http_await_marker(err: &SemaError) -> bool {
    match err.inner() {
        SemaError::Eval(msg) => msg.starts_with(HTTP_AWAIT_MARKER),
        _ => false,
    }
}

/// Extract the JSON payload from an HTTP await marker error.
fn parse_http_marker(err: &SemaError) -> Option<String> {
    match err.inner() {
        SemaError::Eval(msg) if msg.starts_with(HTTP_AWAIT_MARKER) => {
            Some(msg[HTTP_AWAIT_MARKER.len()..].to_string())
        }
        _ => None,
    }
}

/// Clear the HTTP response cache.
fn clear_http_cache() {
    HTTP_CACHE.with(|c| c.borrow_mut().clear());
}

/// Perform an HTTP request via the replay-with-cache strategy.
/// On cache hit, returns the cached response. On cache miss, returns a marker error.
fn wasm_http_request(
    method: &str,
    url: &str,
    body: Option<&Value>,
    opts: Option<&Value>,
) -> Result<Value, SemaError> {
    let mut headers: Vec<(String, String)> = Vec::new();
    let mut timeout_ms: Option<i64> = None;
    let mut has_content_type = false;

    if let Some(opts_val) = opts {
        if let Some(opts_map) = opts_val.as_map_rc() {
            if let Some(headers_val) = opts_map.get(&Value::keyword("headers")) {
                if let Some(hmap) = headers_val.as_map_rc() {
                    for (k, v) in hmap.iter() {
                        let key = match k.view() {
                            ValueView::String(s) => s.to_string(),
                            ValueView::Keyword(s) => sema_core::resolve(s),
                            _ => k.to_string(),
                        };
                        let val = match v.as_str() {
                            Some(s) => s.to_string(),
                            None => v.to_string(),
                        };
                        if key.eq_ignore_ascii_case("content-type") {
                            has_content_type = true;
                        }
                        headers.push((key, val));
                    }
                }
            }
            if let Some(timeout_val) = opts_map.get(&Value::keyword("timeout")) {
                if let Some(ms) = timeout_val.as_int() {
                    timeout_ms = Some(ms);
                }
            }
        }
    }

    let body_str = match body {
        Some(val) => {
            if let Some(s) = val.as_str() {
                Some(s.to_string())
            } else if val.as_map_rc().is_some() {
                let json = sema_core::value_to_json_lossy(val);
                let json_str = serde_json::to_string(&json)
                    .map_err(|e| SemaError::eval(format!("http: json encode: {e}")))?;
                if !has_content_type {
                    headers.push(("Content-Type".to_string(), "application/json".to_string()));
                }
                Some(json_str)
            } else if val.is_nil() {
                None
            } else {
                Some(val.to_string())
            }
        }
        None => None,
    };

    headers.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));

    let key = http_cache_key(method, url, body_str.as_deref(), &headers);

    // In a Web Worker there is no `window`. Do a real *synchronous* XHR: it
    // blocks the worker thread and returns the response directly, so http works
    // without the main-thread replay-the-whole-program hack — and it composes
    // correctly with real `Atomics.wait` sleeps (no re-runs). Cross-origin
    // targets still need CORS/CORP (a same-origin proxy covers the rest).
    if web_sys::window().is_none() {
        return perform_fetch_sync(method, url, body_str.as_deref(), &headers);
    }

    let cached = HTTP_CACHE.with(|c| c.borrow().get(&key).cloned());
    if let Some(val) = cached {
        return Ok(val);
    }

    Err(http_await_marker(
        method,
        url,
        body_str.as_deref(),
        &headers,
        timeout_ms,
    ))
}

/// Synchronous HTTP via `XMLHttpRequest` (worker-only — sync XHR is illegal on
/// the main thread). Blocks the calling (worker) thread until the response,
/// returning the same `{:status :headers :body}` map shape as `perform_fetch`.
/// (Per-request timeout is not applied on this path; the worker can be cancelled
/// via the M6 control buffer instead.)
fn perform_fetch_sync(
    method: &str,
    url: &str,
    body: Option<&str>,
    headers: &[(String, String)],
) -> Result<Value, SemaError> {
    let xhr = web_sys::XmlHttpRequest::new()
        .map_err(|_| SemaError::Io("failed to create XMLHttpRequest".to_string()))?;
    // async = false → synchronous (blocks this worker thread).
    xhr.open_with_async(method, url, false)
        .map_err(|e| SemaError::Io(format!("http: open failed: {}", js_err(&e))))?;
    for (k, v) in headers {
        let _ = xhr.set_request_header(k, v);
    }
    let send = match body {
        Some(b) => xhr.send_with_opt_str(Some(b)),
        None => xhr.send(),
    };
    send.map_err(|e| SemaError::Io(format!("http: request failed: {}", js_err(&e))))?;

    let status = xhr.status().unwrap_or(0) as i64;
    let body_text = xhr.response_text().ok().flatten().unwrap_or_default();

    let mut resp_headers = BTreeMap::new();
    if let Ok(raw) = xhr.get_all_response_headers() {
        for line in raw.split("\r\n").filter(|l| !l.is_empty()) {
            if let Some((k, v)) = line.split_once(':') {
                resp_headers.insert(Value::keyword(k.trim()), Value::string(v.trim()));
            }
        }
    }

    let mut result = BTreeMap::new();
    result.insert(Value::keyword("status"), Value::int(status));
    result.insert(Value::keyword("headers"), Value::map(resp_headers));
    result.insert(Value::keyword("body"), Value::string(&body_text));
    Ok(Value::map(result))
}

/// Best-effort string for a JS error value.
fn js_err(e: &JsValue) -> String {
    e.as_string()
        .or_else(|| {
            js_sys::Reflect::get(e, &JsValue::from_str("message"))
                .ok()
                .and_then(|m| m.as_string())
        })
        .unwrap_or_else(|| "error".to_string())
}

/// Perform an HTTP fetch via the browser's `fetch()` API.
async fn perform_fetch(
    method: &str,
    url: &str,
    body: Option<&str>,
    headers: &[(String, String)],
    timeout_ms: Option<u64>,
) -> Result<Value, SemaError> {
    let window = web_sys::window()
        .ok_or_else(|| SemaError::Io("no global `window` available".to_string()))?;

    let opts = web_sys::RequestInit::new();
    opts.set_method(method);
    opts.set_mode(web_sys::RequestMode::Cors);

    if let Some(body_str) = body {
        opts.set_body(&JsValue::from_str(body_str));
    }

    let abort_controller = if timeout_ms.is_some() {
        let controller = web_sys::AbortController::new()
            .map_err(|_| SemaError::Io("failed to create AbortController".to_string()))?;
        opts.set_signal(Some(&controller.signal()));
        Some(controller)
    } else {
        None
    };

    let request = web_sys::Request::new_with_str_and_init(url, &opts).map_err(|e| {
        SemaError::Io(format!(
            "failed to create request: {}",
            e.as_string().unwrap_or_default()
        ))
    })?;

    for (k, v) in headers {
        request.headers().set(k, v).map_err(|e| {
            SemaError::Io(format!(
                "failed to set header: {}",
                e.as_string().unwrap_or_default()
            ))
        })?;
    }

    if let (Some(ms), Some(controller)) = (timeout_ms, &abort_controller) {
        let c = controller.clone();
        let closure = wasm_bindgen::closure::Closure::once(move || {
            c.abort();
        });
        let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(
            closure.as_ref().unchecked_ref(),
            clamp_timeout_ms(ms),
        );
        closure.forget();
    }

    let resp_jsvalue = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|e| {
            let msg = e
                .as_string()
                .or_else(|| {
                    js_sys::Reflect::get(&e, &JsValue::from_str("message"))
                        .ok()
                        .and_then(|m| m.as_string())
                })
                .unwrap_or_else(|| "fetch failed".to_string());
            SemaError::Io(msg)
        })?;

    let response: web_sys::Response = resp_jsvalue
        .dyn_into()
        .map_err(|_| SemaError::Io("fetch did not return a Response".to_string()))?;

    let status = response.status() as i64;

    let mut resp_headers = BTreeMap::new();
    if let Ok(Some(iter)) = js_sys::try_iter(&response.headers()) {
        for entry in iter.flatten() {
            let arr: js_sys::Array = entry.into();
            if arr.length() >= 2 {
                let k = arr.get(0).as_string().unwrap_or_default();
                let v = arr.get(1).as_string().unwrap_or_default();
                resp_headers.insert(Value::keyword(&k), Value::string(&v));
            }
        }
    }

    let body_promise = response.text().map_err(|e| {
        SemaError::Io(format!(
            "failed to read response body: {}",
            e.as_string().unwrap_or_default()
        ))
    })?;
    let body_jsvalue = JsFuture::from(body_promise).await.map_err(|e| {
        SemaError::Io(format!(
            "failed to read response body: {}",
            e.as_string().unwrap_or_default()
        ))
    })?;
    let body_text = body_jsvalue.as_string().unwrap_or_default();

    let mut result = BTreeMap::new();
    result.insert(Value::keyword("status"), Value::int(status));
    result.insert(Value::keyword("headers"), Value::map(resp_headers));
    result.insert(Value::keyword("body"), Value::string(&body_text));

    Ok(Value::map(result))
}

/// Parse an HTTP marker JSON and perform the fetch, returning (cache_key, response).
async fn perform_fetch_from_marker(json_str: &str) -> Result<(String, Value), SemaError> {
    let parsed: serde_json::Value = serde_json::from_str(json_str)
        .map_err(|e| SemaError::eval(format!("failed to parse HTTP marker JSON: {e}")))?;

    let key = parsed["key"]
        .as_str()
        .ok_or_else(|| SemaError::eval("HTTP marker missing 'key'"))?
        .to_string();
    let method = parsed["method"]
        .as_str()
        .ok_or_else(|| SemaError::eval("HTTP marker missing 'method'"))?;
    let url = parsed["url"]
        .as_str()
        .ok_or_else(|| SemaError::eval("HTTP marker missing 'url'"))?;
    let body = parsed["body"].as_str();
    let timeout_ms = parsed["timeout"].as_u64();

    let mut headers = Vec::new();
    if let Some(arr) = parsed["headers"].as_array() {
        for pair in arr {
            if let Some(pair_arr) = pair.as_array() {
                if pair_arr.len() >= 2 {
                    let k = pair_arr[0].as_str().unwrap_or_default().to_string();
                    let v = pair_arr[1].as_str().unwrap_or_default().to_string();
                    headers.push((k, v));
                }
            }
        }
    }

    let response = perform_fetch(method, url, body, &headers, timeout_ms).await?;
    Ok((key, response))
}

/// Register print/println/display/newline that write to the output buffer instead of stdout
type WasmNativeFn = Box<dyn Fn(&[Value]) -> Result<Value, SemaError>>;

fn register_wasm_io(env: &Env) {
    let register = |name: &str, f: WasmNativeFn| {
        env.set(
            sema_core::intern(name),
            Value::native_fn(NativeFn::simple(name, move |args| f(args))),
        );
    };

    // display: append to current line, no newline (like native print! without newline)
    register(
        "display",
        Box::new(|args: &[Value]| {
            let mut out = String::new();
            for (i, arg) in args.iter().enumerate() {
                if i > 0 {
                    out.push(' ');
                }
                if let Some(s) = arg.as_str() {
                    out.push_str(s);
                } else {
                    out.push_str(&format!("{arg}"));
                }
            }
            append_output(&out);
            Ok(Value::nil())
        }),
    );

    // print: append to current line, no newline
    register(
        "print",
        Box::new(|args: &[Value]| {
            let mut out = String::new();
            for (i, arg) in args.iter().enumerate() {
                if i > 0 {
                    out.push(' ');
                }
                out.push_str(&format!("{arg}"));
            }
            append_output(&out);
            Ok(Value::nil())
        }),
    );

    // println: append to current line, then flush (emit newline)
    register(
        "println",
        Box::new(|args: &[Value]| {
            let mut out = String::new();
            for (i, arg) in args.iter().enumerate() {
                if i > 0 {
                    out.push(' ');
                }
                if let Some(s) = arg.as_str() {
                    out.push_str(s);
                } else {
                    out.push_str(&format!("{arg}"));
                }
            }
            append_output(&out);
            flush_line();
            Ok(Value::nil())
        }),
    );

    // pprint: pretty-print a value and flush
    register(
        "pprint",
        Box::new(|args: &[Value]| {
            if args.len() != 1 {
                return Err(SemaError::arity("pprint", "1", args.len()));
            }
            append_output(&pretty_print(&args[0], 80));
            flush_line();
            Ok(Value::nil())
        }),
    );

    // newline: flush current line
    register(
        "newline",
        Box::new(|_args: &[Value]| {
            flush_line();
            Ok(Value::nil())
        }),
    );

    register(
        "print-error",
        Box::new(|args: &[Value]| {
            let mut out = String::new();
            for (i, arg) in args.iter().enumerate() {
                if i > 0 {
                    out.push(' ');
                }
                out.push_str(&format!("{arg}"));
            }
            append_output(&format!("[error] {out}"));
            flush_line();
            Ok(Value::nil())
        }),
    );

    register(
        "println-error",
        Box::new(|args: &[Value]| {
            let mut out = String::new();
            for (i, arg) in args.iter().enumerate() {
                if i > 0 {
                    out.push(' ');
                }
                out.push_str(&format!("{arg}"));
            }
            append_output(&format!("[error] {out}"));
            flush_line();
            Ok(Value::nil())
        }),
    );

    // time-ms: use Date.now() from the web platform
    register(
        "time-ms",
        Box::new(|args: &[Value]| {
            if !args.is_empty() {
                return Err(SemaError::arity("time-ms", "0", args.len()));
            }
            Ok(Value::int(Date::now() as i64))
        }),
    );

    // term/* pass-through shims (ANSI codes are useless in the browser)
    for name in &[
        "term/bold",
        "term/dim",
        "term/italic",
        "term/underline",
        "term/inverse",
        "term/strikethrough",
        "term/black",
        "term/red",
        "term/green",
        "term/yellow",
        "term/blue",
        "term/magenta",
        "term/cyan",
        "term/white",
        "term/gray",
        "term/strip",
    ] {
        let fn_name = name.to_string();
        register(
            name,
            Box::new(move |args: &[Value]| {
                if args.len() != 1 {
                    return Err(SemaError::arity(&fn_name, "1", args.len()));
                }
                let text = args[0]
                    .as_str()
                    .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
                Ok(Value::string(text))
            }),
        );
    }

    // term/style: return first arg unchanged
    register(
        "term/style",
        Box::new(|args: &[Value]| {
            if args.is_empty() {
                return Err(SemaError::arity("term/style", "1+", args.len()));
            }
            let text = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
            Ok(Value::string(text))
        }),
    );

    // term/rgb: return first arg unchanged
    register(
        "term/rgb",
        Box::new(|args: &[Value]| {
            if args.len() != 4 {
                return Err(SemaError::arity("term/rgb", "4", args.len()));
            }
            let text = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
            Ok(Value::string(text))
        }),
    );

    // sys/platform
    register(
        "sys/platform",
        Box::new(|args: &[Value]| {
            if !args.is_empty() {
                return Err(SemaError::arity("sys/platform", "0", args.len()));
            }
            Ok(Value::string("web"))
        }),
    );

    // sys/arch
    register(
        "sys/arch",
        Box::new(|args: &[Value]| {
            if !args.is_empty() {
                return Err(SemaError::arity("sys/arch", "0", args.len()));
            }
            Ok(Value::string("wasm32"))
        }),
    );

    // sys/os
    register(
        "sys/os",
        Box::new(|args: &[Value]| {
            if !args.is_empty() {
                return Err(SemaError::arity("sys/os", "0", args.len()));
            }
            Ok(Value::string("web"))
        }),
    );

    // sys/elapsed: nanoseconds since WASM module load
    register(
        "sys/elapsed",
        Box::new(|args: &[Value]| {
            if !args.is_empty() {
                return Err(SemaError::arity("sys/elapsed", "0", args.len()));
            }
            let nanos = WASM_START_MS.with(|&start| ((Date::now() - start) * 1_000_000.0) as i64);
            Ok(Value::int(nanos))
        }),
    );

    // sleep: no-op in WASM
    register(
        "sleep",
        Box::new(|args: &[Value]| {
            if args.len() != 1 {
                return Err(SemaError::arity("sleep", "1", args.len()));
            }
            args[0]
                .as_int()
                .ok_or_else(|| SemaError::type_error("int", args[0].type_name()))?;
            Ok(Value::nil())
        }),
    );

    // env: always nil in WASM
    register(
        "env",
        Box::new(|args: &[Value]| {
            if args.len() != 1 {
                return Err(SemaError::arity("env", "1", args.len()));
            }
            args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
            Ok(Value::nil())
        }),
    );

    // sys/set-env: no-op in WASM
    register(
        "sys/set-env",
        Box::new(|args: &[Value]| {
            if args.len() != 2 {
                return Err(SemaError::arity("sys/set-env", "2", args.len()));
            }
            args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
            args[1]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
            Ok(Value::nil())
        }),
    );

    // sys/env-all: empty map in WASM
    register(
        "sys/env-all",
        Box::new(|args: &[Value]| {
            if !args.is_empty() {
                return Err(SemaError::arity("sys/env-all", "0", args.len()));
            }
            Ok(Value::map(std::collections::BTreeMap::new()))
        }),
    );

    // exit: not supported in WASM
    register(
        "exit",
        Box::new(|_args: &[Value]| Err(SemaError::eval("exit not supported in WASM"))),
    );

    // sys/interactive?: always false in WASM
    register(
        "sys/interactive?",
        Box::new(|args: &[Value]| {
            if !args.is_empty() {
                return Err(SemaError::arity("sys/interactive?", "0", args.len()));
            }
            Ok(Value::bool(false))
        }),
    );

    // path/join: join path segments with "/"
    register(
        "path/join",
        Box::new(|args: &[Value]| {
            if args.is_empty() {
                return Err(SemaError::arity("path/join", "1+", 0));
            }
            let mut parts = Vec::new();
            for arg in args {
                let s = arg
                    .as_str()
                    .ok_or_else(|| SemaError::type_error("string", arg.type_name()))?;
                parts.push(s.to_string());
            }
            let joined = parts
                .iter()
                .enumerate()
                .fold(String::new(), |mut acc, (i, part)| {
                    if i > 0 && !acc.ends_with('/') && !part.starts_with('/') {
                        acc.push('/');
                    }
                    acc.push_str(part);
                    acc
                });
            Ok(Value::string(&joined))
        }),
    );

    // path/dir (canonical) + path/dirname (legacy alias): parent directory of a path.
    // Returns "" when there is no parent component.
    fn wasm_path_dir(args: &[Value]) -> Result<Value, SemaError> {
        if args.len() != 1 {
            return Err(SemaError::arity("path/dir", "1", args.len()));
        }
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let trimmed = s.trim_end_matches('/');
        match trimmed.rfind('/') {
            Some(0) => Ok(Value::string("/")),
            Some(pos) => Ok(Value::string(&trimmed[..pos])),
            None => Ok(Value::string("")),
        }
    }
    register("path/dir", Box::new(wasm_path_dir));
    register("path/dirname", Box::new(wasm_path_dir));

    // path/filename (canonical) + path/basename (legacy alias): filename component of a path.
    fn wasm_path_filename(args: &[Value]) -> Result<Value, SemaError> {
        if args.len() != 1 {
            return Err(SemaError::arity("path/filename", "1", args.len()));
        }
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let trimmed = s.trim_end_matches('/');
        match trimmed.rfind('/') {
            Some(pos) => Ok(Value::string(&trimmed[pos + 1..])),
            None => Ok(Value::string(trimmed)),
        }
    }
    register("path/filename", Box::new(wasm_path_filename));
    register("path/basename", Box::new(wasm_path_filename));

    // path/extension (canonical) + path/ext (legacy alias): file extension (without dot).
    // Returns "" when the path has no extension.
    fn wasm_path_extension(args: &[Value]) -> Result<Value, SemaError> {
        if args.len() != 1 {
            return Err(SemaError::arity("path/extension", "1", args.len()));
        }
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let trimmed = s.trim_end_matches('/');
        let basename = match trimmed.rfind('/') {
            Some(pos) => &trimmed[pos + 1..],
            None => trimmed,
        };
        match basename.rfind('.') {
            Some(0) | None => Ok(Value::string("")),
            Some(pos) => Ok(Value::string(&basename[pos + 1..])),
        }
    }
    register("path/extension", Box::new(wasm_path_extension));
    register("path/ext", Box::new(wasm_path_extension));

    // path/absolute: in WASM, just return the input unchanged
    register(
        "path/absolute",
        Box::new(|args: &[Value]| {
            if args.len() != 1 {
                return Err(SemaError::arity("path/absolute", "1", args.len()));
            }
            let s = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
            Ok(Value::string(s))
        }),
    );

    // --- web/* namespace: browser environment detection (WASM-only) ---

    register(
        "web/user-agent",
        Box::new(|args: &[Value]| {
            if !args.is_empty() {
                return Err(SemaError::arity("web/user-agent", "0", args.len()));
            }
            match js_sys::eval("navigator.userAgent") {
                Ok(val) => match val.as_string() {
                    Some(s) => Ok(Value::string(&s)),
                    None => Ok(Value::nil()),
                },
                Err(_) => Ok(Value::nil()),
            }
        }),
    );

    register(
        "web/user-agent-data",
        Box::new(|args: &[Value]| {
            if !args.is_empty() {
                return Err(SemaError::arity("web/user-agent-data", "0", args.len()));
            }
            // navigator.userAgentData is Chromium-only; returns nil on Firefox/Safari
            let script = r#"
                (function() {
                    var d = navigator.userAgentData;
                    if (!d) return null;
                    return JSON.stringify({
                        mobile: d.mobile,
                        platform: d.platform,
                        brands: d.brands.map(function(b) { return b.brand + "/" + b.version; })
                    });
                })()
            "#;
            match js_sys::eval(script) {
                Ok(val) => match val.as_string() {
                    Some(json_str) => {
                        // Parse the JSON into a Sema map
                        match serde_json::from_str::<serde_json::Value>(&json_str) {
                            Ok(json) => Ok(sema_core::json_to_value(&json)),
                            Err(_) => Ok(Value::nil()),
                        }
                    }
                    None => Ok(Value::nil()),
                },
                Err(_) => Ok(Value::nil()),
            }
        }),
    );

    // --- HTTP via replay-with-cache (async eval catches markers and performs fetch) ---

    register(
        "http/get",
        Box::new(|args: &[Value]| {
            if args.is_empty() || args.len() > 2 {
                return Err(SemaError::arity("http/get", "1 or 2", args.len()));
            }
            let url = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
            wasm_http_request("GET", url, None, args.get(1))
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
            wasm_http_request("POST", url, Some(&args[1]), args.get(2))
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
            wasm_http_request("PUT", url, Some(&args[1]), args.get(2))
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
            wasm_http_request("DELETE", url, None, args.get(1))
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
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
            let url = args[1]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
            let body = args.get(2);
            let opts = args.get(3);
            wasm_http_request(method, url, body, opts)
        }),
    );

    // --- sys/* stubs for unsupported system functions ---

    register(
        "sys/args",
        Box::new(|args: &[Value]| {
            if !args.is_empty() {
                return Err(SemaError::arity("sys/args", "0", args.len()));
            }
            Ok(Value::list(Vec::new()))
        }),
    );

    register(
        "sys/cwd",
        Box::new(|args: &[Value]| {
            if !args.is_empty() {
                return Err(SemaError::arity("sys/cwd", "0", args.len()));
            }
            Ok(Value::string("/"))
        }),
    );

    register(
        "sys/home-dir",
        Box::new(|args: &[Value]| {
            if !args.is_empty() {
                return Err(SemaError::arity("sys/home-dir", "0", args.len()));
            }
            Ok(Value::nil())
        }),
    );

    register(
        "sys/temp-dir",
        Box::new(|args: &[Value]| {
            if !args.is_empty() {
                return Err(SemaError::arity("sys/temp-dir", "0", args.len()));
            }
            Ok(Value::string("/tmp"))
        }),
    );

    register(
        "sys/hostname",
        Box::new(|args: &[Value]| {
            if !args.is_empty() {
                return Err(SemaError::arity("sys/hostname", "0", args.len()));
            }
            Ok(Value::nil())
        }),
    );

    register(
        "sys/user",
        Box::new(|args: &[Value]| {
            if !args.is_empty() {
                return Err(SemaError::arity("sys/user", "0", args.len()));
            }
            Ok(Value::nil())
        }),
    );

    register(
        "sys/pid",
        Box::new(|args: &[Value]| {
            if !args.is_empty() {
                return Err(SemaError::arity("sys/pid", "0", args.len()));
            }
            Ok(Value::int(0))
        }),
    );

    register(
        "sys/which",
        Box::new(|args: &[Value]| {
            if args.len() != 1 {
                return Err(SemaError::arity("sys/which", "1", args.len()));
            }
            args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
            Ok(Value::nil())
        }),
    );

    register(
        "sys/tty",
        Box::new(|args: &[Value]| {
            if !args.is_empty() {
                return Err(SemaError::arity("sys/tty", "0", args.len()));
            }
            Ok(Value::nil())
        }),
    );

    // --- VFS file operation shims ---

    register(
        "file/read",
        Box::new(|args: &[Value]| {
            if args.len() != 1 {
                return Err(SemaError::arity("file/read", "1", args.len()));
            }
            let path = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
            let path = &normalize_path(path)?;
            VFS.with(|vfs| match vfs.borrow().get(path.as_str()) {
                Some(content) => Ok(Value::string(content)),
                None => Err(SemaError::Io(format!("file/read {path}: No such file"))),
            })
        }),
    );

    register(
        "file/write",
        Box::new(|args: &[Value]| {
            if args.len() != 2 {
                return Err(SemaError::arity("file/write", "2", args.len()));
            }
            let path = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
            let path = &normalize_path(path)?;
            let content = args[1]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
            vfs_check_quota(path, content.len())?;
            VFS.with(|vfs| {
                let mut map = vfs.borrow_mut();
                let old_len = map.get(path.as_str()).map_or(0, |s| s.len());
                map.insert(path.to_string(), content.to_string());
                VFS_TOTAL_BYTES.with(|t| {
                    t.set(
                        t.get()
                            .saturating_add(content.len())
                            .saturating_sub(old_len),
                    );
                });
            });
            Ok(Value::nil())
        }),
    );

    register(
        "file/exists?",
        Box::new(|args: &[Value]| {
            if args.len() != 1 {
                return Err(SemaError::arity("file/exists?", "1", args.len()));
            }
            let path = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
            let path = &normalize_path(path)?;
            let in_vfs = VFS.with(|vfs| vfs.borrow().contains_key(path.as_str()));
            let in_dirs = VFS_DIRS.with(|dirs| dirs.borrow().contains(path.as_str()));
            Ok(Value::bool(in_vfs || in_dirs))
        }),
    );

    register(
        "file/delete",
        Box::new(|args: &[Value]| {
            if args.len() != 1 {
                return Err(SemaError::arity("file/delete", "1", args.len()));
            }
            let path = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
            let path = &normalize_path(path)?;
            VFS.with(|vfs| match vfs.borrow_mut().remove(path.as_str()) {
                Some(old) => {
                    VFS_TOTAL_BYTES.with(|t| t.set(t.get().saturating_sub(old.len())));
                    Ok(Value::nil())
                }
                None => Err(SemaError::Io(format!("file/delete {path}: No such file"))),
            })
        }),
    );

    register(
        "file/rename",
        Box::new(|args: &[Value]| {
            if args.len() != 2 {
                return Err(SemaError::arity("file/rename", "2", args.len()));
            }
            let from = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
            let from = &normalize_path(from)?;
            let to = args[1]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
            let to = &normalize_path(to)?;
            VFS.with(|vfs| {
                let mut map = vfs.borrow_mut();
                match map.remove(from.as_str()) {
                    Some(content) => {
                        let overwritten_len = map.get(to.as_str()).map_or(0, |s| s.len());
                        map.insert(to.to_string(), content);
                        VFS_TOTAL_BYTES.with(|t| t.set(t.get().saturating_sub(overwritten_len)));
                        Ok(Value::nil())
                    }
                    None => Err(SemaError::Io(format!(
                        "file/rename {from} -> {to}: No such file"
                    ))),
                }
            })
        }),
    );

    register(
        "file/list",
        Box::new(|args: &[Value]| {
            if args.len() != 1 {
                return Err(SemaError::arity("file/list", "1", args.len()));
            }
            let dir = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
            let dir = &normalize_path(dir)?;
            let prefix = if dir == "/" {
                "/".to_string()
            } else {
                format!("{dir}/")
            };
            let mut names = BTreeSet::new();
            VFS.with(|vfs| {
                for key in vfs.borrow().keys() {
                    if let Some(rest) = key.strip_prefix(&prefix) {
                        if !rest.is_empty() && !rest.contains('/') {
                            names.insert(rest.to_string());
                        }
                    }
                }
            });
            VFS_DIRS.with(|dirs| {
                for d in dirs.borrow().iter() {
                    if let Some(rest) = d.strip_prefix(&prefix) {
                        if !rest.is_empty() && !rest.contains('/') {
                            names.insert(rest.to_string());
                        }
                    }
                }
            });
            let entries: Vec<Value> = names.into_iter().map(|n| Value::string(&n)).collect();
            Ok(Value::list(entries))
        }),
    );

    register(
        "file/mkdir",
        Box::new(|args: &[Value]| {
            if args.len() != 1 {
                return Err(SemaError::arity("file/mkdir", "1", args.len()));
            }
            let path = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
            let path = &normalize_path(path)?;
            VFS_DIRS.with(|dirs| {
                let mut set = dirs.borrow_mut();
                let mut current = String::new();
                for seg in path.strip_prefix('/').unwrap_or(path).split('/') {
                    current.push('/');
                    current.push_str(seg);
                    set.insert(current.clone());
                }
            });
            Ok(Value::nil())
        }),
    );

    register(
        "file/is-directory?",
        Box::new(|args: &[Value]| {
            if args.len() != 1 {
                return Err(SemaError::arity("file/is-directory?", "1", args.len()));
            }
            let path = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
            let path = &normalize_path(path)?;
            Ok(Value::bool(
                VFS_DIRS.with(|dirs| dirs.borrow().contains(path.as_str())),
            ))
        }),
    );

    register(
        "file/is-file?",
        Box::new(|args: &[Value]| {
            if args.len() != 1 {
                return Err(SemaError::arity("file/is-file?", "1", args.len()));
            }
            let path = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
            let path = &normalize_path(path)?;
            Ok(Value::bool(
                VFS.with(|vfs| vfs.borrow().contains_key(path.as_str())),
            ))
        }),
    );

    register(
        "file/is-symlink?",
        Box::new(|args: &[Value]| {
            if args.len() != 1 {
                return Err(SemaError::arity("file/is-symlink?", "1", args.len()));
            }
            let _path = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
            Ok(Value::bool(false))
        }),
    );

    register(
        "file/append",
        Box::new(|args: &[Value]| {
            if args.len() != 2 {
                return Err(SemaError::arity("file/append", "2", args.len()));
            }
            let path = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
            let path = &normalize_path(path)?;
            let content = args[1]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
            let combined_len = VFS
                .with(|vfs| vfs.borrow().get(path.as_str()).map_or(0, |s| s.len()))
                + content.len();
            vfs_check_quota(path, combined_len)?;
            VFS.with(|vfs| {
                let mut map = vfs.borrow_mut();
                map.entry(path.to_string())
                    .and_modify(|existing| existing.push_str(content))
                    .or_insert_with(|| content.to_string());
            });
            VFS_TOTAL_BYTES.with(|t| t.set(t.get() + content.len()));
            Ok(Value::nil())
        }),
    );

    register(
        "file/copy",
        Box::new(|args: &[Value]| {
            if args.len() != 2 {
                return Err(SemaError::arity("file/copy", "2", args.len()));
            }
            let src = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
            let src = &normalize_path(src)?;
            let dest = args[1]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
            let dest = &normalize_path(dest)?;
            VFS.with(|vfs| {
                let map = vfs.borrow();
                match map.get(src.as_str()) {
                    Some(content) => {
                        let content = content.clone();
                        drop(map);
                        vfs_check_quota(dest, content.len())?;
                        let mut map = vfs.borrow_mut();
                        let old_len = map.get(dest.as_str()).map_or(0, |s| s.len());
                        map.insert(dest.to_string(), content.clone());
                        VFS_TOTAL_BYTES.with(|t| {
                            t.set(
                                t.get()
                                    .saturating_add(content.len())
                                    .saturating_sub(old_len),
                            );
                        });
                        Ok(Value::nil())
                    }
                    None => Err(SemaError::Io(format!(
                        "file/copy {src} -> {dest}: No such file"
                    ))),
                }
            })
        }),
    );

    register(
        "file/read-lines",
        Box::new(|args: &[Value]| {
            if args.len() != 1 {
                return Err(SemaError::arity("file/read-lines", "1", args.len()));
            }
            let path = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
            let path = &normalize_path(path)?;
            VFS.with(|vfs| match vfs.borrow().get(path.as_str()) {
                Some(content) => {
                    let lines: Vec<Value> = content.split('\n').map(Value::string).collect();
                    Ok(Value::list(lines))
                }
                None => Err(SemaError::Io(format!(
                    "file/read-lines {path}: No such file"
                ))),
            })
        }),
    );

    register(
        "file/write-lines",
        Box::new(|args: &[Value]| {
            if args.len() != 2 {
                return Err(SemaError::arity("file/write-lines", "2", args.len()));
            }
            let path = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
            let path = &normalize_path(path)?;
            let lines = if let Some(l) = args[1].as_list() {
                l
            } else if let Some(v) = args[1].as_vector() {
                v
            } else {
                return Err(SemaError::type_error("list or vector", args[1].type_name()));
            };
            let strs: Vec<String> = lines
                .iter()
                .map(|v| {
                    if let Some(s) = v.as_str() {
                        s.to_string()
                    } else {
                        v.to_string()
                    }
                })
                .collect();
            let content = strs.join("\n");
            vfs_check_quota(path, content.len())?;
            let content_len = content.len();
            VFS.with(|vfs| {
                let mut map = vfs.borrow_mut();
                let old_len = map.get(path.as_str()).map_or(0, |s| s.len());
                map.insert(path.to_string(), content);
                VFS_TOTAL_BYTES.with(|t| {
                    t.set(t.get().saturating_add(content_len).saturating_sub(old_len));
                });
            });
            Ok(Value::nil())
        }),
    );

    register(
        "file/info",
        Box::new(|args: &[Value]| {
            if args.len() != 1 {
                return Err(SemaError::arity("file/info", "1", args.len()));
            }
            let path = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
            let path = &normalize_path(path)?;
            let is_file = VFS.with(|vfs| vfs.borrow().contains_key(path));
            let is_dir = VFS_DIRS.with(|dirs| dirs.borrow().contains(path));
            if !is_file && !is_dir {
                return Err(SemaError::Io(format!(
                    "file/info {path}: No such file or directory"
                )));
            }
            let size = if is_file {
                VFS.with(|vfs| vfs.borrow().get(path).map(|c| c.len() as i64).unwrap_or(0))
            } else {
                0
            };
            let mut map = BTreeMap::new();
            map.insert(Value::keyword("size"), Value::int(size));
            map.insert(Value::keyword("is-dir"), Value::bool(is_dir));
            map.insert(Value::keyword("is-file"), Value::bool(is_file));
            Ok(Value::map(map))
        }),
    );

    // --- IO shims unsupported in WASM ---

    register(
        "read-line",
        Box::new(|_args: &[Value]| Err(SemaError::eval("read-line not supported in WASM"))),
    );

    register(
        "read-stdin",
        Box::new(|_args: &[Value]| Err(SemaError::eval("read-stdin not supported in WASM"))),
    );

    register(
        "shell",
        Box::new(|_args: &[Value]| Err(SemaError::eval("shell not supported in WASM"))),
    );

    // --- Reader/parser functions ---

    register(
        "load",
        Box::new(|args: &[Value]| {
            if args.len() != 1 {
                return Err(SemaError::arity("load", "1", args.len()));
            }
            let path = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
            VFS.with(|vfs| match vfs.borrow().get(path) {
                Some(content) => {
                    let exprs = sema_reader::read_many(content)?;
                    Ok(Value::list(exprs))
                }
                None => Err(SemaError::Io(format!("load {path}: No such file"))),
            })
        }),
    );

    register(
        "read",
        Box::new(|args: &[Value]| {
            if args.len() != 1 {
                return Err(SemaError::arity("read", "1", args.len()));
            }
            let s = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
            sema_reader::read(s)
        }),
    );

    register(
        "read-many",
        Box::new(|args: &[Value]| {
            if args.len() != 1 {
                return Err(SemaError::arity("read-many", "1", args.len()));
            }
            let s = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
            let exprs = sema_reader::read_many(s)?;
            Ok(Value::list(exprs))
        }),
    );

    register(
        "error",
        Box::new(|args: &[Value]| {
            if args.is_empty() {
                return Err(SemaError::eval("error called with no message"));
            }
            let msg = if let Some(s) = args[0].as_str() {
                s.to_string()
            } else {
                args[0].to_string()
            };
            Err(SemaError::eval(msg))
        }),
    );
}

#[wasm_bindgen(js_name = SemaInterpreter)]
pub struct WasmInterpreter {
    inner: sema_eval::Interpreter,
    callback_handles: std::rc::Rc<RefCell<BTreeMap<u32, Value>>>,
    callback_ids_by_value: std::rc::Rc<RefCell<BTreeMap<u64, u32>>>,
    next_callback_id: std::rc::Rc<Cell<u32>>,
}

impl Default for WasmInterpreter {
    fn default() -> Self {
        Self::new()
    }
}

/// Summary of a loaded web archive: entry point, embedded file count, and the
/// optional `sema-version` / `build-target` / `build-timestamp` metadata.
type LoadedArchiveInfo = (
    String,
    usize,
    Option<String>,
    Option<String>,
    Option<String>,
);

#[wasm_bindgen(js_class = SemaInterpreter)]
impl WasmInterpreter {
    #[wasm_bindgen(constructor)]
    pub fn new() -> WasmInterpreter {
        let interp = sema_eval::Interpreter::new();

        // Override print/println/display with our buffer-based versions
        register_wasm_io(&interp.global_env);

        // Set eval step limit to prevent infinite loops from crashing the browser tab.
        // 10M steps is enough for complex examples but prevents runaway computation.
        interp.ctx.set_eval_step_limit(10_000_000);

        WasmInterpreter {
            inner: interp,
            callback_handles: std::rc::Rc::new(RefCell::new(BTreeMap::new())),
            callback_ids_by_value: std::rc::Rc::new(RefCell::new(BTreeMap::new())),
            next_callback_id: std::rc::Rc::new(Cell::new(1)),
        }
    }

    /// Evaluate code, returns JSON: {"value": "...", "output": ["...", ...], "error": null}
    /// or {"value": null, "output": [...], "error": "..."}
    pub fn eval(&self, code: &str) -> JsValue {
        OUTPUT.with(|o| o.borrow_mut().clear());
        LINE_BUF.with(|b| b.borrow_mut().clear());

        let json_str = match self.inner.eval_str_in_global(code) {
            Ok(val) => {
                let output = take_output();
                let val_str = if val.is_nil() {
                    "null".to_string()
                } else {
                    format!("\"{}\"", escape_json(&format!("{val}")))
                };
                format!(
                    "{{\"value\":{},\"output\":[{}],\"error\":null}}",
                    val_str,
                    output
                        .iter()
                        .map(|s| format!("\"{}\"", escape_json(s)))
                        .collect::<Vec<_>>()
                        .join(",")
                )
            }
            Err(e) => {
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
                format!(
                    "{{\"value\":null,\"output\":[{}],\"error\":\"{}\"}}",
                    output
                        .iter()
                        .map(|s| format!("\"{}\"", escape_json(s)))
                        .collect::<Vec<_>>()
                        .join(","),
                    escape_json(&err_str)
                )
            }
        };
        js_sys::JSON::parse(&json_str).unwrap_or(JsValue::NULL)
    }

    /// Evaluate in the global env so defines persist
    #[wasm_bindgen(js_name = evalGlobal)]
    pub fn eval_global(&self, code: &str) -> JsValue {
        OUTPUT.with(|o| o.borrow_mut().clear());
        LINE_BUF.with(|b| b.borrow_mut().clear());

        let json_str = match self.inner.eval_str_in_global(code) {
            Ok(val) => {
                let output = take_output();
                let val_str = if val.is_nil() {
                    "null".to_string()
                } else {
                    format!("\"{}\"", escape_json(&pretty_print(&val, 80)))
                };
                format!(
                    "{{\"value\":{},\"output\":[{}],\"error\":null}}",
                    val_str,
                    output
                        .iter()
                        .map(|s| format!("\"{}\"", escape_json(s)))
                        .collect::<Vec<_>>()
                        .join(",")
                )
            }
            Err(e) => {
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
                format!(
                    "{{\"value\":null,\"output\":[{}],\"error\":\"{}\"}}",
                    output
                        .iter()
                        .map(|s| format!("\"{}\"", escape_json(s)))
                        .collect::<Vec<_>>()
                        .join(","),
                    escape_json(&err_str)
                )
            }
        };
        js_sys::JSON::parse(&json_str).unwrap_or(JsValue::NULL)
    }

    /// Evaluate code via the bytecode VM, returns same JSON format as eval_global
    #[wasm_bindgen(js_name = evalVM)]
    pub fn eval_vm(&self, code: &str) -> JsValue {
        OUTPUT.with(|o| o.borrow_mut().clear());
        LINE_BUF.with(|b| b.borrow_mut().clear());

        let json_str = match self.inner.eval_str_compiled(code) {
            Ok(val) => {
                let output = take_output();
                let val_str = if val.is_nil() {
                    "null".to_string()
                } else {
                    format!("\"{}\"", escape_json(&pretty_print(&val, 80)))
                };
                format!(
                    "{{\"value\":{},\"output\":[{}],\"error\":null}}",
                    val_str,
                    output
                        .iter()
                        .map(|s| format!("\"{}\"", escape_json(s)))
                        .collect::<Vec<_>>()
                        .join(",")
                )
            }
            Err(e) => {
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
                format!(
                    "{{\"value\":null,\"output\":[{}],\"error\":\"{}\"}}",
                    output
                        .iter()
                        .map(|s| format!("\"{}\"", escape_json(s)))
                        .collect::<Vec<_>>()
                        .join(","),
                    escape_json(&err_str)
                )
            }
        };
        js_sys::JSON::parse(&json_str).unwrap_or(JsValue::NULL)
    }

    /// Evaluate code with async HTTP support in the persistent global env
    /// (top-level defines persist across calls). Runs on the bytecode VM.
    #[wasm_bindgen(js_name = evalAsync)]
    pub async fn eval_async(&self, code: &str) -> JsValue {
        clear_http_cache();

        for _ in 0..MAX_REPLAYS {
            OUTPUT.with(|o| o.borrow_mut().clear());
            LINE_BUF.with(|b| b.borrow_mut().clear());

            match self.inner.eval_str_in_global(code) {
                Ok(val) => {
                    let output = take_output();
                    let val_str = if val.is_nil() {
                        "null".to_string()
                    } else {
                        format!("\"{}\"", escape_json(&pretty_print(&val, 80)))
                    };
                    let json_str = format!(
                        "{{\"value\":{},\"output\":[{}],\"error\":null}}",
                        val_str,
                        output
                            .iter()
                            .map(|s| format!("\"{}\"", escape_json(s)))
                            .collect::<Vec<_>>()
                            .join(",")
                    );
                    return js_sys::JSON::parse(&json_str).unwrap_or(JsValue::NULL);
                }
                Err(e) => {
                    if is_http_await_marker(&e) {
                        if let Some(json_str) = parse_http_marker(&e) {
                            match perform_fetch_from_marker(&json_str).await {
                                Ok((key, response)) => {
                                    HTTP_CACHE.with(|c| {
                                        c.borrow_mut().insert(key, response);
                                    });
                                    continue;
                                }
                                Err(fetch_err) => {
                                    let output = take_output();
                                    let err_str = format!("{}", fetch_err.inner());
                                    let json_str = format!(
                                        "{{\"value\":null,\"output\":[{}],\"error\":\"{}\"}}",
                                        output
                                            .iter()
                                            .map(|s| format!("\"{}\"", escape_json(s)))
                                            .collect::<Vec<_>>()
                                            .join(","),
                                        escape_json(&err_str)
                                    );
                                    return js_sys::JSON::parse(&json_str).unwrap_or(JsValue::NULL);
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
                    let json_str = format!(
                        "{{\"value\":null,\"output\":[{}],\"error\":\"{}\"}}",
                        output
                            .iter()
                            .map(|s| format!("\"{}\"", escape_json(s)))
                            .collect::<Vec<_>>()
                            .join(","),
                        escape_json(&err_str)
                    );
                    return js_sys::JSON::parse(&json_str).unwrap_or(JsValue::NULL);
                }
            }
        }

        let json_str = format!(
            "{{\"value\":null,\"output\":[],\"error\":\"{}\"}}",
            escape_json("exceeded maximum number of HTTP requests (50)")
        );
        js_sys::JSON::parse(&json_str).unwrap_or(JsValue::NULL)
    }

    /// Evaluate code with async HTTP support (bytecode VM)
    #[wasm_bindgen(js_name = evalVMAsync)]
    pub async fn eval_vm_async(&self, code: &str) -> JsValue {
        clear_http_cache();

        for _ in 0..MAX_REPLAYS {
            OUTPUT.with(|o| o.borrow_mut().clear());
            LINE_BUF.with(|b| b.borrow_mut().clear());

            match self.inner.eval_str_compiled(code) {
                Ok(val) => {
                    let output = take_output();
                    let val_str = if val.is_nil() {
                        "null".to_string()
                    } else {
                        format!("\"{}\"", escape_json(&pretty_print(&val, 80)))
                    };
                    let json_str = format!(
                        "{{\"value\":{},\"output\":[{}],\"error\":null}}",
                        val_str,
                        output
                            .iter()
                            .map(|s| format!("\"{}\"", escape_json(s)))
                            .collect::<Vec<_>>()
                            .join(",")
                    );
                    return js_sys::JSON::parse(&json_str).unwrap_or(JsValue::NULL);
                }
                Err(e) => {
                    if is_http_await_marker(&e) {
                        if let Some(json_str) = parse_http_marker(&e) {
                            match perform_fetch_from_marker(&json_str).await {
                                Ok((key, response)) => {
                                    HTTP_CACHE.with(|c| {
                                        c.borrow_mut().insert(key, response);
                                    });
                                    continue;
                                }
                                Err(fetch_err) => {
                                    let output = take_output();
                                    let err_str = format!("{}", fetch_err.inner());
                                    let json_str = format!(
                                        "{{\"value\":null,\"output\":[{}],\"error\":\"{}\"}}",
                                        output
                                            .iter()
                                            .map(|s| format!("\"{}\"", escape_json(s)))
                                            .collect::<Vec<_>>()
                                            .join(","),
                                        escape_json(&err_str)
                                    );
                                    return js_sys::JSON::parse(&json_str).unwrap_or(JsValue::NULL);
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
                    let json_str = format!(
                        "{{\"value\":null,\"output\":[{}],\"error\":\"{}\"}}",
                        output
                            .iter()
                            .map(|s| format!("\"{}\"", escape_json(s)))
                            .collect::<Vec<_>>()
                            .join(","),
                        escape_json(&err_str)
                    );
                    return js_sys::JSON::parse(&json_str).unwrap_or(JsValue::NULL);
                }
            }
        }

        let json_str = format!(
            "{{\"value\":null,\"output\":[],\"error\":\"{}\"}}",
            escape_json("exceeded maximum number of HTTP requests (50)")
        );
        js_sys::JSON::parse(&json_str).unwrap_or(JsValue::NULL)
    }

    /// Start a debug session. Compiles the code, sets breakpoints on given lines,
    /// and runs until the first stop or completion.
    /// Returns JSON: { status: "stopped"|"finished"|"error"|"http_needed", ... }
    #[wasm_bindgen(js_name = debugStart)]
    pub fn debug_start(&self, code: &str, breakpoint_lines: &js_sys::Array) -> JsValue {
        // The debugger always executes on the VM, so a `(load ...)` runs the
        // loaded body on the VM regardless of which eval the playground ran last.
        // End any existing session
        DEBUG_SESSION.with(|s| {
            *s.borrow_mut() = None;
        });

        OUTPUT.with(|o| o.borrow_mut().clear());
        LINE_BUF.with(|b| b.borrow_mut().clear());
        // Reset the loop-guard step counter so the limit is per debug session.
        self.inner.ctx.eval_steps.set(0);

        let bp_lines: Vec<u32> = breakpoint_lines
            .iter()
            .filter_map(|v| v.as_f64().map(|n| n as u32))
            .collect();

        // Parse
        let (exprs, spans) = match sema_reader::read_many_with_spans(code) {
            Ok(r) => r,
            Err(e) => return self.debug_error_result(&e),
        };
        self.inner.ctx.merge_span_table(spans);
        if exprs.is_empty() {
            return self.debug_finished_result(&sema_core::Value::nil());
        }

        // Expand macros
        let expanded: Vec<_> = match self.inner.expand_for_vm_batch(&exprs) {
            Ok(v) => v.into_iter().filter(|e| !e.is_nil()).collect(),
            Err(e) => return self.debug_error_result(&e),
        };
        if expanded.is_empty() {
            return self.debug_finished_result(&sema_core::Value::nil());
        }

        // Compile with spans and source file for breakpoint matching
        let source_file = std::path::PathBuf::from("<playground>");
        let span_map = self.inner.ctx.span_table.borrow().clone();
        let prog = match sema_vm::compile_program_with_spans(
            &expanded,
            &span_map,
            Some(source_file.clone()),
        ) {
            Ok(r) => r,
            Err(e) => return self.debug_error_result(&e),
        };

        // Extract valid breakpoint lines from compiled spans
        let valid_lines = sema_vm::valid_breakpoint_lines(&prog.closure, &prog.functions);

        // Snap requested breakpoints to valid lines
        let snapped_bp_lines: Vec<u32> = bp_lines
            .iter()
            .filter_map(|&line| sema_vm::snap_breakpoint_line(line, &valid_lines))
            .collect();

        let mut vm = match sema_vm::VM::new(
            self.inner.global_env.clone(),
            prog.functions,
            &[],
            prog.main_cache_slots,
        ) {
            Ok(vm) => vm,
            Err(e) => return JsValue::from_str(&format!("VM init error: {e}")),
        };

        // Register the async scheduler, exactly like the normal eval path
        // (run_exprs_on_vm) and the native DAP server do. Without this, debugging
        // a program that uses async/await/channels fails with "async/spawn: no
        // async scheduler registered".
        sema_vm::init_scheduler(self.inner.global_env.clone(), prog.native_table.clone());

        let mut debug = sema_vm::DebugState::new_headless();

        // Set snapped breakpoints
        if !snapped_bp_lines.is_empty() {
            debug.set_breakpoints(&source_file, &snapped_bp_lines);
        }

        // If breakpoints are set, run straight to the first one. With no
        // breakpoints, stop on entry so the user can step from the top
        // (otherwise Debug would behave identically to Run).
        debug.step_mode = if snapped_bp_lines.is_empty() {
            sema_vm::StepMode::StepInto
        } else {
            sema_vm::StepMode::Continue
        };
        debug.instructions_remaining = WASM_DEBUG_INSTRUCTION_BUDGET;

        // Helper: attach validLines and breakpoints to a debug response
        let attach_bp_info = |result: JsValue| -> JsValue {
            let valid_arr = js_sys::Array::new();
            for &l in &valid_lines {
                valid_arr.push(&JsValue::from_f64(l as f64));
            }
            let bp_arr = js_sys::Array::new();
            for &l in &snapped_bp_lines {
                bp_arr.push(&JsValue::from_f64(l as f64));
            }
            let _ = js_sys::Reflect::set(&result, &JsValue::from_str("validLines"), &valid_arr);
            let _ = js_sys::Reflect::set(&result, &JsValue::from_str("breakpoints"), &bp_arr);
            result
        };

        match vm.start_cooperative(prog.closure, &self.inner.ctx, &mut debug) {
            Ok(sema_vm::VmExecResult::Stopped(info)) => {
                let result = attach_bp_info(self.debug_stopped_result(&info));
                DEBUG_SESSION.with(|s| {
                    *s.borrow_mut() = Some(DebugSession { vm, debug });
                });
                result
            }
            Ok(sema_vm::VmExecResult::Yielded) => {
                let result = attach_bp_info(self.debug_yielded_result());
                DEBUG_SESSION.with(|s| {
                    *s.borrow_mut() = Some(DebugSession { vm, debug });
                });
                result
            }
            Ok(sema_vm::VmExecResult::Finished(v)) => {
                attach_bp_info(self.debug_finished_result(&v))
            }
            Ok(sema_vm::VmExecResult::AsyncYield(_)) => attach_bp_info(self.debug_yielded_result()),
            Err(e) => self.debug_maybe_http_error(&e),
        }
    }

    /// Perform an HTTP fetch from a debug marker and cache the result.
    /// Called by JS in response to a "http_needed" status.
    /// Takes the marker JSON from the request field. Returns true on success.
    #[wasm_bindgen(js_name = debugPerformFetch)]
    pub async fn debug_perform_fetch(&self, marker_json: &str) -> bool {
        match perform_fetch_from_marker(marker_json).await {
            Ok((key, response)) => {
                HTTP_CACHE.with(|c| {
                    c.borrow_mut().insert(key, response);
                });
                true
            }
            Err(_) => false,
        }
    }

    #[wasm_bindgen(js_name = debugContinue)]
    pub fn debug_continue(&self) -> JsValue {
        self.debug_resume(sema_vm::StepMode::Continue)
    }

    #[wasm_bindgen(js_name = debugStepInto)]
    pub fn debug_step_into(&self) -> JsValue {
        self.debug_resume(sema_vm::StepMode::StepInto)
    }

    #[wasm_bindgen(js_name = debugStepOver)]
    pub fn debug_step_over(&self) -> JsValue {
        self.debug_resume(sema_vm::StepMode::StepOver)
    }

    #[wasm_bindgen(js_name = debugStepOut)]
    pub fn debug_step_out(&self) -> JsValue {
        self.debug_resume(sema_vm::StepMode::StepOut)
    }

    #[wasm_bindgen(js_name = debugPoll)]
    pub fn debug_poll(&self) -> JsValue {
        DEBUG_SESSION.with(|s| {
            let mut session = s.borrow_mut();
            let Some(ref mut sess) = *session else {
                return self.debug_error_str("No active debug session");
            };

            sess.debug.instructions_remaining = WASM_DEBUG_INSTRUCTION_BUDGET;

            match sess.vm.run_cooperative(&self.inner.ctx, &mut sess.debug) {
                Ok(sema_vm::VmExecResult::Stopped(info)) => self.debug_stopped_result(&info),
                Ok(sema_vm::VmExecResult::Yielded) => self.debug_yielded_result(),
                Ok(sema_vm::VmExecResult::AsyncYield(_)) => self.debug_yielded_result(),
                Ok(sema_vm::VmExecResult::Finished(v)) => {
                    let result = self.debug_finished_result(&v);
                    *session = None;
                    result
                }
                Err(e) => {
                    let result = self.debug_maybe_http_error(&e);
                    *session = None;
                    result
                }
            }
        })
    }

    #[wasm_bindgen(js_name = debugStop)]
    pub fn debug_stop(&self) {
        DEBUG_SESSION.with(|s| {
            *s.borrow_mut() = None;
        });
    }

    #[wasm_bindgen(js_name = debugGetLocals)]
    pub fn debug_get_locals(&self) -> JsValue {
        DEBUG_SESSION.with(|s| {
            let mut session = s.borrow_mut();
            let Some(ref mut sess) = *session else {
                return JsValue::NULL;
            };
            // When paused at a breakpoint INSIDE an async task, inspect that
            // task's per-task VM (its frames hold the task-locals); the main VM
            // is parked at the `await`. Falls back to the main VM for ordinary
            // synchronous stops.
            let locals = sema_vm::with_coop_paused_task_vm(|tvm| {
                let frame_idx = tvm.frame_count().saturating_sub(1);
                tvm.debug_locals(frame_idx)
            })
            .unwrap_or_else(|| {
                let frame_idx = sess.vm.frame_count().saturating_sub(1);
                sess.vm.debug_locals(frame_idx)
            });
            let arr = js_sys::Array::new();
            for var in &locals {
                let obj = js_sys::Object::new();
                let _ = js_sys::Reflect::set(&obj, &"name".into(), &JsValue::from_str(&var.name));
                let _ = js_sys::Reflect::set(&obj, &"value".into(), &JsValue::from_str(&var.value));
                let _ =
                    js_sys::Reflect::set(&obj, &"type".into(), &JsValue::from_str(&var.type_name));
                arr.push(&obj);
            }
            arr.into()
        })
    }

    #[wasm_bindgen(js_name = debugGetStackTrace)]
    pub fn debug_get_stack_trace(&self) -> JsValue {
        DEBUG_SESSION.with(|s| {
            let session = s.borrow();
            let Some(ref sess) = *session else {
                return js_sys::Array::new().into();
            };
            // Mirror debug_get_locals: at an async stop, show the PAUSED TASK's
            // call stack, not the main VM's (which is parked at the `await`).
            let frames = sema_vm::with_coop_paused_task_vm(|tvm| tvm.debug_stack_trace())
                .unwrap_or_else(|| sess.vm.debug_stack_trace());
            let arr = js_sys::Array::new();
            for frame in &frames {
                let obj = js_sys::Object::new();
                let _ = js_sys::Reflect::set(&obj, &"name".into(), &JsValue::from_str(&frame.name));
                let _ = js_sys::Reflect::set(
                    &obj,
                    &"line".into(),
                    &JsValue::from_f64(frame.line as f64),
                );
                let _ = js_sys::Reflect::set(
                    &obj,
                    &"column".into(),
                    &JsValue::from_f64(frame.column as f64),
                );
                arr.push(&obj);
            }
            arr.into()
        })
    }

    /// Compile code and return the set of lines that are valid breakpoint targets.
    /// Returns a JS array of line numbers (sorted). Returns empty array on parse/compile error.
    #[wasm_bindgen(js_name = getValidBreakpointLines)]
    pub fn get_valid_breakpoint_lines(&self, code: &str) -> js_sys::Array {
        let result = js_sys::Array::new();

        let (exprs, spans) = match sema_reader::read_many_with_spans(code) {
            Ok(r) => r,
            Err(_) => return result,
        };
        self.inner.ctx.merge_span_table(spans);
        if exprs.is_empty() {
            return result;
        }

        let expanded: Vec<_> = match self.inner.expand_for_vm_batch(&exprs) {
            Ok(v) => v.into_iter().filter(|e| !e.is_nil()).collect(),
            Err(_) => return result,
        };
        if expanded.is_empty() {
            return result;
        }

        let source_file = std::path::PathBuf::from("<playground>");
        let span_map = self.inner.ctx.span_table.borrow().clone();
        let prog =
            match sema_vm::compile_program_with_spans(&expanded, &span_map, Some(source_file)) {
                Ok(r) => r,
                Err(_) => return result,
            };

        for line in sema_vm::valid_breakpoint_lines(&prog.closure, &prog.functions) {
            result.push(&JsValue::from_f64(line as f64));
        }
        result
    }

    #[wasm_bindgen(js_name = debugSetBreakpoints)]
    pub fn debug_set_breakpoints(&self, lines: &js_sys::Array) {
        let bp_lines: Vec<u32> = lines
            .iter()
            .filter_map(|v| v.as_f64().map(|n| n as u32))
            .collect();
        DEBUG_SESSION.with(|s| {
            if let Some(ref mut sess) = *s.borrow_mut() {
                let file = std::path::PathBuf::from("<playground>");
                sess.debug.set_breakpoints(&file, &bp_lines);
            }
        });
    }

    #[wasm_bindgen(js_name = debugIsActive)]
    pub fn debug_is_active(&self) -> bool {
        DEBUG_SESSION.with(|s| s.borrow().is_some())
    }

    /// Create interpreter with options: {stdlib: false, deny: ["network", "fs-write"]}
    #[wasm_bindgen(js_name = createWithOptions)]
    pub fn new_with_options(opts: JsValue) -> WasmInterpreter {
        let with_stdlib = js_sys::Reflect::get(&opts, &JsValue::from_str("stdlib"))
            .ok()
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let interp = if with_stdlib {
            sema_eval::Interpreter::new()
        } else {
            use std::rc::Rc;
            let env = sema_core::Env::new();
            let ctx = sema_core::EvalContext::new();
            sema_core::set_eval_callback(&ctx, sema_eval::eval_value_vm);
            sema_core::set_call_callback(&ctx, sema_eval::call_value);
            sema_core::set_call_owned_callback(&ctx, sema_eval::call_value_owned);
            let global_env = Rc::new(env);
            sema_eval::Interpreter { global_env, ctx }
        };

        register_wasm_io(&interp.global_env);
        interp.ctx.set_eval_step_limit(10_000_000);

        // Apply deny list: overwrite denied functions with PermissionDenied stubs
        if let Ok(deny_val) = js_sys::Reflect::get(&opts, &JsValue::from_str("deny")) {
            if let Some(deny_arr) = deny_val.dyn_ref::<js_sys::Array>() {
                let mut denied_caps: Vec<String> = Vec::new();
                for i in 0..deny_arr.length() {
                    if let Some(s) = deny_arr.get(i).as_string() {
                        denied_caps.push(s);
                    }
                }

                let deny_fns = |env: &Env, cap: &str, fn_names: &[&str]| {
                    for &name in fn_names {
                        let cap_name = cap.to_string();
                        let fn_name = name.to_string();
                        env.set(
                            sema_core::intern(name),
                            Value::native_fn(NativeFn::simple(name, move |_args| {
                                Err(SemaError::PermissionDenied {
                                    function: fn_name.clone(),
                                    capability: cap_name.clone(),
                                })
                            })),
                        );
                    }
                };

                for cap in &denied_caps {
                    match cap.as_str() {
                        "network" => deny_fns(
                            &interp.global_env,
                            "network",
                            &[
                                "http/get",
                                "http/post",
                                "http/put",
                                "http/delete",
                                "http/request",
                            ],
                        ),
                        "fs-read" => deny_fns(
                            &interp.global_env,
                            "fs-read",
                            &[
                                "file/read",
                                "file/exists?",
                                "file/list",
                                "file/is-directory?",
                                "file/is-file?",
                                "file/is-symlink?",
                            ],
                        ),
                        "fs-write" => deny_fns(
                            &interp.global_env,
                            "fs-write",
                            &[
                                "file/write",
                                "file/delete",
                                "file/rename",
                                "file/mkdir",
                                "file/append",
                            ],
                        ),
                        _ => {} // Unknown caps are silently ignored
                    }
                }
            }
        }

        WasmInterpreter {
            inner: interp,
            callback_handles: std::rc::Rc::new(RefCell::new(BTreeMap::new())),
            callback_ids_by_value: std::rc::Rc::new(RefCell::new(BTreeMap::new())),
            next_callback_id: std::rc::Rc::new(Cell::new(1)),
        }
    }

    /// Register a JavaScript function callable from Sema code.
    #[wasm_bindgen(js_name = registerFunction)]
    pub fn register_fn(&self, name: &str, callback: &js_sys::Function) {
        use sema_core::{NativeFn, SemaError, Value};

        let callback = callback.clone();
        let fn_name = name.to_string();
        let callback_handles = self.callback_handles.clone();
        let callback_ids_by_value = self.callback_ids_by_value.clone();
        let next_callback_id = self.next_callback_id.clone();

        let native = NativeFn::simple(&fn_name, move |args: &[Value]| {
            // Pass native JS values
            let js_array = js_sys::Array::new();
            for arg in args {
                js_array.push(&sema_value_to_jsvalue_with_callbacks(
                    arg,
                    &callback_handles,
                    &callback_ids_by_value,
                    &next_callback_id,
                ));
            }

            let result = callback.apply(&JsValue::NULL, &js_array).map_err(|e| {
                let msg = e.as_string().unwrap_or_else(|| format!("{:?}", e));
                SemaError::eval(format!("JS callback error: {msg}"))
            })?;

            // Convert JS result back to Sema value
            js_value_to_sema_value_with_callbacks(&result, &callback_handles)
                .map_err(|e| SemaError::eval(e.as_string().unwrap_or_else(|| format!("{:?}", e))))
        });

        self.inner
            .global_env
            .set_str(name, Value::native_fn(native));
    }

    /// Invoke a named global function directly with JS arguments.
    ///
    /// This avoids reparsing source strings and works for functions
    /// installed in the global environment.
    #[wasm_bindgen(js_name = invokeGlobal)]
    pub fn invoke_global(&self, name: &str, args: &js_sys::Array) -> Result<JsValue, JsValue> {
        let spur = sema_core::intern(name);
        let func = self
            .inner
            .global_env
            .get(spur)
            .ok_or_else(|| JsValue::from_str(&format!("Unbound variable: {name}")))?;

        let mut sema_args = Vec::with_capacity(args.length() as usize);
        for arg in args.iter() {
            sema_args.push(js_value_to_sema_value_with_callbacks(
                &arg,
                &self.callback_handles,
            )?);
        }

        match sema_eval::call_value(&self.inner.ctx, &func, &sema_args) {
            Ok(val) => Ok(sema_value_to_jsvalue_with_callbacks(
                &val,
                &self.callback_handles,
                &self.callback_ids_by_value,
                &self.next_callback_id,
            )),
            Err(e) => Err(JsValue::from_str(&format!("{}", e.inner()))),
        }
    }

    /// Invoke a stored callback handle directly with JS arguments.
    #[wasm_bindgen(js_name = invokeCallback)]
    pub fn invoke_callback(
        &self,
        callback_id: u32,
        args: &js_sys::Array,
    ) -> Result<JsValue, JsValue> {
        let func = self
            .callback_handles
            .borrow()
            .get(&callback_id)
            .cloned()
            .ok_or_else(|| JsValue::from_str(&format!("Unknown callback handle: {callback_id}")))?;

        let mut sema_args = Vec::with_capacity(args.length() as usize);
        for arg in args.iter() {
            sema_args.push(js_value_to_sema_value_with_callbacks(
                &arg,
                &self.callback_handles,
            )?);
        }

        match sema_eval::call_value(&self.inner.ctx, &func, &sema_args) {
            Ok(val) => Ok(sema_value_to_jsvalue_with_callbacks(
                &val,
                &self.callback_handles,
                &self.callback_ids_by_value,
                &self.next_callback_id,
            )),
            Err(e) => Err(JsValue::from_str(&format!("{}", e.inner()))),
        }
    }

    /// Release a callback handle that was materialized for JS.
    #[wasm_bindgen(js_name = releaseCallback)]
    pub fn release_callback(&self, callback_id: u32) {
        if let Some(value) = self.callback_handles.borrow_mut().remove(&callback_id) {
            self.callback_ids_by_value
                .borrow_mut()
                .remove(&value.raw_bits());
        }
    }

    /// Inject a virtual module so that `(import "name")` resolves without a file.
    #[wasm_bindgen(js_name = preloadModule)]
    pub fn preload_module(&self, name: &str, source: &str) -> JsValue {
        use sema_core::{intern, resolve, Env, SemaError, Value};
        use std::collections::BTreeMap;

        let result = (|| -> Result<(), SemaError> {
            let (exprs, spans) = sema_reader::read_many_with_spans(source)
                .map_err(|e| SemaError::eval(format!("{e}")))?;
            self.inner.ctx.merge_span_table(spans);

            let module_env = std::rc::Rc::new(Env::with_parent(self.inner.global_env.clone()));
            self.inner.ctx.clear_module_exports();

            let empty_spans = std::collections::HashMap::new();
            let eval_result = sema_eval::eval_module_body_vm(
                &self.inner.ctx,
                &module_env,
                &exprs,
                &empty_spans,
                None,
            );

            let declared = self.inner.ctx.take_module_exports();
            eval_result?;
            let exports: BTreeMap<String, Value> = match declared {
                Some(names) => names
                    .iter()
                    .filter_map(|n| {
                        let spur = intern(n);
                        module_env.get_local(spur).map(|v| (n.clone(), v))
                    })
                    .collect(),
                None => {
                    let mut map = BTreeMap::new();
                    module_env.iter_bindings(|k, v| {
                        map.insert(resolve(k), v.clone());
                    });
                    map
                }
            };

            self.inner
                .ctx
                .cache_module(std::path::PathBuf::from(name), exports);
            Ok(())
        })();

        let json_str = match result {
            Ok(()) => r#"{"ok":true,"error":null}"#.to_string(),
            Err(e) => format!(
                r#"{{"ok":false,"error":"{}"}}"#,
                escape_json(&format!("{}", e.inner()))
            ),
        };
        js_sys::JSON::parse(&json_str).unwrap_or(JsValue::NULL)
    }

    /// Load a compiled web archive into the interpreter's embedded module table.
    #[wasm_bindgen(js_name = loadArchive)]
    pub fn load_archive(&self, archive_bytes: &[u8]) -> JsValue {
        let result = (|| -> Result<LoadedArchiveInfo, SemaError> {
            let archive = sema_core::archive::deserialize_archive_from_bytes(archive_bytes)
                .map_err(|e| SemaError::eval(format!("invalid archive: {e}")))?;

            let entry_point = Self::archive_metadata_value(&archive.metadata, "entry-point")
                .unwrap_or_else(|| "__main__.semac".to_string());
            let sema_version = Self::archive_metadata_value(&archive.metadata, "sema-version");
            let build_target = Self::archive_metadata_value(&archive.metadata, "build-target");
            let build_timestamp =
                Self::archive_metadata_value(&archive.metadata, "build-timestamp");

            Self::validate_web_archive_metadata(
                sema_version.as_deref(),
                build_target.as_deref(),
                &entry_point,
                archive.files.contains_key(&entry_point),
            )?;

            self.inner.ctx.clear_module_cache();
            self.inner.ctx.clear_embedded_files();
            for (path, bytes) in archive.files {
                self.inner
                    .ctx
                    .set_embedded_file(std::path::PathBuf::from(path), bytes);
            }

            Ok((
                entry_point,
                self.inner.ctx.embedded_files.borrow().len(),
                sema_version,
                build_target,
                build_timestamp,
            ))
        })();

        let json_str = match result {
            Ok((entry_point, file_count, sema_version, build_target, build_timestamp)) => format!(
                "{{\"ok\":true,\"entryPoint\":\"{}\",\"fileCount\":{},\"semaVersion\":{},\"buildTarget\":{},\"buildTimestamp\":{},\"error\":null}}",
                escape_json(&entry_point),
                file_count,
                Self::json_opt_str(sema_version.as_deref()),
                Self::json_opt_str(build_target.as_deref()),
                Self::json_opt_str(build_timestamp.as_deref()),
            ),
            Err(e) => format!(
                "{{\"ok\":false,\"entryPoint\":null,\"fileCount\":0,\"semaVersion\":null,\"buildTarget\":null,\"buildTimestamp\":null,\"error\":\"{}\"}}",
                escape_json(&format!("{}", e.inner()))
            ),
        };
        js_sys::JSON::parse(&json_str).unwrap_or(JsValue::NULL)
    }

    fn archive_metadata_value(
        metadata: &std::collections::HashMap<String, Vec<u8>>,
        key: &str,
    ) -> Option<String> {
        metadata
            .get(key)
            .and_then(|value| std::str::from_utf8(value).ok())
            .map(|value| value.to_string())
    }

    fn validate_web_archive_metadata(
        sema_version: Option<&str>,
        build_target: Option<&str>,
        entry_point: &str,
        has_entry_point: bool,
    ) -> Result<(), SemaError> {
        if let Some(target) = build_target {
            if target != "web" {
                return Err(SemaError::eval(format!(
                    "archive build target mismatch: expected web, got {target}"
                )));
            }
        }

        if let Some(version) = sema_version {
            let runtime_version = env!("CARGO_PKG_VERSION");
            if version != runtime_version {
                return Err(SemaError::eval(format!(
                    "archive version mismatch: built with Sema {version}, runtime is {runtime_version}"
                )));
            }
        }

        if !has_entry_point {
            return Err(SemaError::eval(format!(
                "archive entry point not found: {entry_point}"
            )));
        }

        Ok(())
    }

    fn json_opt_str(value: Option<&str>) -> String {
        match value {
            Some(value) => format!("\"{}\"", escape_json(value)),
            None => "null".to_string(),
        }
    }

    /// Execute an embedded archive entry path.
    #[wasm_bindgen(js_name = runEntry)]
    pub fn run_entry(&self, path: &str) -> JsValue {
        OUTPUT.with(|o| o.borrow_mut().clear());
        LINE_BUF.with(|b| b.borrow_mut().clear());

        match self.run_embedded_entry_result(path) {
            Ok(val) => self.eval_success_result(&val),
            Err(e) => self.eval_error_result(&e),
        }
    }

    /// Execute an embedded archive entry path with async HTTP replay support.
    #[wasm_bindgen(js_name = runEntryAsync)]
    pub async fn run_entry_async(&self, path: &str) -> JsValue {
        clear_http_cache();

        for _ in 0..MAX_REPLAYS {
            OUTPUT.with(|o| o.borrow_mut().clear());
            LINE_BUF.with(|b| b.borrow_mut().clear());

            match self.run_embedded_entry_result(path) {
                Ok(val) => return self.eval_success_result(&val),
                Err(e) => {
                    if is_http_await_marker(&e) {
                        if let Some(json_str) = parse_http_marker(&e) {
                            match perform_fetch_from_marker(&json_str).await {
                                Ok((key, response)) => {
                                    HTTP_CACHE.with(|c| {
                                        c.borrow_mut().insert(key, response);
                                    });
                                    continue;
                                }
                                Err(fetch_err) => return self.eval_error_result(&fetch_err),
                            }
                        }
                    }
                    return self.eval_error_result(&e);
                }
            }
        }

        let json_str = format!(
            "{{\"value\":null,\"output\":[],\"error\":\"{}\"}}",
            escape_json("exceeded maximum number of HTTP requests (50)")
        );
        js_sys::JSON::parse(&json_str).unwrap_or(JsValue::NULL)
    }

    /// Read a file from the virtual filesystem.
    #[wasm_bindgen(js_name = readFile)]
    pub fn read_file(&self, path: &str) -> JsValue {
        let path = match normalize_path(path) {
            Ok(p) => p,
            Err(_) => return JsValue::NULL,
        };
        VFS.with(|vfs| match vfs.borrow().get(&path) {
            Some(content) => JsValue::from_str(content),
            None => JsValue::NULL,
        })
    }

    /// Write a file to the virtual filesystem.
    #[wasm_bindgen(js_name = writeFile)]
    pub fn write_file(&self, path: &str, content: &str) -> JsValue {
        let path = match normalize_path(path) {
            Ok(p) => p,
            Err(e) => return JsValue::from_str(&format!("{}", e.inner())),
        };
        match vfs_check_quota(&path, content.len()) {
            Ok(()) => {
                VFS.with(|vfs| {
                    let mut map = vfs.borrow_mut();
                    let old_len = map.get(&path).map_or(0, |s| s.len());
                    map.insert(path.to_string(), content.to_string());
                    VFS_TOTAL_BYTES.with(|t| {
                        t.set(
                            t.get()
                                .saturating_add(content.len())
                                .saturating_sub(old_len),
                        );
                    });
                });
                JsValue::NULL
            }
            Err(e) => {
                let msg = format!("{}", e.inner());
                JsValue::from_str(&msg)
            }
        }
    }

    /// Delete a file from the virtual filesystem. Returns true if the file existed.
    #[wasm_bindgen(js_name = deleteFile)]
    pub fn delete_file(&self, path: &str) -> bool {
        let path = match normalize_path(path) {
            Ok(p) => p,
            Err(_) => return false,
        };
        VFS.with(|vfs| match vfs.borrow_mut().remove(&path) {
            Some(old) => {
                VFS_TOTAL_BYTES.with(|t| t.set(t.get().saturating_sub(old.len())));
                true
            }
            None => false,
        })
    }

    /// List files and directories in the given directory path.
    #[wasm_bindgen(js_name = listFiles)]
    pub fn list_files(&self, dir: &str) -> JsValue {
        let dir = match normalize_path(dir) {
            Ok(p) => p,
            Err(_) => return js_sys::Array::new().into(),
        };
        let prefix = if dir == "/" {
            "/".to_string()
        } else {
            format!("{dir}/")
        };
        let mut names = BTreeSet::new();
        VFS.with(|vfs| {
            for key in vfs.borrow().keys() {
                if let Some(rest) = key.strip_prefix(&prefix) {
                    if let Some(first) = rest.split('/').next() {
                        if !first.is_empty() {
                            names.insert(first.to_string());
                        }
                    }
                }
            }
        });
        VFS_DIRS.with(|dirs| {
            for d in dirs.borrow().iter() {
                if let Some(rest) = d.strip_prefix(&prefix) {
                    if let Some(first) = rest.split('/').next() {
                        if !first.is_empty() {
                            names.insert(first.to_string());
                        }
                    }
                }
            }
        });
        let arr = js_sys::Array::new();
        for name in names {
            arr.push(&JsValue::from_str(&name));
        }
        arr.into()
    }

    /// Check if a path exists in the virtual filesystem (file or directory).
    #[wasm_bindgen(js_name = fileExists)]
    pub fn file_exists(&self, path: &str) -> bool {
        let path = match normalize_path(path) {
            Ok(p) => p,
            Err(_) => return false,
        };
        let in_vfs = VFS.with(|vfs| vfs.borrow().contains_key(&path));
        let in_dirs = VFS_DIRS.with(|dirs| dirs.borrow().contains(&path));
        in_vfs || in_dirs
    }

    /// Create a directory in the virtual filesystem.
    pub fn mkdir(&self, path: &str) {
        let path = match normalize_path(path) {
            Ok(p) => p,
            Err(_) => return,
        };
        VFS_DIRS.with(|dirs| {
            let mut set = dirs.borrow_mut();
            let mut current = String::new();
            for seg in path.strip_prefix('/').unwrap_or(&path).split('/') {
                current.push('/');
                current.push_str(seg);
                set.insert(current.clone());
            }
        });
    }

    /// Check if a path is a directory in the virtual filesystem.
    #[wasm_bindgen(js_name = isDirectory)]
    pub fn is_directory(&self, path: &str) -> bool {
        let path = match normalize_path(path) {
            Ok(p) => p,
            Err(_) => return false,
        };
        VFS_DIRS.with(|dirs| dirs.borrow().contains(&path))
    }

    /// Get VFS usage statistics.
    #[wasm_bindgen(js_name = vfsStats)]
    pub fn vfs_stats(&self) -> JsValue {
        let file_count = VFS.with(|vfs| vfs.borrow().len());
        let total_bytes = VFS_TOTAL_BYTES.with(|t| t.get());
        let json_str = format!(
            "{{\"files\":{},\"bytes\":{},\"maxFiles\":{},\"maxBytes\":{},\"maxFileBytes\":{}}}",
            file_count, total_bytes, VFS_MAX_FILES, VFS_MAX_TOTAL_BYTES, VFS_MAX_FILE_BYTES
        );
        js_sys::JSON::parse(&json_str).unwrap_or(JsValue::NULL)
    }

    /// Clear all files and directories from the virtual filesystem.
    #[wasm_bindgen(js_name = resetVFS)]
    pub fn reset_vfs(&self) {
        VFS.with(|vfs| vfs.borrow_mut().clear());
        VFS_DIRS.with(|dirs| {
            let mut set = dirs.borrow_mut();
            set.clear();
            set.insert("/".to_string());
        });
        VFS_TOTAL_BYTES.with(|t| t.set(0));
    }

    /// Snapshot the entire VFS as a plain JS object `{ files: {path: content},
    /// dirs: [path] }` — structured-clonable across `postMessage`. Used by the
    /// playground to mirror the worker's VFS back to the main thread after each
    /// eval (and to seed the worker before one). See `loadVfs`.
    #[wasm_bindgen(js_name = dumpVfs)]
    pub fn dump_vfs(&self) -> JsValue {
        let obj = js_sys::Object::new();
        let files = js_sys::Object::new();
        VFS.with(|vfs| {
            for (k, v) in vfs.borrow().iter() {
                let _ = js_sys::Reflect::set(&files, &JsValue::from_str(k), &JsValue::from_str(v));
            }
        });
        let dirs = js_sys::Array::new();
        VFS_DIRS.with(|d| {
            for p in d.borrow().iter() {
                dirs.push(&JsValue::from_str(p));
            }
        });
        let _ = js_sys::Reflect::set(&obj, &JsValue::from_str("files"), &files);
        let _ = js_sys::Reflect::set(&obj, &JsValue::from_str("dirs"), &dirs);
        obj.into()
    }

    /// Replace the entire VFS from a snapshot produced by `dumpVfs`. Resets
    /// first, so the VFS exactly matches the snapshot.
    #[wasm_bindgen(js_name = loadVfs)]
    pub fn load_vfs(&self, snapshot: JsValue) {
        self.reset_vfs();
        if !snapshot.is_object() {
            return;
        }
        if let Ok(files) = js_sys::Reflect::get(&snapshot, &JsValue::from_str("files")) {
            if files.is_object() {
                let files_obj: js_sys::Object = files.unchecked_into();
                for key in js_sys::Object::keys(&files_obj).iter() {
                    let (Some(path), Ok(val)) =
                        (key.as_string(), js_sys::Reflect::get(&files_obj, &key))
                    else {
                        continue;
                    };
                    if let Some(content) = val.as_string() {
                        VFS_TOTAL_BYTES.with(|t| t.set(t.get() + content.len()));
                        VFS.with(|vfs| {
                            vfs.borrow_mut().insert(path, content);
                        });
                    }
                }
            }
        }
        if let Ok(dirs) = js_sys::Reflect::get(&snapshot, &JsValue::from_str("dirs")) {
            if let Ok(arr) = dirs.dyn_into::<js_sys::Array>() {
                VFS_DIRS.with(|d| {
                    let mut set = d.borrow_mut();
                    for item in arr.iter() {
                        if let Some(p) = item.as_string() {
                            set.insert(p);
                        }
                    }
                });
            }
        }
    }

    /// Get the Sema version
    pub fn version(&self) -> String {
        env!("CARGO_PKG_VERSION").to_string()
    }

    /// Enable real wall-clock `async/sleep` via `Atomics.wait` on the given
    /// control buffer. Call this once from a Web Worker (where blocking is
    /// allowed), passing an `Int32Array` over a `SharedArrayBuffer` shared with
    /// the main thread. After this, the scheduler's virtual-clock advances also
    /// block the worker for the real duration. Do NOT call on the main thread —
    /// `Atomics.wait` is illegal there; leaving it uninstalled keeps the
    /// instant virtual-clock behavior.
    #[wasm_bindgen(js_name = installAtomicsSleep)]
    pub fn install_atomics_sleep(&self, view: js_sys::Int32Array) {
        SLEEP_I32.with(|s| *s.borrow_mut() = Some(view));
        sema_core::set_blocking_sleep_callback(worker_atomics_sleep);
        // The same control buffer carries the cancel flag (slot 0): the VM loop
        // guard polls this so a Stop aborts a running program (incl. mid-sleep).
        sema_core::set_interrupt_callback(worker_check_interrupt);
    }

    /// Install a sink called with each completed output line as it is produced,
    /// so the Web Worker can stream `println` output to the main thread live
    /// (a long-running / sleeping program shows output as it happens). Pass a
    /// JS function `(line: string) => void`.
    #[wasm_bindgen(js_name = setOutputSink)]
    pub fn set_output_sink(&self, sink: js_sys::Function) {
        OUTPUT_SINK.with(|s| *s.borrow_mut() = Some(sink));
    }
}

impl WasmInterpreter {
    fn eval_success_result(&self, val: &sema_core::Value) -> JsValue {
        let output = take_output();
        let val_str = if val.is_nil() {
            "null".to_string()
        } else {
            format!("\"{}\"", escape_json(&pretty_print(val, 80)))
        };
        let json_str = format!(
            "{{\"value\":{},\"output\":[{}],\"error\":null}}",
            val_str,
            output
                .iter()
                .map(|s| format!("\"{}\"", escape_json(s)))
                .collect::<Vec<_>>()
                .join(",")
        );
        js_sys::JSON::parse(&json_str).unwrap_or(JsValue::NULL)
    }

    fn eval_error_result(&self, e: &sema_core::SemaError) -> JsValue {
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
        let json_str = format!(
            "{{\"value\":null,\"output\":[{}],\"error\":\"{}\"}}",
            output
                .iter()
                .map(|s| format!("\"{}\"", escape_json(s)))
                .collect::<Vec<_>>()
                .join(","),
            escape_json(&err_str)
        );
        js_sys::JSON::parse(&json_str).unwrap_or(JsValue::NULL)
    }

    fn run_embedded_entry_result(&self, path: &str) -> Result<Value, SemaError> {
        let entry_path = std::path::PathBuf::from(path);
        let bytes = self
            .inner
            .ctx
            .get_embedded_file(&entry_path)
            .ok_or_else(|| SemaError::eval(format!("embedded entry not found: {path}")))?;

        self.inner.ctx.push_file_path(entry_path.clone());
        let result = (|| {
            if sema_vm::is_bytecode_file(&bytes) {
                let compiled = sema_vm::deserialize_from_bytes(&bytes)?;
                sema_eval::execute_compile_result(
                    &self.inner.ctx,
                    self.inner.global_env.clone(),
                    compiled,
                )
            } else {
                let source = String::from_utf8(bytes).map_err(|e| {
                    SemaError::eval(format!("embedded entry is not valid UTF-8: {e}"))
                })?;
                let (exprs, spans) = sema_reader::read_many_with_spans(&source)?;
                self.inner.ctx.merge_span_table(spans.clone());
                sema_eval::eval_module_body_vm(
                    &self.inner.ctx,
                    &self.inner.global_env,
                    &exprs,
                    &spans,
                    Some(entry_path.clone()),
                )
            }
        })();
        self.inner.ctx.pop_file_path();
        result
    }

    fn debug_resume(&self, mode: sema_vm::StepMode) -> JsValue {
        DEBUG_SESSION.with(|s| {
            let mut session = s.borrow_mut();
            let Some(ref mut sess) = *session else {
                return self.debug_error_str("No active debug session");
            };

            sess.debug.step_mode = mode;
            if mode != sema_vm::StepMode::Continue {
                // Step depth must be measured against the VM that will actually be
                // stepped. At a stop INSIDE an async task the resume re-drives that
                // task's per-task VM (not the main VM, which is parked at the
                // await), so StepOver/StepOut depth comparisons must use the task's
                // frame count. Falls back to the main VM for ordinary sync stops.
                sess.debug.step_frame_depth =
                    sema_vm::with_coop_paused_task_vm(|tvm| tvm.frame_count())
                        .unwrap_or_else(|| sess.vm.frame_count());
            }
            sess.debug.instructions_remaining = WASM_DEBUG_INSTRUCTION_BUDGET;

            match sess.vm.run_cooperative(&self.inner.ctx, &mut sess.debug) {
                Ok(sema_vm::VmExecResult::Stopped(info)) => self.debug_stopped_result(&info),
                Ok(sema_vm::VmExecResult::Yielded) => self.debug_yielded_result(),
                Ok(sema_vm::VmExecResult::AsyncYield(_)) => self.debug_yielded_result(),
                Ok(sema_vm::VmExecResult::Finished(v)) => {
                    let result = self.debug_finished_result(&v);
                    *session = None;
                    result
                }
                Err(e) => {
                    let result = self.debug_maybe_http_error(&e);
                    *session = None;
                    result
                }
            }
        })
    }

    fn debug_stopped_result(&self, info: &sema_vm::StopInfo) -> JsValue {
        let output = take_output();
        let output_json = output
            .iter()
            .map(|s| format!("\"{}\"", escape_json(s)))
            .collect::<Vec<_>>()
            .join(",");
        let reason = match info.reason {
            sema_vm::StopReason::Breakpoint => "breakpoint",
            sema_vm::StopReason::Step => "step",
            sema_vm::StopReason::Pause => "pause",
            sema_vm::StopReason::Entry => "entry",
            sema_vm::StopReason::Exception => "exception",
        };
        let json_str = format!(
            "{{\"status\":\"stopped\",\"line\":{},\"reason\":\"{}\",\"output\":[{}]}}",
            info.line, reason, output_json,
        );
        js_sys::JSON::parse(&json_str).unwrap_or(JsValue::NULL)
    }

    fn debug_finished_result(&self, val: &sema_core::Value) -> JsValue {
        let output = take_output();
        let val_str = if val.is_nil() {
            "null".to_string()
        } else {
            format!("\"{}\"", escape_json(&sema_core::pretty_print(val, 80)))
        };
        let output_json = output
            .iter()
            .map(|s| format!("\"{}\"", escape_json(s)))
            .collect::<Vec<_>>()
            .join(",");
        let json_str = format!(
            "{{\"status\":\"finished\",\"value\":{},\"output\":[{}],\"error\":null}}",
            val_str, output_json,
        );
        js_sys::JSON::parse(&json_str).unwrap_or(JsValue::NULL)
    }

    fn debug_yielded_result(&self) -> JsValue {
        let output = take_output();
        let output_json = output
            .iter()
            .map(|s| format!("\"{}\"", escape_json(s)))
            .collect::<Vec<_>>()
            .join(",");
        let json_str = format!("{{\"status\":\"yielded\",\"output\":[{}]}}", output_json,);
        js_sys::JSON::parse(&json_str).unwrap_or(JsValue::NULL)
    }

    /// Check if an error is an HTTP await marker; if so, return an "http_needed"
    /// result so JS can perform the fetch and restart the debug session.
    fn debug_maybe_http_error(&self, e: &sema_core::SemaError) -> JsValue {
        if let Some(json_payload) = parse_http_marker(e) {
            return self.debug_http_needed_result(&json_payload);
        }
        self.debug_error_result(e)
    }

    fn debug_http_needed_result(&self, marker_json: &str) -> JsValue {
        let output = take_output();
        let output_json = output
            .iter()
            .map(|s| format!("\"{}\"", escape_json(s)))
            .collect::<Vec<_>>()
            .join(",");
        let json_str = format!(
            "{{\"status\":\"http_needed\",\"output\":[{}],\"request\":{}}}",
            output_json, marker_json,
        );
        js_sys::JSON::parse(&json_str).unwrap_or(JsValue::NULL)
    }

    fn debug_error_result(&self, e: &sema_core::SemaError) -> JsValue {
        let output = take_output();
        let mut err_str = format!("{}", e.inner());
        if let Some(trace) = e.stack_trace() {
            err_str.push_str(&format!("\n{trace}"));
        }
        if let Some(hint) = e.hint() {
            err_str.push_str(&format!("\n  hint: {hint}"));
        }
        let output_json = output
            .iter()
            .map(|s| format!("\"{}\"", escape_json(s)))
            .collect::<Vec<_>>()
            .join(",");
        let json_str = format!(
            "{{\"status\":\"error\",\"output\":[{}],\"error\":\"{}\"}}",
            output_json,
            escape_json(&err_str),
        );
        js_sys::JSON::parse(&json_str).unwrap_or(JsValue::NULL)
    }

    fn debug_error_str(&self, msg: &str) -> JsValue {
        let json_str = format!(
            "{{\"status\":\"error\",\"output\":[],\"error\":\"{}\"}}",
            escape_json(msg),
        );
        js_sys::JSON::parse(&json_str).unwrap_or(JsValue::NULL)
    }
}

const CALLBACK_HANDLE_KEY: &str = "__semaCallbackHandle";

fn callback_handle_from_js(value: &JsValue) -> Option<u32> {
    if !value.is_object() {
        return None;
    }
    let handle = js_sys::Reflect::get(value, &JsValue::from_str(CALLBACK_HANDLE_KEY)).ok()?;
    let n = handle.as_f64()?;
    if n.fract() == 0.0 && n >= 0.0 && n <= u32::MAX as f64 {
        Some(n as u32)
    } else {
        None
    }
}

fn make_callback_handle_object(callback_id: u32) -> JsValue {
    let obj = js_sys::Object::new();
    let _ = js_sys::Reflect::set(
        &obj,
        &JsValue::from_str(CALLBACK_HANDLE_KEY),
        &JsValue::from_f64(callback_id as f64),
    );
    obj.into()
}

fn allocate_callback_handle(
    val: &Value,
    callback_handles: &std::rc::Rc<RefCell<BTreeMap<u32, Value>>>,
    callback_ids_by_value: &std::rc::Rc<RefCell<BTreeMap<u64, u32>>>,
    next_callback_id: &std::rc::Rc<Cell<u32>>,
) -> u32 {
    let raw_bits = val.raw_bits();
    if let Some(existing) = callback_ids_by_value.borrow().get(&raw_bits).copied() {
        return existing;
    }

    let callback_id = next_callback_id.get();
    next_callback_id.set(callback_id.saturating_add(1));
    callback_handles
        .borrow_mut()
        .insert(callback_id, val.clone());
    callback_ids_by_value
        .borrow_mut()
        .insert(raw_bits, callback_id);
    callback_id
}

fn sema_value_to_jsvalue_with_callbacks(
    val: &Value,
    callback_handles: &std::rc::Rc<RefCell<BTreeMap<u32, Value>>>,
    callback_ids_by_value: &std::rc::Rc<RefCell<BTreeMap<u64, u32>>>,
    next_callback_id: &std::rc::Rc<Cell<u32>>,
) -> JsValue {
    match val.view() {
        ValueView::Nil => JsValue::NULL,
        ValueView::Bool(b) => JsValue::from_bool(b),
        ValueView::Int(n) => JsValue::from_f64(n as f64),
        ValueView::Float(f) => JsValue::from_f64(f),
        ValueView::String(s) => JsValue::from_str(&s),
        ValueView::Keyword(s) => JsValue::from_str(&format!(":{}", sema_core::resolve(s))),
        ValueView::Symbol(s) => JsValue::from_str(&sema_core::resolve(s)),
        ValueView::Lambda(_) | ValueView::NativeFn(_) | ValueView::MultiMethod(_) => {
            make_callback_handle_object(allocate_callback_handle(
                val,
                callback_handles,
                callback_ids_by_value,
                next_callback_id,
            ))
        }
        // Bytevectors cross as a Uint8Array so binary payloads (e.g. a bytevector
        // handed to ws/send) reach JS as bytes — not the "#u8(…)" string that the
        // JSON fallback would produce.
        ValueView::Bytevector(bytes) => {
            let arr = js_sys::Uint8Array::new_with_length(bytes.len() as u32);
            arr.copy_from(bytes.as_slice());
            arr.into()
        }
        _ => {
            // For complex types (lists, maps, vectors), go through JSON.
            // Use lossy conversion so NaN/Infinity become null locally
            // instead of stringifying the entire structure.
            let json = sema_core::value_to_json_lossy(val);
            let s = serde_json::to_string(&json).unwrap_or_default();
            js_sys::JSON::parse(&s).unwrap_or(JsValue::NULL)
        }
    }
}

fn js_value_to_sema_value_with_callbacks(
    value: &JsValue,
    callback_handles: &std::rc::Rc<RefCell<BTreeMap<u32, Value>>>,
) -> Result<Value, JsValue> {
    if let Some(callback_id) = callback_handle_from_js(value) {
        return callback_handles
            .borrow()
            .get(&callback_id)
            .cloned()
            .ok_or_else(|| JsValue::from_str(&format!("Unknown callback handle: {callback_id}")));
    }

    if value.is_undefined() || value.is_null() {
        return Ok(Value::nil());
    }

    if let Some(b) = value.as_bool() {
        return Ok(Value::bool(b));
    }

    if let Some(n) = value.as_f64() {
        if n.fract() == 0.0 && n >= i64::MIN as f64 && n <= i64::MAX as f64 {
            return Ok(Value::int(n as i64));
        }
        return Ok(Value::float(n));
    }

    if let Some(s) = value.as_string() {
        return Ok(Value::string(&s));
    }

    // Binary payloads (e.g. a WebSocket binary frame handed to a ws/listen
    // callback) arrive as a Uint8Array or ArrayBuffer. Preserve them as Sema
    // bytevectors — JSON-stringifying a Uint8Array would yield a `{"0":…}`
    // object, silently dropping the binary shape.
    if let Some(arr) = value.dyn_ref::<js_sys::Uint8Array>() {
        return Ok(Value::bytevector(arr.to_vec()));
    }
    if let Some(buf) = value.dyn_ref::<js_sys::ArrayBuffer>() {
        return Ok(Value::bytevector(js_sys::Uint8Array::new(buf).to_vec()));
    }

    let json = js_sys::JSON::stringify(value)
        .map_err(|e| JsValue::from_str(&format!("Failed to serialize JS value: {:?}", e)))?;
    let json_str = json
        .as_string()
        .ok_or_else(|| JsValue::from_str("Failed to convert JS value to JSON string"))?;
    let parsed: serde_json::Value = serde_json::from_str(&json_str)
        .map_err(|e| JsValue::from_str(&format!("Failed to parse JSON value: {e}")))?;
    Ok(sema_core::json_to_value(&parsed))
}

/// Format Sema source code. Returns JSON: {"formatted": "...", "error": null}
/// or {"formatted": null, "error": "..."}
#[wasm_bindgen(js_name = formatCode)]
pub fn format_code(code: &str, width: usize, indent: usize, align: bool) -> JsValue {
    let opts = sema_fmt::FormatOptions {
        width,
        indent,
        align,
    };
    match sema_fmt::format_source(code, &opts) {
        Ok(formatted) => {
            let json_str = format!(
                "{{\"formatted\":\"{}\",\"error\":null}}",
                escape_json(&formatted)
            );
            js_sys::JSON::parse(&json_str).unwrap_or(JsValue::NULL)
        }
        Err(e) => {
            let json_str = format!(
                "{{\"formatted\":null,\"error\":\"{}\"}}",
                escape_json(&format!("{}", e.inner()))
            );
            js_sys::JSON::parse(&json_str).unwrap_or(JsValue::NULL)
        }
    }
}

fn escape_json(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // Other C0 control characters must be \uXXXX-escaped; emitting them
            // raw produces invalid JSON that the browser parses as `null`.
            c if (c as u32) < 0x20 => {
                use std::fmt::Write;
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

/// Clamp a user-supplied timeout (milliseconds, u64) to a valid setTimeout
/// delay. A bare `as i32` wrapped values > ~2.1e9 ms to negative/zero, which
/// made the abort controller fire immediately and break the request.
fn clamp_timeout_ms(ms: u64) -> i32 {
    i32::try_from(ms).unwrap_or(i32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_timeout_ms_does_not_wrap_to_negative() {
        // ~ 3 billion ms is well past i32::MAX; old `as i32` wrapped negative.
        assert_eq!(clamp_timeout_ms(3_000_000_000), i32::MAX);
        assert_eq!(clamp_timeout_ms(u64::MAX), i32::MAX);
        assert_eq!(clamp_timeout_ms(5000), 5000);
        assert!(clamp_timeout_ms(3_000_000_000) > 0);
    }

    #[test]
    fn escape_json_handles_basic_escapes() {
        assert_eq!(escape_json("a\"b\\c\nd\re\tf"), "a\\\"b\\\\c\\nd\\re\\tf");
        assert_eq!(escape_json("plain"), "plain");
    }

    #[test]
    fn escape_json_escapes_c0_control_chars() {
        // WASM-3: control chars < 0x20 (other than \n \r \t) must become
        // \uXXXX escapes, otherwise the emitted JSON is invalid and parses as null.
        assert_eq!(escape_json("\u{0}"), "\\u0000");
        assert_eq!(escape_json("\u{1}\u{1f}"), "\\u0001\\u001f");
        // A bell + backspace + escape char interleaved with text.
        assert_eq!(
            escape_json("x\u{7}y\u{8}z\u{1b}"),
            "x\\u0007y\\u0008z\\u001b"
        );
        // The dedicated escapes are still preferred over the generic form.
        assert_eq!(escape_json("\t"), "\\t");
    }

    #[test]
    fn web_archive_metadata_rejects_wrong_build_target() {
        let err = WasmInterpreter::validate_web_archive_metadata(
            Some(env!("CARGO_PKG_VERSION")),
            Some("native"),
            "__main__.semac",
            true,
        )
        .unwrap_err();

        assert!(format!("{}", err.inner()).contains("build target mismatch"));
    }

    #[test]
    fn web_archive_metadata_rejects_version_mismatch() {
        let err = WasmInterpreter::validate_web_archive_metadata(
            Some("0.0.0"),
            Some("web"),
            "__main__.semac",
            true,
        )
        .unwrap_err();

        assert!(format!("{}", err.inner()).contains("archive version mismatch"));
    }

    #[test]
    fn web_archive_metadata_rejects_missing_entry_point() {
        let err = WasmInterpreter::validate_web_archive_metadata(
            Some(env!("CARGO_PKG_VERSION")),
            Some("web"),
            "__main__.semac",
            false,
        )
        .unwrap_err();

        assert!(format!("{}", err.inner()).contains("entry point not found"));
    }
}
