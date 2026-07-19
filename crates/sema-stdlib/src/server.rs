use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use sema_core::runtime::NativeOutcome;
use sema_core::{check_arity, value_to_json_lossy, SemaError, Value};

use crate::register_fn;

/// Decode `http/file`'s off-thread canonicalize+mime result into the `__file`
/// marker map on the VM thread. A plain `fn` (no captures) so it fits the
/// `fn(T) -> Value` decoder slot of `quarantined_compute`/`fs_offload`.
fn http_file_marker(resolved: (String, String)) -> Value {
    let (path_str, content_type) = resolved;
    let mut map = BTreeMap::new();
    map.insert(Value::keyword("__file"), Value::bool(true));
    map.insert(Value::keyword("__file_path"), Value::string(&path_str));
    map.insert(
        Value::keyword("__file_content_type"),
        Value::string(&content_type),
    );
    Value::map(map)
}

/// Decode a `:static`-route file request's off-thread canonicalize result into
/// the response the sync path builds: a 403 map on symlink escape, else the
/// `__file` marker map. Non-capturing `fn` for the `quarantined_compute`/
/// `fs_offload` decoder slot.
fn static_file_response(resolved: (bool, String, String)) -> Value {
    let (escapes, path_str, content_type) = resolved;
    if escapes {
        let mut headers = BTreeMap::new();
        headers.insert(Value::string("content-type"), Value::string("text/plain"));
        let mut result = BTreeMap::new();
        result.insert(Value::keyword("status"), Value::int(403));
        result.insert(Value::keyword("headers"), Value::map(headers));
        result.insert(Value::keyword("body"), Value::string("Forbidden"));
        return Value::map(result);
    }
    let mut map = BTreeMap::new();
    map.insert(Value::keyword("__file"), Value::bool(true));
    map.insert(Value::keyword("__file_path"), Value::string(&path_str));
    map.insert(
        Value::keyword("__file_content_type"),
        Value::string(&content_type),
    );
    Value::map(map)
}

fn value_to_json_lossy_string(val: &Value) -> Result<String, String> {
    serde_json::to_string(&value_to_json_lossy(val)).map_err(|e| e.to_string())
}

// --- Raw types for cross-thread communication (Value is !Send due to Rc) ---

/// Raw HTTP request data that is Send-safe for crossing thread boundaries.
struct RawRequest {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    query: Option<String>,
    body: String,
    content_type_is_json: bool,
}

/// Raw HTTP response data that is Send-safe for crossing thread boundaries.
struct RawResponse {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

/// The response type sent back from the evaluator thread to the axum handler thread.
/// Supports both normal HTTP responses and SSE streaming.
enum ServerResponse {
    /// A normal HTTP response.
    Raw(RawResponse),
    /// An SSE stream: the receiver yields event data strings. Unbounded so the
    /// producer's `send` never blocks — SSE handlers run on the evaluator thread
    /// and may be inside a provider's `block_on` (e.g. llm/stream), where a
    /// bounded `blocking_send` would panic ("block within a runtime").
    Sse(tokio::sync::mpsc::UnboundedReceiver<String>),
    /// A WebSocket connection: bidirectional channels for message passing.
    WebSocket {
        /// Sends messages from axum (client) to the evaluator (server handler).
        incoming_tx: tokio::sync::mpsc::Sender<WsMsg>,
        /// Publishes lossless readiness generations after incoming messages.
        incoming_generation: tokio::sync::watch::Sender<u64>,
        /// Receives messages from the evaluator (server handler) to axum (client).
        outgoing_rx: tokio::sync::mpsc::Receiver<WsMsg>,
    },
    /// A file to serve from disk (binary-safe, read on the axum/tokio side).
    File {
        path: std::path::PathBuf,
        content_type: String,
    },
}

/// A single WebSocket frame carried between the axum bridge and the evaluator's
/// server-side `:send`/`:recv`. Text frames surface to Sema as strings, binary
/// frames as bytevectors.
enum WsMsg {
    Text(String),
    Binary(Vec<u8>),
}

/// A server request sent from the axum handler thread to the main evaluator thread.
enum ServerRequest {
    Http {
        lifecycle: Arc<ServeRequestLifecycle>,
        raw: RawRequest,
        respond: tokio::sync::oneshot::Sender<ServerResponse>,
    },
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct ServeRequestId(u64);

static NEXT_SERVE_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

fn next_serve_request_id() -> ServeRequestId {
    ServeRequestId(NEXT_SERVE_REQUEST_ID.fetch_add(1, Ordering::Relaxed))
}

struct ServeRequestLifecycle {
    id: ServeRequestId,
    lifecycle_tx: tokio::sync::mpsc::UnboundedSender<Arc<Self>>,
    disconnected: AtomicBool,
    finished: AtomicBool,
}

impl ServeRequestLifecycle {
    fn new(
        id: ServeRequestId,
        lifecycle_tx: tokio::sync::mpsc::UnboundedSender<Arc<Self>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            id,
            lifecycle_tx,
            disconnected: AtomicBool::new(false),
            finished: AtomicBool::new(false),
        })
    }

    fn mark_disconnected(self: &Arc<Self>) {
        if !self.disconnected.swap(true, Ordering::AcqRel) {
            let _ = self.lifecycle_tx.send(Arc::clone(self));
        }
    }

    fn mark_finished(self: &Arc<Self>) {
        if !self.finished.swap(true, Ordering::AcqRel) {
            let _ = self.lifecycle_tx.send(Arc::clone(self));
        }
    }

    fn is_disconnected(&self) -> bool {
        self.disconnected.load(Ordering::Acquire)
    }

    fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }
}

struct RequestFutureLease(Option<Arc<ServeRequestLifecycle>>);

impl RequestFutureLease {
    fn new(lifecycle: Arc<ServeRequestLifecycle>) -> Self {
        Self(Some(lifecycle))
    }

    fn disarm(&mut self) {
        self.0.take();
    }
}

impl Drop for RequestFutureLease {
    fn drop(&mut self) {
        if let Some(lifecycle) = self.0.take() {
            lifecycle.mark_disconnected();
        }
    }
}

struct HandlerFinishedLease(Arc<ServeRequestLifecycle>);

impl Drop for HandlerFinishedLease {
    fn drop(&mut self) {
        self.0.mark_finished();
    }
}

/// Build a JSON response map: {:status N :headers {"content-type" "application/json"} :body json-string}
fn json_response(status: i64, val: &Value) -> Result<Value, SemaError> {
    let json = sema_core::value_to_json_lossy(val);
    let body = serde_json::to_string(&json)
        .map_err(|e| SemaError::eval(format!("http response: json encode: {e}")))?;

    let mut headers = BTreeMap::new();
    headers.insert(
        Value::string("content-type"),
        Value::string("application/json"),
    );

    let mut result = BTreeMap::new();
    result.insert(Value::keyword("status"), Value::int(status));
    result.insert(Value::keyword("headers"), Value::map(headers));
    result.insert(Value::keyword("body"), Value::string(&body));
    Ok(Value::map(result))
}

pub fn register(env: &sema_core::Env, sandbox: &sema_core::Sandbox) {
    register_fn(env, "http/ok", |args| {
        check_arity!(args, "http/ok", 1);
        json_response(200, &args[0])
    });

    register_fn(env, "http/created", |args| {
        check_arity!(args, "http/created", 1);
        json_response(201, &args[0])
    });

    register_fn(env, "http/no-content", |args| {
        check_arity!(args, "http/no-content", 0);
        let mut result = BTreeMap::new();
        result.insert(Value::keyword("status"), Value::int(204));
        result.insert(Value::keyword("headers"), Value::map(BTreeMap::new()));
        result.insert(Value::keyword("body"), Value::string(""));
        Ok(Value::map(result))
    });

    register_fn(env, "http/not-found", |args| {
        check_arity!(args, "http/not-found", 1);
        json_response(404, &args[0])
    });

    register_fn(env, "http/redirect", |args| {
        check_arity!(args, "http/redirect", 1);
        let url = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;

        let mut headers = BTreeMap::new();
        headers.insert(Value::string("location"), Value::string(url));

        let mut result = BTreeMap::new();
        result.insert(Value::keyword("status"), Value::int(302));
        result.insert(Value::keyword("headers"), Value::map(headers));
        result.insert(Value::keyword("body"), Value::string(""));
        Ok(Value::map(result))
    });

    register_fn(env, "http/error", |args| {
        check_arity!(args, "http/error", 2);
        let status = args[0]
            .as_int()
            .ok_or_else(|| SemaError::type_error("integer", args[0].type_name()))?;
        json_response(status, &args[1])
    });

    register_fn(env, "http/html", |args| {
        check_arity!(args, "http/html", 1);
        let content = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;

        let mut headers = BTreeMap::new();
        headers.insert(Value::string("content-type"), Value::string("text/html"));

        let mut result = BTreeMap::new();
        result.insert(Value::keyword("status"), Value::int(200));
        result.insert(Value::keyword("headers"), Value::map(headers));
        result.insert(Value::keyword("body"), Value::string(content));
        Ok(Value::map(result))
    });

    register_fn(env, "http/text", |args| {
        check_arity!(args, "http/text", 1);
        let content = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;

        let mut headers = BTreeMap::new();
        headers.insert(Value::string("content-type"), Value::string("text/plain"));

        let mut result = BTreeMap::new();
        result.insert(Value::keyword("status"), Value::int(200));
        result.insert(Value::keyword("headers"), Value::map(headers));
        result.insert(Value::keyword("body"), Value::string(content));
        Ok(Value::map(result))
    });

    crate::register_runtime_fn_path_gated(
        env,
        sandbox,
        sema_core::Caps::FS_READ,
        "http/file",
        &[0],
        |args| {
            if args.is_empty() || args.len() > 2 {
                return Err(SemaError::arity("http/file", "1-2", args.len()));
            }
            let file_path = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;

            // Resolve to absolute path
            let path = std::path::Path::new(file_path);
            let abs_path = if path.is_absolute() {
                path.to_path_buf()
            } else {
                std::env::current_dir()
                    .map_err(|e| SemaError::eval(format!("http/file: {e}")))?
                    .join(path)
            };

            // Explicit content-type override (checked eagerly so a bad-type arg
            // still errors identically whether or not we end up offloading).
            let content_type_override = if args.len() == 2 {
                Some(
                    args[1]
                        .as_str()
                        .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?
                        .to_string(),
                )
            } else {
                None
            };

            // Offload the `canonicalize()` (symlink/`..` resolution, can hit
            // multiple syscalls) and the extension-based mime guess so a
            // slow/cold filesystem doesn't stall the single cooperative VM
            // thread. Both are pure, Send-safe computations over owned
            // paths/strings — no `Value`/`Rc` crosses the thread boundary;
            // `http_file_marker` rebuilds the identical `__file` marker map on
            // the VM thread once the worker resolves. Under the unified runtime
            // this suspends structurally on a quarantined-bounded External wait.
            let resolve = move || -> Result<(String, String), String> {
                let real_path = abs_path
                    .canonicalize()
                    .map_err(|e| format!("http/file: {}: {e}", abs_path.display()))?;
                let content_type = match content_type_override {
                    Some(ct) => ct,
                    None => mime_guess::from_path(&real_path)
                        .first_or_octet_stream()
                        .to_string(),
                };
                Ok((real_path.to_string_lossy().to_string(), content_type))
            };
            if sema_core::in_runtime_quantum() {
                return crate::io::quarantined_compute("http/file", http_file_marker, resolve);
            }
            let resolved = resolve().map_err(SemaError::eval)?;
            Ok(NativeOutcome::Return(http_file_marker(resolved)))
        },
    );

    register_fn(env, "http/stream", |args| {
        check_arity!(args, "http/stream", 1);
        if args[0].as_native_fn_ref().is_none() && args[0].as_lambda_rc().is_none() {
            return Err(SemaError::type_error("function", args[0].type_name()));
        }
        let mut map = BTreeMap::new();
        map.insert(Value::keyword("__stream"), Value::bool(true));
        map.insert(Value::keyword("__stream_handler"), args[0].clone());
        Ok(Value::map(map))
    });

    register_fn(env, "http/websocket", |args| {
        check_arity!(args, "http/websocket", 1);
        if args[0].as_native_fn_ref().is_none() && args[0].as_lambda_rc().is_none() {
            return Err(SemaError::type_error("function", args[0].type_name()));
        }
        let mut map = BTreeMap::new();
        map.insert(Value::keyword("__websocket"), Value::bool(true));
        map.insert(Value::keyword("__ws_handler"), args[0].clone());
        Ok(Value::map(map))
    });

    // (route/prefix "/api" routes) — prepend prefix to all route patterns
    register_fn(env, "route/prefix", |args| {
        check_arity!(args, "route/prefix", 2);
        let prefix = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?
            .to_string();
        let routes: Vec<Value> = if let Some(items) = args[1].as_list() {
            items.to_vec()
        } else if let Some(items) = args[1].as_vector_rc() {
            items.to_vec()
        } else {
            return Err(SemaError::type_error("list or vector", args[1].type_name()));
        };
        let prefix = prefix.trim_end_matches('/');
        let mut result = Vec::with_capacity(routes.len());
        for route in routes {
            let items = route.as_vector_rc().ok_or_else(|| {
                SemaError::eval(
                    "route/prefix: each route must be a vector [method pattern handler]",
                )
            })?;
            if items.len() < 3 {
                return Err(SemaError::eval(
                    "route/prefix: each route must have at least 3 elements",
                ));
            }
            let method = items[0].clone();
            let pattern_str = items[1]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", items[1].type_name()))?;
            let new_pattern = if method.as_keyword().as_deref() == Some("static") {
                // For static routes, prefix is the first arg, dir is second — keep dir unchanged
                Value::string(&format!("{}{}", prefix, pattern_str))
            } else {
                Value::string(&format!("{}{}", prefix, pattern_str))
            };
            let mut new_items = vec![method, new_pattern];
            new_items.extend(items[2..].iter().cloned());
            result.push(Value::vector(new_items));
        }
        Ok(Value::list(result))
    });

    // (tools->routes tools) — generate POST routes from tool definitions
    register_fn(env, "tools->routes", |args| {
        check_arity!(args, "tools->routes", 1);
        let tools: Vec<Value> = if let Some(items) = args[0].as_list() {
            items.to_vec()
        } else if let Some(items) = args[0].as_vector_rc() {
            items.to_vec()
        } else {
            return Err(SemaError::type_error("list or vector", args[0].type_name()));
        };
        let mut routes = Vec::with_capacity(tools.len());
        for tool_val in &tools {
            let tool = tool_val
                .as_tool_def_rc()
                .ok_or_else(|| SemaError::type_error("tool", tool_val.type_name()))?;
            let path = format!("/tools/{}", tool.name);
            let handler = tool.handler.clone();
            let param_schema = tool.parameters.clone();
            let tool_name = tool.name.clone();

            // Build a native fn that extracts params from JSON body and calls the tool handler
            let route_handler = Value::native_fn(sema_core::NativeFn::with_ctx(
                format!("tools->routes/{}", tool_name),
                move |ctx, req_args| {
                    check_arity!(req_args, "tool-route-handler", 1);
                    let req = &req_args[0];
                    // Extract JSON body or use empty map
                    let json_body = req
                        .as_map_rc()
                        .and_then(|m| m.get(&Value::keyword("json")).cloned())
                        .unwrap_or_else(Value::nil);

                    // Call the tool handler with the params
                    let tool_args = if json_body.is_nil() {
                        vec![Value::map(BTreeMap::new())]
                    } else {
                        vec![json_body]
                    };
                    let result = sema_core::call_callback(ctx, &handler, &tool_args)?;

                    // Wrap result in http/ok-style response
                    let body = value_to_json_lossy_string(&result)
                        .unwrap_or_else(|_| format!("{}", result));
                    let mut headers = BTreeMap::new();
                    headers.insert(
                        Value::string("content-type"),
                        Value::string("application/json"),
                    );
                    let mut resp = BTreeMap::new();
                    resp.insert(Value::keyword("status"), Value::int(200));
                    resp.insert(Value::keyword("headers"), Value::map(headers));
                    resp.insert(Value::keyword("body"), Value::string(&body));
                    Ok(Value::map(resp))
                },
            ));

            // Also create a schema endpoint
            let schema_path = format!("/tools/{}/schema", tool_name);
            let schema_clone = param_schema.clone();
            let tool_name_clone = tool_name.clone();
            let tool_desc = tool.description.clone();
            let schema_handler = Value::native_fn(sema_core::NativeFn::simple(
                format!("tools->routes/{}/schema", tool_name_clone),
                move |_args| {
                    let schema_json = value_to_json_lossy_string(&schema_clone)
                        .unwrap_or_else(|_| "{}".to_string());
                    let mut body_map = BTreeMap::new();
                    body_map.insert(Value::string("name"), Value::string(&tool_name_clone));
                    body_map.insert(Value::string("description"), Value::string(&tool_desc));
                    body_map.insert(Value::string("parameters"), Value::string(&schema_json));
                    let body = value_to_json_lossy_string(&Value::map(body_map))
                        .unwrap_or_else(|_| "{}".to_string());
                    let mut headers = BTreeMap::new();
                    headers.insert(
                        Value::string("content-type"),
                        Value::string("application/json"),
                    );
                    let mut resp = BTreeMap::new();
                    resp.insert(Value::keyword("status"), Value::int(200));
                    resp.insert(Value::keyword("headers"), Value::map(headers));
                    resp.insert(Value::keyword("body"), Value::string(&body));
                    Ok(Value::map(resp))
                },
            ));

            routes.push(Value::vector(vec![
                Value::keyword("post"),
                Value::string(&path),
                route_handler,
            ]));
            routes.push(Value::vector(vec![
                Value::keyword("get"),
                Value::string(&schema_path),
                schema_handler,
            ]));
        }
        Ok(Value::list(routes))
    });

    // Canonical slash-namespaced alias (Decision #24)
    if let Some(v) = env.get(sema_core::intern("tools->routes")) {
        env.set(sema_core::intern("route/from-tools"), v);
    }

    register_router(env);
    register_serve(env, sandbox);
}

/// Match a URL path against a route pattern, returning extracted parameters on success.
///
/// - Exact segments match literally: `/users` matches `/users`
/// - `:param` segments capture values: `/users/:id` matches `/users/42` -> `[("id","42")]`
/// - `*` wildcard captures rest of path: `/files/*` matches `/files/a/b/c` -> `[("*","a/b/c")]`
/// - Trailing slashes are normalized away before matching.
/// - Segment count must match (except for wildcard which consumes the rest).
pub fn match_path(pattern: &str, path: &str) -> Option<Vec<(String, String)>> {
    // Normalize: strip trailing slash, then split into segments.
    // Root "/" normalizes to a single empty-string segment.
    fn segments(s: &str) -> Vec<&str> {
        let trimmed = s.trim_end_matches('/');
        if trimmed.is_empty() {
            vec![""]
        } else {
            trimmed.split('/').collect()
        }
    }

    let pat_segs = segments(pattern);
    let path_segs = segments(path);

    let mut params = Vec::new();

    for (i, pat_seg) in pat_segs.iter().enumerate() {
        if *pat_seg == "*" {
            // Wildcard: capture the rest of the path from this segment onward
            let rest = if i < path_segs.len() {
                path_segs[i..].join("/")
            } else {
                String::new()
            };
            // Strip leading slash that may appear from the join of segments starting with ""
            let rest = rest.trim_start_matches('/').to_string();
            params.push(("*".to_string(), rest));
            return Some(params);
        }

        // Non-wildcard: segment count must match at this position
        if i >= path_segs.len() {
            return None;
        }

        if let Some(name) = pat_seg.strip_prefix(':') {
            // Parameter capture
            params.push((name.to_string(), path_segs[i].to_string()));
        } else if *pat_seg != path_segs[i] {
            // Literal mismatch
            return None;
        }
    }

    // After consuming all pattern segments, path must have no extra segments
    if pat_segs.len() != path_segs.len() {
        return None;
    }

    Some(params)
}

/// Resolves a `:static` route's `Vec<(index, absolute-dir)>` off the VM thread,
/// then rebuilds the dispatch fn on the VM thread. Holds the route table's
/// handler `Value`s across the External park; its [`Trace`] impl exposes those
/// as live GC edges so the collector never reclaims a handler while the batch
/// canonicalize is in flight.
struct RouterDecoder {
    routes: Vec<(String, String, Value)>,
    indices: Vec<usize>,
}

impl sema_core::runtime::Trace for RouterDecoder {
    fn trace(&self, sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        for (_, _, handler) in &self.routes {
            sink(sema_core::cycle::GcEdge::Value(handler));
        }
        true
    }
}

impl sema_core::runtime::CompletionDecoder for RouterDecoder {
    fn decode(
        self: Box<Self>,
        _context: &mut sema_core::runtime::NativeCallContext<'_>,
        result: Result<sema_core::runtime::SendPayload, sema_core::runtime::ExternalFailure>,
    ) -> sema_core::runtime::DecodedCompletion {
        match result {
            Ok(payload) => match sema_core::runtime::downcast_send_payload::<
                Result<Vec<String>, String>,
            >(payload, "http/router")
            {
                Ok(Ok(resolved)) => {
                    let mut routes = self.routes;
                    for (idx, path_str) in self.indices.iter().zip(resolved) {
                        routes[*idx].2 = Value::string(&path_str);
                    }
                    Ok(build_router_dispatch_fn(std::rc::Rc::new(routes)))
                }
                Ok(Err(message)) => Err(SemaError::eval(message)),
                Err(failure) => Err(SemaError::eval(failure.message().to_string())),
            },
            Err(failure) => Err(SemaError::eval(format!(
                "http/router: {}",
                failure.message()
            ))),
        }
    }
}

/// Parse the route table and build the dispatch fn. Under the unified runtime a
/// `:static` route's directory canonicalize is offloaded to a
/// quarantined-bounded External wait; synchronous callers canonicalize inline.
/// Returns the runtime native ABI so the External suspend can flow out.
fn router_body(args: &[Value]) -> sema_core::runtime::NativeResult {
    use std::rc::Rc;

    check_arity!(args, "http/router", 1);

    // Parse route table: list of [method pattern handler] vectors
    let routes_list = args[0]
        .as_list()
        .or_else(|| args[0].as_vector())
        .ok_or_else(|| SemaError::type_error("list or vector", args[0].type_name()))?;

    // Under the runtime, don't `canonicalize()` a `:static` route's directory
    // inline (a symlink-resolving stat chain) — it
    // would run on the single cooperative VM thread. Instead defer it: push a
    // `nil` placeholder handler and remember (index, absolute-but-not-yet-
    // canonical dir) in `pending`, then resolve every pending dir in ONE offload
    // after the loop. This is safe because, unlike the per-request dispatch loop
    // below, nothing here calls back into Sema (no `continue`-across-a-suspend
    // problem) — every route is still validated, in order, on the VM thread;
    // only the blocking syscall is deferred.
    let async_ctx = sema_core::in_runtime_quantum();

    let mut routes: Vec<(String, String, Value)> = Vec::new();
    let mut pending: Vec<(usize, String)> = Vec::new();
    for route in routes_list.iter() {
        let elems = route
            .as_vector()
            .or_else(|| route.as_list())
            .ok_or_else(|| {
                SemaError::eval("http/router: each route must be a vector [method path handler]")
            })?;
        if elems.len() != 3 {
            return Err(SemaError::eval(
                "http/router: each route must have exactly 3 elements [method path handler]",
            ));
        }
        let method = elems[0]
            .as_keyword()
            .ok_or_else(|| SemaError::type_error("keyword", elems[0].type_name()))?;
        let pattern = elems[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", elems[1].type_name()))?
            .to_string();

        // For :static routes, resolve the directory path at definition time
        // and ensure the pattern ends with /* for wildcard matching
        if method == "static" {
            let dir_path = elems[2].as_str().ok_or_else(|| {
                SemaError::eval("http/router: :static route directory must be a string")
            })?;

            let dir = std::path::Path::new(dir_path);
            let abs_dir = if dir.is_absolute() {
                dir.to_path_buf()
            } else {
                std::env::current_dir()
                    .map_err(|e| SemaError::eval(format!("http/router: {e}")))?
                    .join(dir)
            };

            // Ensure the pattern has a wildcard suffix for matching
            let static_pattern = if pattern.ends_with("/*") || pattern.ends_with("*") {
                pattern
            } else {
                format!("{}/*", pattern.trim_end_matches('/'))
            };

            if async_ctx {
                let idx = routes.len();
                routes.push((method, static_pattern, Value::nil()));
                pending.push((idx, abs_dir.to_string_lossy().to_string()));
                continue;
            }

            let abs_dir = abs_dir.canonicalize().map_err(|e| {
                SemaError::eval(format!(
                    "http/router: static directory '{}': {e}",
                    abs_dir.display()
                ))
            })?;

            // Store the resolved absolute directory path as the handler value
            let handler = Value::string(&abs_dir.to_string_lossy());
            routes.push((method, static_pattern, handler));
            continue;
        }

        let handler = elems[2].clone();
        routes.push((method, pattern, handler));
    }

    if pending.is_empty() {
        return Ok(NativeOutcome::Return(build_router_dispatch_fn(Rc::new(
            routes,
        ))));
    }

    // At least one :static directory still needs canonicalizing, and we deferred
    // it precisely because `async_ctx` was true — offload the whole batch and
    // yield, rebuilding the dispatch function (identical shape to the sync path)
    // once the worker resolves every directory.
    let dir_paths: Vec<String> = pending.iter().map(|(_, d)| d.clone()).collect();
    let indices: Vec<usize> = pending.iter().map(|(i, _)| *i).collect();
    let job = move || -> Result<Vec<String>, String> {
        let mut resolved = Vec::with_capacity(dir_paths.len());
        for d in &dir_paths {
            let real = std::path::Path::new(d)
                .canonicalize()
                .map_err(|e| format!("http/router: static directory '{d}': {e}"))?;
            resolved.push(real.to_string_lossy().to_string());
        }
        Ok(resolved)
    };
    // This tail is reached only when `async_ctx` (a runtime quantum) deferred the
    // static-directory canonicalization; resolve it off the VM thread and rebuild.
    let decoder = Box::new(RouterDecoder { routes, indices });
    crate::io::quarantined_compute_with_decoder("http/router", decoder, move || {
        Ok(Box::new(job()) as sema_core::runtime::SendPayload)
    })
}

fn register_router(env: &sema_core::Env) {
    use sema_core::{intern, NativeFn};

    env.set(
        intern("http/router"),
        Value::native_fn(NativeFn::with_ctx_runtime(
            "http/router",
            |_ctx, args: &[Value]| match router_body(args)? {
                NativeOutcome::Return(value) => Ok(value),
                _ => Err(SemaError::eval(
                    "http/router: native suspended outside the cooperative runtime",
                )),
            },
            |_ctx, args| router_body(args),
        )),
    );
}

/// Dispatch one request against the resolved route table. Returns the runtime
/// native ABI so a `:static` file's `canonicalize()` can suspend structurally on
/// a quarantined-bounded External wait under the unified runtime. `invoke` runs
/// a matched non-static route's handler (through the evaluator's call callback —
/// the caller supplies the appropriate `EvalContext`). `can_suspend` is `true`
/// only when the caller reached this via the runtime ABI (so a structural
/// suspend can flow out); the synchronous value ABI passes `false` and
/// canonicalizes inline, so it never returns a suspension the value ABI cannot
/// carry.
fn dispatch_body(
    routes: &[(String, String, Value)],
    args: &[Value],
    invoke: &dyn Fn(&Value, Value) -> Result<Value, SemaError>,
    can_suspend: bool,
) -> sema_core::runtime::NativeResult {
    check_arity!(args, "http/router/dispatch", 1);
    let req = &args[0];

    // Extract method from request map
    let req_map = req
        .as_map_rc()
        .ok_or_else(|| SemaError::type_error("map", req.type_name()))?;

    let req_method = req_map
        .get(&Value::keyword("method"))
        .ok_or_else(|| SemaError::eval("http/router: request missing :method"))?
        .as_keyword()
        .ok_or_else(|| SemaError::type_error("keyword", "other"))?;

    let req_path = req_map
        .get(&Value::keyword("path"))
        .ok_or_else(|| SemaError::eval("http/router: request missing :path"))?
        .as_str()
        .ok_or_else(|| SemaError::type_error("string", "other"))?
        .to_string();

    // Try each route
    for (method, pattern, handler) in routes.iter() {
        // WebSocket routes match GET requests (WS upgrade starts as GET)
        let is_ws_route = method == "ws";
        // Static routes only match GET/HEAD requests
        let is_static_route = method == "static";
        if is_ws_route || is_static_route {
            if req_method != "get" && req_method != "head" {
                continue;
            }
        } else if method != "any" && method != &req_method {
            continue;
        }

        // Path matching
        if let Some(params) = match_path(pattern, &req_path) {
            // For static routes, resolve the file and return a file marker
            if is_static_route {
                let dir_path = handler.as_str().unwrap_or("");
                let rel_path = params
                    .iter()
                    .find(|(k, _)| k == "*")
                    .map(|(_, v)| v.as_str())
                    .unwrap_or("");

                // Security: reject path traversal
                if rel_path.contains("..") {
                    let mut headers = BTreeMap::new();
                    headers.insert(Value::string("content-type"), Value::string("text/plain"));
                    let mut result = BTreeMap::new();
                    result.insert(Value::keyword("status"), Value::int(400));
                    result.insert(Value::keyword("headers"), Value::map(headers));
                    result.insert(Value::keyword("body"), Value::string("Bad Request"));
                    return Ok(NativeOutcome::Return(Value::map(result)));
                }

                let file_path = std::path::Path::new(dir_path).join(rel_path);

                // If it's a directory, try index.html
                let file_path = if file_path.is_dir() {
                    file_path.join("index.html")
                } else {
                    file_path
                };

                if !file_path.exists() {
                    // Don't match — fall through to other routes (allows SPA
                    // fallback as a later catch-all). This decision must stay
                    // synchronous even in async context: `continue`ing this loop
                    // after an offloaded yield isn't possible — resuming delivers
                    // the decoded value directly as this whole dispatch call's
                    // result, bypassing any further routes — so only the
                    // *terminal* work below (which always ends in a `return`,
                    // never `continue`) is safe to offload. `exists()`/`is_dir()`
                    // are also single fast stat syscalls, unlike `canonicalize()`
                    // below which can walk a full symlink chain.
                    continue;
                }

                // From here on every path returns (403 escape or the `__file`
                // marker) — no more `continue`s — so it's safe to offload the
                // rest instead of stalling the single cooperative VM thread on
                // `canonicalize()`'s symlink-resolving stat chain.
                let dir_path_owned = dir_path.to_string();
                let file_path_owned = file_path.clone();
                let resolve = move || -> Result<(bool, String, String), String> {
                    // Security (STD-11): confirm the resolved file stays inside
                    // dir_path. The ".." substring check above can't catch
                    // symlink/junction escapes; canonicalize() resolves links,
                    // then we verify the prefix.
                    let escapes = match (
                        std::fs::canonicalize(&dir_path_owned),
                        std::fs::canonicalize(&file_path_owned),
                    ) {
                        (Ok(base), Ok(real)) => !real.starts_with(&base),
                        _ => true,
                    };
                    let content_type = mime_guess::from_path(&file_path_owned)
                        .first_or_octet_stream()
                        .to_string();
                    Ok((
                        escapes,
                        file_path_owned.to_string_lossy().to_string(),
                        content_type,
                    ))
                };
                if can_suspend && sema_core::in_runtime_quantum() {
                    return crate::io::quarantined_compute(
                        "http/router/dispatch",
                        static_file_response,
                        resolve,
                    );
                }
                // Bare/top-level (and value-ABI-under-runtime): canonicalize
                // inline. `resolve` never returns `Err`, so this cannot fail.
                let resolved = resolve().map_err(SemaError::eval)?;
                return Ok(NativeOutcome::Return(static_file_response(resolved)));
            }

            // Build params map (keyword keys)
            let mut params_map = BTreeMap::new();
            for (k, v) in &params {
                params_map.insert(Value::keyword(k), Value::string(v));
            }

            // Merge params into existing :params in the request
            let existing_params = req_map
                .get(&Value::keyword("params"))
                .and_then(|v| v.as_map_rc());

            if let Some(existing) = existing_params {
                for (k, v) in existing.iter() {
                    params_map.entry(k.clone()).or_insert_with(|| v.clone());
                }
            }

            // Build new request with merged params
            let mut new_req = (*req_map).clone();
            new_req.insert(Value::keyword("params"), Value::map(params_map));
            let new_req_val = Value::map(new_req);

            // For WebSocket routes, return a marker map instead of calling handler
            if is_ws_route {
                let mut ws_map = BTreeMap::new();
                ws_map.insert(Value::keyword("__websocket"), Value::bool(true));
                ws_map.insert(Value::keyword("__ws_handler"), handler.clone());
                ws_map.insert(Value::keyword("__ws_request"), new_req_val);
                return Ok(NativeOutcome::Return(Value::map(ws_map)));
            }

            // Call handler
            return invoke(handler, new_req_val).map(NativeOutcome::Return);
        }
    }

    // No route matched — return 404
    let mut headers = BTreeMap::new();
    headers.insert(
        Value::string("content-type"),
        Value::string("application/json"),
    );
    let mut result = BTreeMap::new();
    result.insert(Value::keyword("status"), Value::int(404));
    result.insert(Value::keyword("headers"), Value::map(headers));
    result.insert(Value::keyword("body"), Value::string("\"Not Found\""));
    Ok(NativeOutcome::Return(Value::map(result)))
}

/// Build the `http/router/dispatch` closure for a fully-resolved route table
/// (every `:static` directory already canonicalized). Dual-ABI: the runtime
/// callback lets a `:static` file's `canonicalize()` suspend structurally on an
/// External wait; the synchronous value callback canonicalizes inline. Both
/// invoke a matched non-static handler through `call_callback` — the value ABI
/// with its passed `EvalContext`, the runtime ABI with the installed stdlib
/// context.
fn build_router_dispatch_fn(routes: std::rc::Rc<Vec<(String, String, Value)>>) -> Value {
    use sema_core::{call_callback, with_stdlib_ctx, EvalContext, NativeFn};

    let routes_value = std::rc::Rc::clone(&routes);
    Value::native_fn(NativeFn::with_ctx_runtime(
        "http/router/dispatch",
        move |ctx: &EvalContext, args: &[Value]| {
            let invoke = |handler: &Value, req: Value| call_callback(ctx, handler, &[req]);
            match dispatch_body(&routes, args, &invoke, false)? {
                NativeOutcome::Return(value) => Ok(value),
                _ => Err(SemaError::eval(
                    "http/router/dispatch: native suspended outside the cooperative runtime",
                )),
            }
        },
        move |_ctx, args| {
            let invoke = |handler: &Value, req: Value| {
                with_stdlib_ctx(|c| call_callback(c, handler, &[req]))
            };
            dispatch_body(&routes_value, args, &invoke, true)
        },
    ))
}

/// Convert an HTTP method string (e.g. "GET") to a lowercase keyword Value (e.g. :get).
/// Validate a user-supplied port number. A bare `as u16` silently wrapped
/// out-of-range values (70000 -> 4464, -1 -> 65535), binding the wrong port
/// while logging the original. Port 0 deliberately asks the OS for an
/// ephemeral listener; reject anything outside 0..=65535.
fn parse_port(p: i64) -> Result<u16, SemaError> {
    if (0..=65535).contains(&p) {
        Ok(p as u16)
    } else {
        Err(SemaError::eval(format!(
            "http/serve: port must be in 0..=65535, got {p}"
        )))
    }
}

fn method_keyword(method: &str) -> Value {
    Value::keyword(&method.to_ascii_lowercase())
}

/// Parse a query string like "a=1&b=2" into a Sema map {:a "1" :b "2"}.
fn parse_query_string(query: Option<&str>) -> Value {
    let mut map = BTreeMap::new();
    if let Some(qs) = query {
        for pair in qs.split('&') {
            if pair.is_empty() {
                continue;
            }
            let (key, val) = match pair.split_once('=') {
                Some((k, v)) => (k, v),
                None => (pair, ""),
            };
            map.insert(Value::keyword(key), Value::string(val));
        }
    }
    Value::map(map)
}

/// Convert a RawRequest into a Sema Value map on the main (evaluator) thread.
fn raw_request_to_value(raw: &RawRequest) -> Value {
    let mut headers_map = BTreeMap::new();
    for (k, v) in &raw.headers {
        headers_map.insert(Value::string(k), Value::string(v));
    }

    let query_val = parse_query_string(raw.query.as_deref());

    let mut req_map = BTreeMap::new();
    req_map.insert(Value::keyword("method"), method_keyword(&raw.method));
    req_map.insert(Value::keyword("path"), Value::string(&raw.path));
    req_map.insert(Value::keyword("headers"), Value::map(headers_map));
    req_map.insert(Value::keyword("query"), query_val);
    req_map.insert(Value::keyword("params"), Value::map(BTreeMap::new()));
    req_map.insert(Value::keyword("body"), Value::string(&raw.body));

    // Auto-parse JSON body if content-type indicates json
    if raw.content_type_is_json && !raw.body.is_empty() {
        if let Ok(json_val) = serde_json::from_str::<serde_json::Value>(&raw.body) {
            let sema_val = crate::json::json_to_value(&json_val);
            req_map.insert(Value::keyword("json"), sema_val);
        }
    }

    Value::map(req_map)
}

/// Convert a Sema response Value map into a RawResponse for sending back to the axum thread.
fn value_to_raw_response(val: &Value) -> RawResponse {
    let map = match val.as_map_rc() {
        Some(m) => m,
        None => {
            return RawResponse {
                status: 200,
                headers: vec![("content-type".to_string(), "text/plain".to_string())],
                body: val.to_string(),
            };
        }
    };

    let status = map
        .get(&Value::keyword("status"))
        .and_then(|v| v.as_int())
        .unwrap_or(200) as u16;

    let body = map
        .get(&Value::keyword("body"))
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .unwrap_or_default();

    let mut headers = Vec::new();
    if let Some(h) = map
        .get(&Value::keyword("headers"))
        .and_then(|v| v.as_map_rc())
    {
        for (k, v) in h.iter() {
            let key = k
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or_else(|| k.to_string());
            let val = v
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or_else(|| v.to_string());
            headers.push((key, val));
        }
    }

    RawResponse {
        status,
        headers,
        body,
    }
}

/// Convert a RawResponse into an axum HTTP response.
fn raw_response_to_axum(raw: &RawResponse) -> axum::response::Response {
    use axum::http::{HeaderName, HeaderValue, StatusCode};
    use axum::response::IntoResponse;

    let status = StatusCode::from_u16(raw.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);

    let mut builder = axum::http::Response::builder().status(status);
    for (k, v) in &raw.headers {
        if let (Ok(name), Ok(val)) = (
            HeaderName::try_from(k.as_str()),
            HeaderValue::try_from(v.as_str()),
        ) {
            builder = builder.header(name, val);
        }
    }

    builder
        .body(axum::body::Body::from(raw.body.clone()))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// Handle an incoming axum request: extract metadata, forward to the evaluator, and
/// return the appropriate response (normal HTTP, SSE stream, or WebSocket upgrade).
async fn handle_axum_request(
    ws_upgrade: Option<axum::extract::ws::WebSocketUpgrade>,
    req: axum::extract::Request,
    tx: tokio::sync::mpsc::Sender<ServerRequest>,
    lifecycle_tx: tokio::sync::mpsc::UnboundedSender<Arc<ServeRequestLifecycle>>,
) -> axum::response::Response {
    use axum::response::IntoResponse;

    // Extract method, URI, headers from axum request
    let method = req.method().to_string();
    let uri = req.uri().clone();
    let path = uri.path().to_string();
    let query = uri.query().map(|q| q.to_string());

    let mut headers = Vec::new();
    let mut content_type_is_json = false;
    for (name, value) in req.headers().iter() {
        let v = value.to_str().unwrap_or("").to_string();
        let n = name.as_str().to_string();
        if n == "content-type" && v.contains("json") {
            content_type_is_json = true;
        }
        headers.push((n, v));
    }

    // Read body with a size cap so a client can't stream an unbounded body into
    // memory and exhaust the process (DoS). `to_bytes` returns Err once the
    // limit is exceeded; surface that as 413 rather than a generic read error.
    const MAX_BODY_BYTES: usize = 16 * 1024 * 1024; // 16 MiB
    let body_bytes = match axum::body::to_bytes(req.into_body(), MAX_BODY_BYTES).await {
        Ok(b) => b,
        Err(_) => {
            return raw_response_to_axum(&RawResponse {
                status: 413,
                headers: vec![("content-type".to_string(), "text/plain".to_string())],
                body: format!(
                    "Request body too large or unreadable (max {} bytes)",
                    MAX_BODY_BYTES
                ),
            });
        }
    };
    let body = String::from_utf8_lossy(&body_bytes).to_string();

    let raw = RawRequest {
        method,
        path,
        headers,
        query,
        body,
        content_type_is_json,
    };

    // Create oneshot channel for the response
    let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
    let lifecycle = ServeRequestLifecycle::new(next_serve_request_id(), lifecycle_tx);

    // Send request to main thread
    if tx
        .send(ServerRequest::Http {
            lifecycle: Arc::clone(&lifecycle),
            raw,
            respond: resp_tx,
        })
        .await
        .is_err()
    {
        return raw_response_to_axum(&RawResponse {
            status: 503,
            headers: vec![("content-type".to_string(), "text/plain".to_string())],
            body: "Server shutting down".to_string(),
        });
    }

    // The route future owns the runtime handler until a response shape is
    // transferred. If Hyper cancels this future because its connection task is
    // dropped, the lease publishes a per-request disconnect for the VM accept
    // loop to cancel. Reading an HTTP/1 request followed by an orderly FIN does
    // not necessarily cancel Hyper's in-flight service future; this lease pins
    // the precise future-drop ownership boundary.
    let mut request_lease = RequestFutureLease::new(lifecycle);

    // Wait for response from main thread
    let response = resp_rx.await;
    request_lease.disarm();
    match response {
        Ok(ServerResponse::Raw(raw_resp)) => raw_response_to_axum(&raw_resp),
        Ok(ServerResponse::Sse(rx)) => {
            use axum::response::sse::{Event, Sse};
            use futures::stream::StreamExt;
            use tokio_stream::wrappers::UnboundedReceiverStream;

            let stream = UnboundedReceiverStream::new(rx)
                .map(|data| Ok::<_, std::convert::Infallible>(Event::default().data(data)));
            Sse::new(stream).into_response()
        }
        Ok(ServerResponse::WebSocket {
            incoming_tx,
            incoming_generation,
            outgoing_rx,
        }) => {
            if let Some(ws) = ws_upgrade {
                ws.on_upgrade(move |socket| {
                    bridge_websocket(socket, incoming_tx, incoming_generation, outgoing_rx)
                })
                .into_response()
            } else {
                raw_response_to_axum(&RawResponse {
                    status: 400,
                    headers: vec![("content-type".to_string(), "text/plain".to_string())],
                    body: "WebSocket upgrade required".to_string(),
                })
            }
        }
        Ok(ServerResponse::File { path, content_type }) => {
            match tokio::fs::read(&path).await {
                Ok(bytes) => {
                    use axum::http::{HeaderValue, StatusCode};

                    let mut response = axum::http::Response::builder()
                        .status(StatusCode::OK)
                        .body(axum::body::Body::from(bytes))
                        .unwrap();
                    if let Ok(ct) = HeaderValue::try_from(&content_type) {
                        response.headers_mut().insert("content-type", ct);
                    }
                    // Set cache headers for static assets
                    if let Ok(val) = HeaderValue::from_str("public, max-age=3600") {
                        response.headers_mut().insert("cache-control", val);
                    }
                    response
                }
                Err(_) => raw_response_to_axum(&RawResponse {
                    status: 404,
                    headers: vec![("content-type".to_string(), "text/plain".to_string())],
                    body: "Not Found".to_string(),
                }),
            }
        }
        Err(_) => raw_response_to_axum(&RawResponse {
            status: 500,
            headers: vec![("content-type".to_string(), "text/plain".to_string())],
            body: "Handler did not respond".to_string(),
        }),
    }
}

/// Tear down both halves of a WebSocket bridge before publishing its final
/// incoming-queue generation.
async fn finish_server_ws_bridge_tasks(
    mut recv_task: tokio::task::JoinHandle<()>,
    mut send_task: tokio::task::JoinHandle<()>,
    incoming_generation: tokio::sync::watch::Sender<u64>,
) {
    tokio::select! {
        _ = &mut recv_task => {
            send_task.abort();
            let _ = send_task.await;
        }
        _ = &mut send_task => {
            recv_task.abort();
            let _ = recv_task.await;
        }
    }

    // Awaiting the losing task guarantees its future and every mpsc sender it
    // owns have dropped before shutdown readiness is published.
    incoming_generation.send_modify(|generation| *generation = generation.wrapping_add(1));
}

/// Bridge an axum WebSocket to the evaluator's channels.
async fn bridge_websocket(
    socket: axum::extract::ws::WebSocket,
    incoming_tx: tokio::sync::mpsc::Sender<WsMsg>,
    incoming_generation: tokio::sync::watch::Sender<u64>,
    mut outgoing_rx: tokio::sync::mpsc::Receiver<WsMsg>,
) {
    use axum::extract::ws::Message;
    use futures::{SinkExt, StreamExt};

    let (mut ws_sink, mut ws_stream) = socket.split();

    // Task 1: forward messages from client (WebSocket) to evaluator. Text frames
    // become `WsMsg::Text`, binary frames `WsMsg::Binary`; ping/pong are handled
    // by axum and ignored here.
    let incoming_generation_for_messages = incoming_generation.clone();
    let recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = ws_stream.next().await {
            let forwarded = match msg {
                Message::Text(text) => incoming_tx.send(WsMsg::Text(text.to_string())).await,
                Message::Binary(bytes) => incoming_tx.send(WsMsg::Binary(bytes.to_vec())).await,
                Message::Close(_) => break,
                _ => continue, // ping/pong
            };
            if forwarded.is_err() {
                break;
            }
            incoming_generation_for_messages
                .send_modify(|generation| *generation = generation.wrapping_add(1));
        }
    });

    // Task 2: forward messages from evaluator to client (WebSocket)
    let send_task = tokio::spawn(async move {
        while let Some(msg) = outgoing_rx.recv().await {
            let frame = match msg {
                WsMsg::Text(s) => Message::Text(s.into()),
                WsMsg::Binary(b) => Message::Binary(b.into()),
            };
            if ws_sink.send(frame).await.is_err() {
                break;
            }
        }
        // Try to send a close frame
        let _ = ws_sink.send(Message::Close(None)).await;
    });

    finish_server_ws_bridge_tasks(recv_task, send_task, incoming_generation).await;
}

/// Check if a response Value is an SSE stream marker.
fn is_stream_response(val: &Value) -> bool {
    if let Some(m) = val.as_map_rc() {
        m.get(&Value::keyword("__stream"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    } else {
        false
    }
}

/// Check if a response Value is a WebSocket marker.
fn is_websocket_response(val: &Value) -> bool {
    if let Some(m) = val.as_map_rc() {
        m.get(&Value::keyword("__websocket"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    } else {
        false
    }
}

/// Check if a response Value is a file response marker.
fn is_file_response(val: &Value) -> bool {
    if let Some(m) = val.as_map_rc() {
        m.get(&Value::keyword("__file"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    } else {
        false
    }
}

/// Extract file path and content type from a file response marker and send to axum.
fn handle_file_response(
    response_val: &Value,
    respond: tokio::sync::oneshot::Sender<ServerResponse>,
) {
    let map = response_val.as_map_rc().unwrap();
    let path_str = map
        .get(&Value::keyword("__file_path"))
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .unwrap_or_default();
    let content_type = map
        .get(&Value::keyword("__file_content_type"))
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "application/octet-stream".to_string());

    let _ = respond.send(ServerResponse::File {
        path: std::path::PathBuf::from(path_str),
        content_type,
    });
}

/// Build the `send` native fn handed to an SSE handler. Extracted so a test can
/// drive it from inside a tokio runtime — the exact condition (a handler
/// streaming via `llm/stream`, which runs the callback inside the provider's
/// `block_on`) that panicked when the channel was bounded + `blocking_send`.
fn make_sse_send_fn(sse_tx: tokio::sync::mpsc::UnboundedSender<String>) -> Value {
    use sema_core::NativeFn;
    Value::native_fn(NativeFn::simple("http/stream/send", move |args| {
        check_arity!(args, "http/stream/send", 1);
        let msg = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        // Err only when the receiver dropped (client disconnected) — preserves
        // the "SSE stream closed" contract.
        sse_tx
            .send(msg.to_string())
            .map_err(|_| SemaError::eval("SSE stream closed"))?;
        Ok(Value::nil())
    }))
}

/// Handle an SSE stream response: extract the stream handler, create channels,
/// send the SSE receiver to axum, then call the handler with a `send` function.
fn handle_sse_response(
    ctx: &sema_core::EvalContext,
    response_val: &Value,
    respond: tokio::sync::oneshot::Sender<ServerResponse>,
) {
    use sema_core::call_callback;

    let map = response_val.as_map_rc().unwrap();
    let stream_handler = map
        .get(&Value::keyword("__stream_handler"))
        .cloned()
        .unwrap();

    // Create the SSE channel. Unbounded because the handler runs on the
    // evaluator thread and may `send` from inside a provider's block_on (e.g.
    // llm/stream feeding tokens): UnboundedSender::send is synchronous and never
    // asserts "not in a runtime", so it can't panic like blocking_send. Chunks
    // are small and network-paced; a bounded try_send would silently drop tokens.
    let (sse_tx, sse_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    // Send the SSE receiver to axum so it can start streaming immediately
    let _ = respond.send(ServerResponse::Sse(sse_rx));

    // Build the `send` function for the Sema handler
    let send_fn = make_sse_send_fn(sse_tx);

    // Call the stream handler with the send function.
    // When it returns (or errors), the sse_tx is dropped, closing the stream.
    match call_callback(ctx, &stream_handler, &[send_fn]) {
        Ok(_) => {}
        Err(e) => {
            eprintln!("http/stream handler error: {e}");
        }
    }
}

const SERVER_WS_RECV_COMPLETION_KIND: u64 = 0x7377_7372; // "swsr"

type ServerWsReceiverCell =
    std::rc::Rc<std::cell::RefCell<Option<tokio::sync::mpsc::Receiver<WsMsg>>>>;

/// Rechecks the VM-owned incoming receiver after every generation wake. The
/// watch handle contains no `Value`, and cancellation drops only its cloned
/// receiver while leaving the installed message receiver usable.
struct ServerWsRecvContinuation {
    in_rx: ServerWsReceiverCell,
    incoming_generation: tokio::sync::watch::Receiver<u64>,
}

impl sema_core::runtime::Trace for ServerWsRecvContinuation {
    fn trace(&self, _sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        true
    }
}

impl sema_core::runtime::NativeContinuation for ServerWsRecvContinuation {
    fn resume(
        self: Box<Self>,
        _context: &mut sema_core::runtime::NativeCallContext<'_>,
        input: sema_core::runtime::ResumeInput,
    ) -> sema_core::runtime::NativeResult {
        use sema_core::runtime::ResumeInput;

        match input {
            ResumeInput::Returned(_) => {
                suspend_server_ws_receive(self.in_rx, self.incoming_generation)
            }
            ResumeInput::Failed(error) => Err(error),
            ResumeInput::Cancelled(reason) => Err(SemaError::eval(format!(
                "ws/recv was cancelled ({reason:?})"
            ))),
            ResumeInput::Runtime(_) => Err(SemaError::eval(
                "ws/recv continuation received an unexpected runtime response",
            )),
        }
    }
}

/// Check the incoming queue without moving its receiver off the VM thread.
/// `None` means the queue is empty; a cleared or disconnected receiver is a
/// ready `nil` result.
fn try_server_ws_message(in_rx: &ServerWsReceiverCell) -> Option<Value> {
    use tokio::sync::mpsc::error::TryRecvError;

    let mut rx_opt = in_rx.borrow_mut();
    let Some(rx) = rx_opt.as_mut() else {
        return Some(Value::nil());
    };
    match rx.try_recv() {
        Ok(WsMsg::Text(text)) => Some(Value::string(&text)),
        Ok(WsMsg::Binary(bytes)) => Some(Value::bytevector(bytes)),
        Err(TryRecvError::Empty) => None,
        Err(TryRecvError::Disconnected) => {
            *rx_opt = None;
            Some(Value::nil())
        }
    }
}

async fn wait_for_server_ws_generation(
    mut incoming_generation: tokio::sync::watch::Receiver<u64>,
) -> Result<(), String> {
    let _ = incoming_generation.changed().await;
    Ok(())
}

fn arm_server_ws_generation(
    incoming_generation: &tokio::sync::watch::Receiver<u64>,
) -> tokio::sync::watch::Receiver<u64> {
    let mut armed = incoming_generation.clone();
    armed.borrow_and_update();
    armed
}

/// Arm a lossless generation wait before the final VM-thread queue check. A
/// message queued before the snapshot is visible to `try_recv`; a later queue
/// publication advances the generation and wakes the continuation.
fn suspend_server_ws_receive(
    in_rx: ServerWsReceiverCell,
    incoming_generation: tokio::sync::watch::Receiver<u64>,
) -> sema_core::runtime::NativeResult {
    let wait_generation = arm_server_ws_generation(&incoming_generation);
    if let Some(value) = try_server_ws_message(&in_rx) {
        return Ok(NativeOutcome::Return(value));
    }

    let continuation: Box<dyn sema_core::runtime::NativeContinuation> =
        Box::new(ServerWsRecvContinuation {
            in_rx,
            incoming_generation,
        });
    let kind = sema_core::runtime::CompletionKind::try_from_raw(SERVER_WS_RECV_COMPLETION_KIND)
        .expect("server websocket receive completion kind is nonzero");
    crate::runtime_offload::external_io_async_try_with_continuation(
        "ws/recv",
        kind,
        "server ws/recv/generation",
        |()| Ok(Value::nil()),
        continuation,
        move || wait_for_server_ws_generation(wait_generation),
    )
}

fn close_server_ws_receiver(
    in_rx: &ServerWsReceiverCell,
    incoming_generation: &tokio::sync::watch::Sender<u64>,
) {
    in_rx.borrow_mut().take();
    incoming_generation.send_modify(|generation| *generation = generation.wrapping_add(1));
}

/// Resumes once a runtime-dispatched WebSocket handler call (see
/// `handle_ws_response_runtime`) returns, fails, or is cancelled. Holds no
/// `Value` — the handler's return value is discarded (matching the legacy
/// `handle_ws_response`'s `Ok(_) => {}`), and a handler error is logged
/// rather than failing the whole connection task, exactly as before.
struct WsHandlerContinuation;

impl sema_core::runtime::Trace for WsHandlerContinuation {
    fn trace(&self, _sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        true
    }
}

impl sema_core::runtime::NativeContinuation for WsHandlerContinuation {
    fn resume(
        self: Box<Self>,
        _context: &mut sema_core::runtime::NativeCallContext<'_>,
        input: sema_core::runtime::ResumeInput,
    ) -> sema_core::runtime::NativeResult {
        use sema_core::runtime::ResumeInput;
        match input {
            ResumeInput::Returned(_) => Ok(NativeOutcome::Return(Value::nil())),
            ResumeInput::Failed(error) => {
                eprintln!("ws handler error: {error}");
                Ok(NativeOutcome::Return(Value::nil()))
            }
            ResumeInput::Cancelled(reason) => Err(SemaError::eval(format!(
                "http/serve: websocket connection handler was cancelled ({reason:?})"
            ))),
            ResumeInput::Runtime(_) => Err(SemaError::eval(
                "http/serve: websocket handler call received an unexpected runtime response",
            )),
        }
    }
}

/// Runtime-ABI counterpart of [`handle_ws_response`]: build the bidirectional
/// channels and connection map exactly as the legacy path does, but instead
/// of calling the WS handler synchronously through `call_callback` (which
/// runs it on a fresh, non-cooperative "foreign VM" — see
/// `sema-vm/src/vm.rs`'s `make_closure` "TEMPORARY BRIDGE" comment — where
/// `in_runtime_quantum()` is suspended for the duration, so `(:recv conn)`
/// could never suspend cooperatively and would fall back to blocking), this
/// dispatches the handler through `NativeOutcome::Call`. That is a genuine
/// VM-dispatched call within the connection's OWN spawned task quantum
/// (the same mechanism `AcceptLoopContinuation` uses to invoke the
/// per-connection dispatch factory), so `in_runtime_quantum()` stays true for
/// the handler's whole body — including any nested `(:recv conn)` — and the
/// connection's `recv_fn` (below) can suspend on an External generation
/// wait instead of blocking the VM thread. This is what makes an idle
/// WebSocket non-blocking for its siblings (SRV-1 piece c).
fn handle_ws_response_runtime(
    response_val: &Value,
    respond: tokio::sync::oneshot::Sender<ServerResponse>,
) -> sema_core::runtime::NativeResult {
    use sema_core::runtime::{NativeCall, NativeOutcome};
    use sema_core::NativeFn;
    use std::cell::RefCell;
    use std::rc::Rc;

    let map = response_val.as_map_rc().unwrap();
    let ws_handler = map.get(&Value::keyword("__ws_handler")).cloned().unwrap();

    let (in_tx, in_rx) = tokio::sync::mpsc::channel::<WsMsg>(256);
    let (incoming_generation_tx, incoming_generation) = tokio::sync::watch::channel(0_u64);
    let (out_tx, out_rx) = tokio::sync::mpsc::channel::<WsMsg>(256);

    let _ = respond.send(ServerResponse::WebSocket {
        incoming_tx: in_tx,
        incoming_generation: incoming_generation_tx.clone(),
        outgoing_rx: out_rx,
    });

    // Same send/close shape as the legacy `handle_ws_response` — `ws/send`
    // only blocks the VM thread if the 256-slot outgoing queue is full (a
    // slow/stalled client), a narrower limitation than the idle-recv
    // head-of-line case the cooperative `ws/recv` path handles.
    let out_tx = Rc::new(RefCell::new(Some(out_tx)));
    let out_tx_for_send = out_tx.clone();
    let send_fn = Value::native_fn(NativeFn::simple("ws/send", move |args| {
        check_arity!(args, "ws/send", 1);
        let msg = if let Some(s) = args[0].as_str() {
            WsMsg::Text(s.to_string())
        } else if let Some(b) = args[0].as_bytevector() {
            WsMsg::Binary(b.to_vec())
        } else {
            return Err(SemaError::type_error(
                "string or bytevector",
                args[0].type_name(),
            ));
        };
        let guard = out_tx_for_send.borrow();
        let tx = guard
            .as_ref()
            .ok_or_else(|| SemaError::eval("WebSocket closed"))?;
        tx.blocking_send(msg)
            .map_err(|_| SemaError::eval("WebSocket closed"))?;
        Ok(Value::nil())
    }));

    let in_rx = Rc::new(RefCell::new(Some(in_rx)));
    let in_rx_for_recv_legacy = in_rx.clone();
    let in_rx_for_recv_runtime = in_rx.clone();
    let incoming_generation_for_recv_runtime = incoming_generation.clone();
    let recv_fn = Value::native_fn(NativeFn::simple_with_runtime(
        "ws/recv",
        move |args| {
            check_arity!(args, "ws/recv", 0);
            let mut rx_opt = in_rx_for_recv_legacy.borrow_mut();
            if let Some(rx) = rx_opt.as_mut() {
                match rx.blocking_recv() {
                    Some(WsMsg::Text(s)) => Ok(Value::string(&s)),
                    Some(WsMsg::Binary(b)) => Ok(Value::bytevector(b)),
                    None => {
                        *rx_opt = None;
                        Ok(Value::nil())
                    }
                }
            } else {
                Ok(Value::nil())
            }
        },
        move |_ctx, args| -> sema_core::runtime::NativeResult {
            check_arity!(args, "ws/recv", 0);
            // Always true here — this native is only ever reached via the
            // Call dispatch above, itself only issued from inside a runtime
            // quantum — but check anyway rather than assume, mirroring every
            // other cooperative op in this codebase (see `ws.rs`'s
            // `ws_recv`).
            if sema_core::in_runtime_quantum() {
                return suspend_server_ws_receive(
                    in_rx_for_recv_runtime.clone(),
                    incoming_generation_for_recv_runtime.clone(),
                );
            }
            let mut rx_opt = in_rx_for_recv_runtime.borrow_mut();
            if let Some(rx) = rx_opt.as_mut() {
                match rx.blocking_recv() {
                    Some(WsMsg::Text(s)) => Ok(NativeOutcome::Return(Value::string(&s))),
                    Some(WsMsg::Binary(b)) => Ok(NativeOutcome::Return(Value::bytevector(b))),
                    None => {
                        *rx_opt = None;
                        Ok(NativeOutcome::Return(Value::nil()))
                    }
                }
            } else {
                Ok(NativeOutcome::Return(Value::nil()))
            }
        },
    ));

    let out_tx_for_close = out_tx;
    let in_rx_for_close = in_rx;
    let incoming_generation_for_close = incoming_generation_tx;
    let close_fn = Value::native_fn(NativeFn::simple("ws/close", move |args| {
        check_arity!(args, "ws/close", 0);
        out_tx_for_close.borrow_mut().take();
        close_server_ws_receiver(&in_rx_for_close, &incoming_generation_for_close);
        Ok(Value::nil())
    }));

    let mut conn_map = BTreeMap::new();
    conn_map.insert(Value::keyword("send"), send_fn);
    conn_map.insert(Value::keyword("recv"), recv_fn);
    conn_map.insert(Value::keyword("close"), close_fn);
    let conn = Value::map(conn_map);

    Ok(NativeOutcome::Call(NativeCall {
        callable: ws_handler,
        args: vec![conn],
        continuation: Box::new(WsHandlerContinuation),
    }))
}

/// Handle a WebSocket response: extract the WS handler, create bidirectional channels,
/// send them to axum for bridging, then call the handler with a connection map.
///
/// Legacy/non-quantum path only (reached solely from `http_serve_impl`'s
/// serial loop and the responder's legacy value-ABI branch — both dead in
/// the shipped product). Its `ws/recv` blocks the VM thread with
/// `blocking_recv`; that's fine here because this whole path already has no
/// sibling connections to stall (`http_serve_impl` handles one request at a
/// time by construction). See [`handle_ws_response_runtime`] for the
/// cooperative path production actually runs.
fn handle_ws_response(
    ctx: &sema_core::EvalContext,
    response_val: &Value,
    respond: tokio::sync::oneshot::Sender<ServerResponse>,
) {
    use sema_core::{call_callback, NativeFn};
    use std::cell::RefCell;
    use std::rc::Rc;

    let map = response_val.as_map_rc().unwrap();
    let ws_handler = map.get(&Value::keyword("__ws_handler")).cloned().unwrap();

    // Create bidirectional channels
    let (in_tx, in_rx) = tokio::sync::mpsc::channel::<WsMsg>(256); // client -> evaluator
    let (incoming_generation_tx, _incoming_generation) = tokio::sync::watch::channel(0_u64);
    let (out_tx, out_rx) = tokio::sync::mpsc::channel::<WsMsg>(256); // evaluator -> client

    // Send channels to axum for WebSocket bridging
    let _ = respond.send(ServerResponse::WebSocket {
        incoming_tx: in_tx,
        incoming_generation: incoming_generation_tx.clone(),
        outgoing_rx: out_rx,
    });

    // Build the connection map for the Sema handler: {:send fn :recv fn :close fn}
    //
    // Share a single outgoing sender between `send` and `close`. axum's send
    // task only exits (and the socket only closes) when the *last* `Sender` is
    // dropped, so `ws/close` must release the sole sender — not a throwaway
    // clone. Mirrors the `in_rx` Option pattern below.
    //
    // NOTE: `ws/send`/`ws/recv` below use bounded blocking_send/blocking_recv,
    // which are correct for the typical handler (runs on the evaluator thread,
    // no nested runtime). But a WS handler that drives `llm/stream` (whose
    // callback fires inside the provider's block_on) would hit the same "block
    // within a runtime" panic the SSE path fixed with an unbounded channel. Left
    // as a known limitation — WS+llm/stream isn't a shipped pattern yet.
    let out_tx = Rc::new(RefCell::new(Some(out_tx)));
    let out_tx_for_send = out_tx.clone();
    let send_fn = Value::native_fn(NativeFn::simple("ws/send", move |args| {
        check_arity!(args, "ws/send", 1);
        // A string sends a text frame; a bytevector sends a binary frame.
        let msg = if let Some(s) = args[0].as_str() {
            WsMsg::Text(s.to_string())
        } else if let Some(b) = args[0].as_bytevector() {
            WsMsg::Binary(b.to_vec())
        } else {
            return Err(SemaError::type_error(
                "string or bytevector",
                args[0].type_name(),
            ));
        };
        let guard = out_tx_for_send.borrow();
        let tx = guard
            .as_ref()
            .ok_or_else(|| SemaError::eval("WebSocket closed"))?;
        tx.blocking_send(msg)
            .map_err(|_| SemaError::eval("WebSocket closed"))?;
        Ok(Value::nil())
    }));

    // Wrap receiver in Rc<RefCell<Option<...>>> since NativeFn closures must be Fn (not FnOnce)
    let in_rx = Rc::new(RefCell::new(Some(in_rx)));
    let in_rx_for_recv = in_rx.clone();
    let recv_fn = Value::native_fn(NativeFn::simple("ws/recv", move |args| {
        check_arity!(args, "ws/recv", 0);
        let mut rx_opt = in_rx_for_recv.borrow_mut();
        if let Some(rx) = rx_opt.as_mut() {
            match rx.blocking_recv() {
                // Text frames surface as strings, binary frames as bytevectors.
                Some(WsMsg::Text(s)) => Ok(Value::string(&s)),
                Some(WsMsg::Binary(b)) => Ok(Value::bytevector(b)),
                None => {
                    // Channel closed — remove the receiver
                    *rx_opt = None;
                    Ok(Value::nil())
                }
            }
        } else {
            Ok(Value::nil())
        }
    }));

    let out_tx_for_close = out_tx;
    let in_rx_for_close = in_rx;
    let incoming_generation_for_close = incoming_generation_tx;
    let close_fn = Value::native_fn(NativeFn::simple("ws/close", move |args| {
        check_arity!(args, "ws/close", 0);
        // Take + drop the sole outgoing sender: this closes `out_rx`, so axum's
        // send task exits and the socket actually closes.
        out_tx_for_close.borrow_mut().take();
        // Drop the incoming receiver too.
        close_server_ws_receiver(&in_rx_for_close, &incoming_generation_for_close);
        Ok(Value::nil())
    }));

    let mut conn_map = BTreeMap::new();
    conn_map.insert(Value::keyword("send"), send_fn);
    conn_map.insert(Value::keyword("recv"), recv_fn);
    conn_map.insert(Value::keyword("close"), close_fn);
    let conn = Value::map(conn_map);

    // Call the WebSocket handler with the connection map
    match call_callback(ctx, &ws_handler, &[conn]) {
        Ok(_) => {}
        Err(e) => {
            eprintln!("ws handler error: {e}");
        }
    }
}

/// Registers `__http-serve-run`, NOT `http/serve` — the user-facing
/// `http/serve` is a Sema wrapper defined in prelude.rs that mints the
/// per-connection dispatch factory fresh on every call and forwards to this
/// native. See that wrapper's doc comment for why (a cached factory, stored
/// anywhere longer-lived than one call, pins its compiling `Interpreter`'s
/// global env forever).
fn register_serve(env: &sema_core::Env, sandbox: &sema_core::Sandbox) {
    use sema_core::{intern, Caps, EvalContext, NativeFn};

    if sandbox.is_unrestricted() {
        env.set(
            intern("__http-serve-run"),
            Value::native_fn(NativeFn::with_ctx_runtime(
                "__http-serve-run",
                |ctx: &EvalContext, args: &[Value]| http_serve_impl(ctx, args),
                |_ctx, args| http_serve_runtime_impl(args),
            )),
        );
    } else {
        let sandbox = sandbox.clone();
        let sandbox_runtime = sandbox.clone();
        env.set(
            intern("__http-serve-run"),
            Value::native_fn(NativeFn::with_ctx_runtime(
                "__http-serve-run",
                move |ctx: &EvalContext, args: &[Value]| {
                    sandbox.check(Caps::NETWORK, "http/serve")?;
                    http_serve_impl(ctx, args)
                },
                move |_ctx, args| {
                    sandbox_runtime.check(Caps::NETWORK, "http/serve")?;
                    http_serve_runtime_impl(args)
                },
            )),
        );
    }
}

struct ServerHostGuard {
    abort: Option<sema_core::AbortHook>,
}

impl ServerHostGuard {
    fn new(abort: sema_core::AbortHook) -> Self {
        Self { abort: Some(abort) }
    }
}

impl Drop for ServerHostGuard {
    fn drop(&mut self) {
        if let Some(abort) = self.abort.take() {
            abort();
        }
    }
}

struct ServeReceivers {
    requests: tokio::sync::mpsc::Receiver<ServerRequest>,
    lifecycle: tokio::sync::mpsc::UnboundedReceiver<Arc<ServeRequestLifecycle>>,
}

/// `http/serve`'s handler/dispatch-factory + bound request channels, once
/// bind+listen succeeds. Shared between the sync (serial `blocking_recv`) and
/// runtime (concurrent accept-loop) dispatch paths — everything up to "the
/// server is listening and the caller has been notified" is identical
/// between them.
struct ServeSetup {
    handler: Value,
    /// The per-connection dispatch factory — see prelude.rs's `http/serve`
    /// wrapper. Unused by the legacy sync path (`http_serve_impl`, which
    /// dispatches inline exactly as before); threaded through only so
    /// `__http-serve-run`'s arg parsing is shared between both paths.
    factory: Value,
    receivers: ServeReceivers,
    host: ServerHostGuard,
}

/// Calls a Sema function value with args, used only for `http_serve_setup`'s
/// `:on-listen` invocation — see its doc comment for the sync/runtime split.
type ServeInvoke<'a> = &'a dyn Fn(&Value, &[Value]) -> Result<Value, SemaError>;

/// Parse options, bind + spawn the axum server, and wait for it to come up.
///
/// Registered as `__http-serve-run`, NOT `http/serve` directly — see
/// prelude.rs's `http/serve` wrapper for why: `args[0]` is the user's
/// `handler`, `args[1]` is the per-connection dispatch factory the wrapper
/// mints fresh on every call (a plain argument, not a value stored anywhere
/// persistent — see that doc comment for the leak a cached version of this
/// had), and `args[2]` is the optional options map.
///
/// `invoke` calls a Sema function value with args — used only for the
/// `:on-listen` callback. The sync caller passes its own `EvalContext`
/// directly; the runtime caller (no `EvalContext` of its own — the runtime
/// ABI only threads `NativeCallContext`) routes through the shared
/// `STDLIB_CTX` via `with_stdlib_ctx`, the same seam `http/router/dispatch`'s
/// runtime path uses for its handler calls.
fn http_serve_setup(args: &[Value], invoke: ServeInvoke<'_>) -> Result<ServeSetup, SemaError> {
    // `http/serve`'s sync dispatch path below runs its own blocking accept
    // loop on THIS thread (`rx.blocking_recv()`) for the life of the server —
    // by design at top level, where it's the only thing this thread will ever
    // do again. The concurrent runtime dispatch path (`http_serve_runtime_impl`)
    // fixes that for both PLAIN HTTP and WebSocket connections (see
    // docs/deferred.md "SRV-1"): the accept loop parks cooperatively on a
    // re-arming `WaitKind::External` instead of blocking, each connection runs
    // its own spawned task, and a WebSocket handler's `ws/recv` suspends
    // cooperatively too (`handle_ws_response_runtime`) — so a slow/parked
    // handler, plain or WebSocket, does not stall its siblings, and
    // `http/serve` composes inside `async/spawn`. The acceptance suite in
    // `crates/sema/tests/http_serve_concurrent_test.rs` is the regression
    // gate for this claim.
    if args.len() < 2 || args.len() > 3 {
        return Err(SemaError::arity(
            "http/serve",
            "1-2",
            args.len().saturating_sub(1),
        ));
    }

    let handler = args[0].clone();
    let factory = args[1].clone();

    // Parse options map (arg 2): {:port 3000 :host "0.0.0.0"
    //                             :port-fallback true :on-listen (fn (info) ...)}
    let mut port: u16 = 3000;
    let mut host = "0.0.0.0".to_string();
    // Off by default: `http/serve` fails fast on a taken port, preserving the
    // long-standing contract. First-party servers (notebook, web dev server)
    // opt in so users get automatic fallback there.
    let mut port_fallback = false;
    let mut on_listen: Option<Value> = None;

    if args.len() == 3 {
        if let Some(opts) = args[2].as_map_rc() {
            if let Some(p) = opts.get(&Value::keyword("port")).and_then(|v| v.as_int()) {
                port = parse_port(p)?;
            }
            if let Some(h) = opts.get(&Value::keyword("host")).and_then(|v| v.as_str()) {
                host = h.to_string();
            }
            if let Some(f) = opts.get(&Value::keyword("port-fallback")) {
                port_fallback = f.is_truthy();
            }
            if let Some(cb) = opts.get(&Value::keyword("on-listen")) {
                if cb.as_native_fn_ref().is_some() || cb.as_lambda_rc().is_some() {
                    on_listen = Some(cb.clone());
                }
            }
        }
    }

    // Request admission remains bounded. Lifecycle notifications are
    // unbounded because they originate in Drop and must never await or be lost
    // merely because the request queue is full.
    let (tx, request_rx) = tokio::sync::mpsc::channel::<ServerRequest>(256);
    let (lifecycle_tx, lifecycle_rx) =
        tokio::sync::mpsc::unbounded_channel::<Arc<ServeRequestLifecycle>>();

    // Create a std sync channel for the ready signal, carrying the port the
    // server actually bound to (may differ from `port` when fallback kicks in).
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<u16, String>>();

    let bind_host = host.clone();
    let bind_port = port;

    // Spawn the bind+serve future (Send + 'static) onto the process-wide I/O
    // pool. Its abort hook is retained by the server root and invoked when the
    // root's final continuation state drops.
    let abort = sema_io::io_spawn(async move {
        let tx = tx;
        let lifecycle_tx = lifecycle_tx;

        // Build the axum router with a fallback handler that catches all requests.
        // We manually extract WebSocketUpgrade from request parts when needed.
        let app = axum::Router::new().fallback(move |req: axum::extract::Request| {
            let tx = tx.clone();
            let lifecycle_tx = lifecycle_tx.clone();
            async move {
                // Try to extract WebSocketUpgrade from request parts
                use axum::extract::FromRequestParts;
                let (mut parts, body) = req.into_parts();
                let ws_upgrade: Option<axum::extract::ws::WebSocketUpgrade> =
                    axum::extract::ws::WebSocketUpgrade::from_request_parts(&mut parts, &())
                        .await
                        .ok();
                let req = axum::extract::Request::from_parts(parts, body);
                handle_axum_request(ws_upgrade, req, tx, lifecycle_tx).await
            }
        });

        // Bind the TCP listener. With fallback enabled, walk to the next
        // free port on AddrInUse; otherwise bind the requested port only.
        let bind_result = if port_fallback {
            sema_core::net::bind_with_fallback(&bind_host, bind_port, 100).and_then(
                |(std_listener, actual)| {
                    std_listener.set_nonblocking(true)?;
                    let listener = tokio::net::TcpListener::from_std(std_listener)?;
                    Ok((listener, actual))
                },
            )
        } else {
            let addr = format!("{bind_host}:{bind_port}");
            tokio::net::TcpListener::bind(&addr)
                .await
                .map(|listener| (listener, bind_port))
        };
        let (listener, fallback_port) = match bind_result {
            Ok(pair) => pair,
            Err(e) => {
                let _ = ready_tx.send(Err(format!("bind {bind_host}:{bind_port}: {e}")));
                return;
            }
        };
        let actual_port = listener
            .local_addr()
            .map(|address| address.port())
            .unwrap_or(fallback_port);

        // Signal success with the port actually bound
        let _ = ready_tx.send(Ok(actual_port));

        // Run the server
        let _ = axum::serve(listener, app).await;
    });
    let host_guard = ServerHostGuard::new(abort);

    // Wait for the ready signal (carrying the actual bound port) from the thread
    let actual_port = match ready_rx.recv() {
        Ok(Ok(p)) => p,
        Ok(Err(e)) => {
            return Err(SemaError::Io(e));
        }
        Err(_) => {
            return Err(SemaError::eval(
                "http/serve: server thread died before binding",
            ));
        }
    };

    eprintln!("Listening on {host}:{actual_port}");

    // Hand the caller the address actually bound (host/port may differ from the
    // request when :port-fallback picked the next free port) so it can print a
    // URL or open a browser.
    if let Some(cb) = &on_listen {
        let mut info = BTreeMap::new();
        info.insert(Value::keyword("host"), Value::string(&host));
        info.insert(Value::keyword("port"), Value::int(actual_port as i64));
        info.insert(
            Value::keyword("url"),
            Value::string(&format!("http://{host}:{actual_port}")),
        );
        if let Err(e) = invoke(cb, &[Value::map(info)]) {
            eprintln!("http/serve on-listen handler error: {e}");
        }
    }

    Ok(ServeSetup {
        handler,
        factory,
        receivers: ServeReceivers {
            requests: request_rx,
            lifecycle: lifecycle_rx,
        },
        host: host_guard,
    })
}

/// Legacy/non-quantum dispatch path: serially drains `rx` on this thread. Used
/// as `http/serve`'s value-ABI fallback (`NativeFn::with_ctx_runtime`'s `func`)
/// — reachable only when `http/serve` runs OUTSIDE any unified-runtime quantum,
/// which the shipped product never does (every eval entry point drives the
/// runtime; see `docs/deferred.md` "Unified runtime migration"). Kept
/// byte-for-byte equivalent to pre-SRV-1 behavior as the safety net for that
/// path, since it cannot suspend (no runtime to park it).
fn http_serve_impl(ctx: &sema_core::EvalContext, args: &[Value]) -> Result<Value, SemaError> {
    use sema_core::call_callback;

    let ServeSetup {
        handler,
        factory: _,
        receivers,
        host,
    } = http_serve_setup(args, &|f, a| call_callback(ctx, f, a))?;
    let _host = host;
    let ServeReceivers {
        requests: mut rx,
        lifecycle: lifecycle_rx,
    } = receivers;
    // The legacy path cannot issue a runtime CancelPromise. Drop its lifecycle
    // receiver so disconnect notifications fail immediately instead of
    // accumulating behind this serial loop.
    drop(lifecycle_rx);

    // Main evaluator loop: read requests from channel, call handler, send response.
    //
    // Single-consumer by construction: every connection (HTTP or WebSocket)
    // funnels through this one `rx`, and this loop handles ONE `ServerRequest`
    // at a time on the evaluator thread before looping back to `blocking_recv`
    // for the next. A WebSocket handler's `(:recv conn)` (`ws/recv` above,
    // `blocking_recv` on its own per-connection channel) only ever gets called
    // from inside `call_callback` below — so a WS handler idling in `ws/recv`
    // waiting on its client keeps this loop from picking up the NEXT request
    // (HTTP or WS) until that client sends something or disconnects. This
    // serial loop is retained ONLY as the non-runtime-quantum fallback (see
    // this function's doc comment) — the runtime dispatch path below is what
    // production actually runs, and fixes exactly this for plain HTTP.
    while let Some(req) = rx.blocking_recv() {
        match req {
            ServerRequest::Http {
                lifecycle: _,
                raw,
                respond,
            } => {
                let request_val = raw_request_to_value(&raw);
                match call_callback(ctx, &handler, &[request_val]) {
                    Ok(response_val) => {
                        // Check for SSE stream marker
                        if is_stream_response(&response_val) {
                            handle_sse_response(ctx, &response_val, respond);
                        } else if is_websocket_response(&response_val) {
                            handle_ws_response(ctx, &response_val, respond);
                        } else if is_file_response(&response_val) {
                            handle_file_response(&response_val, respond);
                        } else {
                            let raw_resp = value_to_raw_response(&response_val);
                            let _ = respond.send(ServerResponse::Raw(raw_resp));
                        }
                    }
                    Err(e) => {
                        eprintln!("http/serve handler error: {e}");
                        let _ = respond.send(ServerResponse::Raw(RawResponse {
                            status: 500,
                            headers: vec![(
                                "content-type".to_string(),
                                "application/json".to_string(),
                            )],
                            body: format!(
                                "{{\"error\":\"{}\"}}",
                                e.to_string().replace('"', "\\\"")
                            ),
                        }));
                    }
                }
            }
        }
    }

    Ok(Value::nil())
}

/// Completion-kind tag for `http/serve`'s accept-loop External wait ("svac" —
/// serve-accept). Distinct from `http.rs`'s `HTTP_COMPLETION_KIND` (the
/// OUTBOUND `http/*` client) so the two subsystems' offload accounting never
/// share a bucket.
const SERVE_ACCEPT_COMPLETION_KIND: u64 = 0x7376_6163;

enum ServeEvent {
    Request(ServerRequest),
    Lifecycle(Arc<ServeRequestLifecycle>),
    Closed,
}

enum RequestOwner {
    AwaitingPromise { disconnected: bool },
    Running(sema_core::runtime::PromiseId),
}

struct ServeLoopState {
    receivers: std::cell::RefCell<Option<ServeReceivers>>,
    event: std::cell::RefCell<Option<ServeEvent>>,
    owners: std::cell::RefCell<HashMap<ServeRequestId, RequestOwner>>,
    _host: ServerHostGuard,
}

type ServeLoopStateRef = std::rc::Rc<ServeLoopState>;

/// Build the per-request `responder` native: consumes the handler's return
/// value exactly once and routes it to the connection's `respond` channel —
/// the same raw/SSE/WebSocket/file dispatch the legacy serial loop did inline
/// (`http_serve_impl` above), now run from inside the per-connection task the
/// runtime accept loop spawns. `respond` is a `oneshot::Sender`, usable only
/// once, but a `NativeFn` closure is `Fn` not `FnOnce` — the `RefCell<Option<_>>`
/// takes it on first (and only legal) call; a second call is a clear
/// internal-invariant error rather than a silent no-op or panic.
///
/// A handler that RAISES instead of returning never calls this native at all
/// (see the `http/serve` wrapper in prelude.rs), so `respond` is dropped
/// unsent; `handle_axum_request`'s `resp_rx.await` `Err(_)` arm already covers
/// that case with a bounded 500 ("Handler did not respond") — safe, but not
/// byte-identical to the legacy loop's `{"error": "..."}` JSON body. This is
/// the CHOSEN, pinned contract (see `uncaught_handler_error_produces_the_
/// bounded_500_fallback` below): the legacy JSON shape is not documented
/// anywhere (`website/docs/stdlib/web-server.md` only documents the explicit
/// `http/error`/`http/not-found`/etc. constructors, never an implicit
/// uncaught-exception body), so there is no compatibility obligation to
/// restore it, and `server_test.rs`'s `test_http_serve_handler_error` only
/// ever asserted the status code (500), never the body.
///
/// Dual-ABI: the runtime branch routes a WebSocket response through
/// `handle_ws_response_runtime`'s `NativeOutcome::Call` (see its doc comment
/// for why a plain synchronous `call_callback` can't let the handler's
/// `(:recv conn)` suspend cooperatively); every other response shape needs no
/// suspension, so both branches build it identically. The legacy branch
/// (reachable only outside a runtime quantum — dead in the shipped product)
/// keeps calling the WS handler synchronously via `handle_ws_response`.
fn make_responder_native(
    respond: tokio::sync::oneshot::Sender<ServerResponse>,
    lifecycle: Arc<ServeRequestLifecycle>,
) -> Value {
    use sema_core::runtime::NativeResult;
    use sema_core::NativeFn;
    let respond = std::rc::Rc::new(std::cell::RefCell::new(Some(respond)));
    let respond_runtime = respond.clone();
    // Both ABI closures live inside one NativeFn allocation. Holding one Rc in
    // each makes HandlerFinishedLease drop only when that final logical native
    // owner is reclaimed; cloning the responder Value does not clone the lease.
    let finished = std::rc::Rc::new(HandlerFinishedLease(lifecycle));
    let finished_runtime = finished.clone();
    Value::native_fn(NativeFn::with_ctx_runtime(
        "http/serve/responder",
        move |ctx: &sema_core::EvalContext, args: &[Value]| {
            let _finished = &finished;
            check_arity!(args, "http/serve/responder", 1);
            let response_val = &args[0];
            let respond = respond.borrow_mut().take().ok_or_else(|| {
                SemaError::eval("http/serve: internal: handler response already sent")
            })?;
            if is_stream_response(response_val) {
                handle_sse_response(ctx, response_val, respond);
            } else if is_websocket_response(response_val) {
                handle_ws_response(ctx, response_val, respond);
            } else if is_file_response(response_val) {
                handle_file_response(response_val, respond);
            } else {
                let raw_resp = value_to_raw_response(response_val);
                let _ = respond.send(ServerResponse::Raw(raw_resp));
            }
            Ok(Value::nil())
        },
        move |_ctx, args| -> NativeResult {
            let _finished = &finished_runtime;
            check_arity!(args, "http/serve/responder", 1);
            let response_val = args[0].clone();
            let respond = respond_runtime.borrow_mut().take().ok_or_else(|| {
                SemaError::eval("http/serve: internal: handler response already sent")
            })?;
            if is_websocket_response(&response_val) {
                return handle_ws_response_runtime(&response_val, respond);
            }
            if is_stream_response(&response_val) {
                sema_core::with_stdlib_ctx(|c| handle_sse_response(c, &response_val, respond));
                return Ok(NativeOutcome::Return(Value::nil()));
            }
            if is_file_response(&response_val) {
                handle_file_response(&response_val, respond);
                return Ok(NativeOutcome::Return(Value::nil()));
            }
            let raw_resp = value_to_raw_response(&response_val);
            let _ = respond.send(ServerResponse::Raw(raw_resp));
            Ok(NativeOutcome::Return(Value::nil()))
        },
    ))
}

/// Build the next accept-loop External wait: move both receivers out of the
/// loop state, select between admitted requests and lifecycle notifications
/// off the VM thread, then restore them before [`AcceptLoopContinuation`]
/// dispatches the event. This is the re-arming shape the SRV-1 liveness spike
/// (`crates/sema-vm/src/runtime/tests.rs`, `srv1_spike_*`) proves deadlock-free:
/// a task parked here alone still drives the runtime to `Idle`, never a false
/// `Quiescent`/deadlock, and re-arming across many iterations leaks nothing.
fn next_accept_wait(
    handler: Value,
    factory: Value,
    state: ServeLoopStateRef,
) -> sema_core::runtime::NativeResult {
    use sema_core::runtime::CompletionKind;

    let receivers = state.receivers.borrow_mut().take().ok_or_else(|| {
        SemaError::eval("http/serve: internal: accept-loop receivers missing (already parked?)")
    })?;
    let decode_state = state.clone();
    let continuation: Box<dyn sema_core::runtime::NativeContinuation> =
        Box::new(AcceptLoopContinuation {
            handler: handler.clone(),
            factory: factory.clone(),
            state,
        });
    let kind = CompletionKind::try_from_raw(SERVE_ACCEPT_COMPLETION_KIND)
        .expect("http/serve accept completion kind is nonzero");
    crate::runtime_offload::external_io_async_try_with_continuation(
        "http/serve",
        kind,
        "http/serve/accept",
        move |(receivers, event): (ServeReceivers, ServeEvent)| -> Result<Value, SemaError> {
            *decode_state.receivers.borrow_mut() = Some(receivers);
            *decode_state.event.borrow_mut() = Some(event);
            Ok(Value::nil())
        },
        continuation,
        move || {
            let ServeReceivers {
                mut requests,
                mut lifecycle,
            } = receivers;
            async move {
                let event = tokio::select! {
                    request = requests.recv() => request.map_or(ServeEvent::Closed, ServeEvent::Request),
                    notification = lifecycle.recv() => {
                        notification.map_or(ServeEvent::Closed, ServeEvent::Lifecycle)
                    }
                };
                Ok::<_, String>((
                    ServeReceivers {
                        requests,
                        lifecycle,
                    },
                    event,
                ))
            }
        },
    )
}

/// Resumes the accept-loop's External wait. A request mints and spawns its
/// handler task through the per-connection dispatch factory; a lifecycle wake
/// either records an early disconnect, cancels the matching promise, or removes
/// a finished owner.
/// Traces `handler`/`factory`: both are live `Value`s held across the External
/// park, exactly like `RouterDecoder`'s route handlers.
///
/// The factory does the `async/spawn` itself, in compiled Sema bytecode,
/// rather than this continuation issuing a bare `RuntimeRequest::Spawn` —
/// deliberately: `spawn_via_registry` (`sema-vm/src/runtime/state.rs`) has a
/// `ReturnOwner::VmResume` fast path that silently discards any
/// caller-supplied continuation OTHER than `async/spawn`'s own trivial
/// default, injecting the settled promise straight onto the parked VM's stack
/// instead. Every hop chained off a plain top-level call keeps
/// `owner == VmResume` the whole way (confirmed empirically: a version of this
/// code that issued `RuntimeRequest::Spawn` with a custom re-arm continuation
/// had that continuation silently skipped — the spawned task still ran
/// correctly, but the accept loop's OWN promise, not the connection's
/// response, became `http/serve`'s call result, and the loop stopped
/// re-arming after one request). Routing the spawn through compiled bytecode
/// (see prelude.rs's factory) sidesteps the gap entirely — no bare
/// `RuntimeRequest::Spawn` ever crosses this Rust continuation boundary — at
/// the cost of one Sema-level indirection, and needs no sema-vm change.
struct AcceptLoopContinuation {
    handler: Value,
    factory: Value,
    state: ServeLoopStateRef,
}

impl sema_core::runtime::Trace for AcceptLoopContinuation {
    fn trace(&self, sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        sink(sema_core::cycle::GcEdge::Value(&self.handler));
        sink(sema_core::cycle::GcEdge::Value(&self.factory));
        true
    }
}

impl sema_core::runtime::NativeContinuation for AcceptLoopContinuation {
    fn resume(
        self: Box<Self>,
        _context: &mut sema_core::runtime::NativeCallContext<'_>,
        input: sema_core::runtime::ResumeInput,
    ) -> sema_core::runtime::NativeResult {
        use sema_core::runtime::{NativeCall, NativeOutcome, ResumeInput};
        match input {
            ResumeInput::Returned(_) => {
                let event = self.state.event.borrow_mut().take().ok_or_else(|| {
                    SemaError::eval("http/serve: internal: accept-loop event missing after wake")
                })?;
                match event {
                    ServeEvent::Closed => Ok(NativeOutcome::Return(Value::nil())),
                    ServeEvent::Request(ServerRequest::Http {
                        lifecycle,
                        raw,
                        respond,
                    }) => {
                        let disconnected = lifecycle.is_disconnected();
                        self.state
                            .owners
                            .borrow_mut()
                            .entry(lifecycle.id)
                            .and_modify(|owner| {
                                if let RequestOwner::AwaitingPromise {
                                    disconnected: pending,
                                } = owner
                                {
                                    *pending |= disconnected;
                                }
                            })
                            .or_insert(RequestOwner::AwaitingPromise { disconnected });
                        let request_val = raw_request_to_value(&raw);
                        let responder_val = make_responder_native(respond, Arc::clone(&lifecycle));
                        Ok(NativeOutcome::Call(NativeCall {
                            callable: self.factory.clone(),
                            args: vec![self.handler.clone(), request_val, responder_val],
                            continuation: Box::new(AfterDispatchContinuation {
                                handler: self.handler,
                                factory: self.factory,
                                state: self.state,
                                request_id: lifecycle.id,
                            }),
                        }))
                    }
                    ServeEvent::Lifecycle(lifecycle) => {
                        handle_lifecycle_event(self.handler, self.factory, self.state, lifecycle)
                    }
                }
            }
            ResumeInput::Failed(error) => Err(error),
            ResumeInput::Cancelled(reason) => Err(SemaError::eval(format!(
                "http/serve: accept loop was cancelled ({reason:?})"
            ))),
            ResumeInput::Runtime(_) => Err(SemaError::eval(
                "http/serve: accept loop continuation received an unexpected runtime response",
            )),
        }
    }
}

fn handle_lifecycle_event(
    handler: Value,
    factory: Value,
    state: ServeLoopStateRef,
    lifecycle: Arc<ServeRequestLifecycle>,
) -> sema_core::runtime::NativeResult {
    let promise = apply_lifecycle_event(&mut state.owners.borrow_mut(), &lifecycle);
    match promise {
        Some(promise) => cancel_request_and_rearm(handler, factory, state, promise),
        None => next_accept_wait(handler, factory, state),
    }
}

fn apply_lifecycle_event(
    owners: &mut HashMap<ServeRequestId, RequestOwner>,
    lifecycle: &ServeRequestLifecycle,
) -> Option<sema_core::runtime::PromiseId> {
    if lifecycle.is_finished() {
        owners.remove(&lifecycle.id);
        return None;
    }
    if !lifecycle.is_disconnected() {
        return None;
    }

    match owners.remove(&lifecycle.id) {
        Some(RequestOwner::Running(promise)) => Some(promise),
        Some(RequestOwner::AwaitingPromise { .. }) | None => {
            owners.insert(
                lifecycle.id,
                RequestOwner::AwaitingPromise { disconnected: true },
            );
            None
        }
    }
}

fn cancel_request_and_rearm(
    handler: Value,
    factory: Value,
    state: ServeLoopStateRef,
    promise: sema_core::runtime::PromiseId,
) -> sema_core::runtime::NativeResult {
    use sema_core::runtime::{NativeOutcome, RuntimeRequest};

    Ok(NativeOutcome::Runtime(RuntimeRequest::CancelPromise {
        promise,
        continuation: Box::new(CancelRequestContinuation {
            handler,
            factory,
            state,
        }),
    }))
}

struct CancelRequestContinuation {
    handler: Value,
    factory: Value,
    state: ServeLoopStateRef,
}

impl sema_core::runtime::Trace for CancelRequestContinuation {
    fn trace(&self, sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        sink(sema_core::cycle::GcEdge::Value(&self.handler));
        sink(sema_core::cycle::GcEdge::Value(&self.factory));
        true
    }
}

impl sema_core::runtime::NativeContinuation for CancelRequestContinuation {
    fn resume(
        self: Box<Self>,
        _context: &mut sema_core::runtime::NativeCallContext<'_>,
        input: sema_core::runtime::ResumeInput,
    ) -> sema_core::runtime::NativeResult {
        use sema_core::runtime::{ResumeInput, RuntimeResponse};

        match input {
            ResumeInput::Runtime(RuntimeResponse::Cancelled(_)) | ResumeInput::Failed(_) => {
                next_accept_wait(self.handler, self.factory, self.state)
            }
            ResumeInput::Cancelled(reason) => Err(SemaError::eval(format!(
                "http/serve: accept loop was cancelled while cancelling a request ({reason:?})"
            ))),
            _ => Err(SemaError::eval(
                "http/serve: request cancellation returned an unexpected runtime response",
            )),
        }
    }
}

/// Resumes once the dispatch factory returns — the per-connection
/// handler task is already minted AND spawned. Its promise is retained in the
/// request-owner table so a future-drop notification can cancel exactly that
/// handler; the response still reaches the client through `responder`. The
/// re-arm itself is what the SRV-1 spike's
/// `srv1_spike_rearm_indefinite` proves terminates cleanly and leaks nothing
/// across many iterations. Traces `handler`/`factory` for the same reason as
/// [`AcceptLoopContinuation`] (still held across this stage's `Call`).
struct AfterDispatchContinuation {
    handler: Value,
    factory: Value,
    state: ServeLoopStateRef,
    request_id: ServeRequestId,
}

impl sema_core::runtime::Trace for AfterDispatchContinuation {
    fn trace(&self, sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        sink(sema_core::cycle::GcEdge::Value(&self.handler));
        sink(sema_core::cycle::GcEdge::Value(&self.factory));
        true
    }
}

impl sema_core::runtime::NativeContinuation for AfterDispatchContinuation {
    fn resume(
        self: Box<Self>,
        _context: &mut sema_core::runtime::NativeCallContext<'_>,
        input: sema_core::runtime::ResumeInput,
    ) -> sema_core::runtime::NativeResult {
        use sema_core::runtime::ResumeInput;
        match input {
            ResumeInput::Returned(promise) => {
                let promise = match promise.view() {
                    sema_core::ValueView::AsyncPromise(promise) => promise.id,
                    _ => {
                        return Err(SemaError::eval(
                            "http/serve: dispatch-task factory did not return an async promise",
                        ));
                    }
                };
                let disconnected = {
                    let mut owners = self.state.owners.borrow_mut();
                    match owners.remove(&self.request_id) {
                        Some(RequestOwner::AwaitingPromise { disconnected }) => {
                            if !disconnected {
                                owners.insert(self.request_id, RequestOwner::Running(promise));
                            }
                            disconnected
                        }
                        Some(RequestOwner::Running(_)) | None => {
                            return Err(SemaError::eval(
                                "http/serve: request owner missing while dispatching handler",
                            ));
                        }
                    }
                };
                if disconnected {
                    cancel_request_and_rearm(self.handler, self.factory, self.state, promise)
                } else {
                    next_accept_wait(self.handler, self.factory, self.state)
                }
            }
            ResumeInput::Failed(error) => Err(error),
            ResumeInput::Cancelled(reason) => Err(SemaError::eval(format!(
                "http/serve: accept loop was cancelled while dispatching a handler task ({reason:?})"
            ))),
            ResumeInput::Runtime(_) => Err(SemaError::eval(
                "http/serve: dispatch-task factory call returned an unexpected runtime response",
            )),
        }
    }
}

/// Concurrent runtime-ABI dispatch path: `http/serve`'s accept loop, re-arming
/// on a `WaitKind::External` per request (see [`next_accept_wait`]) instead of
/// blocking the VM thread. Each connection's handler runs as its own spawned
/// task (scope isolation is free here — `spawn_via_registry` already
/// fresh-defaults every spawned task's dynamic scopes, the same seam
/// `async/spawn` uses), so a slow/parked handler does not stall its
/// siblings. This is what `http/serve` runs in the shipped product
/// (every eval entry point drives the unified runtime).
fn http_serve_runtime_impl(args: &[Value]) -> sema_core::runtime::NativeResult {
    let ServeSetup {
        handler,
        factory,
        receivers,
        host,
    } = http_serve_setup(args, &|f, a| {
        sema_core::with_stdlib_ctx(|c| sema_core::call_callback(c, f, a))
    })?;
    let state = std::rc::Rc::new(ServeLoopState {
        receivers: std::cell::RefCell::new(Some(receivers)),
        event: std::cell::RefCell::new(None),
        owners: std::cell::RefCell::new(HashMap::new()),
        _host: host,
    });
    next_accept_wait(handler, factory, state)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_serve_loop_state() -> ServeLoopStateRef {
        std::rc::Rc::new(ServeLoopState {
            receivers: std::cell::RefCell::new(None),
            event: std::cell::RefCell::new(None),
            owners: std::cell::RefCell::new(HashMap::new()),
            _host: ServerHostGuard { abort: None },
        })
    }

    #[test]
    fn server_host_guard_aborts_only_after_final_owner_drops() {
        let aborts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let aborts_from_hook = Arc::clone(&aborts);
        let guard = std::rc::Rc::new(ServerHostGuard::new(Box::new(move || {
            aborts_from_hook.fetch_add(1, Ordering::SeqCst);
        })));
        let second_owner = std::rc::Rc::clone(&guard);

        drop(guard);
        assert_eq!(aborts.load(Ordering::SeqCst), 0);
        drop(second_owner);
        assert_eq!(aborts.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn handler_finished_lease_signals_only_after_final_logical_owner_drops() {
        let (lifecycle_tx, mut lifecycle_rx) = tokio::sync::mpsc::unbounded_channel();
        let lifecycle = ServeRequestLifecycle::new(ServeRequestId(7), lifecycle_tx);
        let lease = std::rc::Rc::new(HandlerFinishedLease(Arc::clone(&lifecycle)));
        let second_owner = std::rc::Rc::clone(&lease);

        drop(lease);
        assert!(matches!(
            lifecycle_rx.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ));
        drop(second_owner);
        let notification = lifecycle_rx
            .try_recv()
            .expect("final lease owner publishes handler completion");
        assert_eq!(notification.id, lifecycle.id);
        assert!(notification.is_finished());
    }

    #[test]
    fn lifecycle_finished_and_disconnected_reordering_leaves_no_owner() {
        let (lifecycle_tx, _lifecycle_rx) = tokio::sync::mpsc::unbounded_channel();
        let finished_first = ServeRequestLifecycle::new(ServeRequestId(8), lifecycle_tx.clone());
        let mut owners = HashMap::from([(
            finished_first.id,
            RequestOwner::AwaitingPromise {
                disconnected: false,
            },
        )]);
        finished_first.mark_finished();
        finished_first.mark_disconnected();
        assert!(apply_lifecycle_event(&mut owners, &finished_first).is_none());
        assert!(!owners.contains_key(&finished_first.id));

        let disconnected_first = ServeRequestLifecycle::new(ServeRequestId(9), lifecycle_tx);
        disconnected_first.mark_disconnected();
        assert!(apply_lifecycle_event(&mut owners, &disconnected_first).is_none());
        assert!(matches!(
            owners.get(&disconnected_first.id),
            Some(RequestOwner::AwaitingPromise { disconnected: true })
        ));
        disconnected_first.mark_finished();
        assert!(apply_lifecycle_event(&mut owners, &disconnected_first).is_none());
        assert!(!owners.contains_key(&disconnected_first.id));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn dropping_axum_request_future_publishes_disconnect() {
        let (request_tx, mut request_rx) = tokio::sync::mpsc::channel(1);
        let (lifecycle_tx, mut lifecycle_rx) = tokio::sync::mpsc::unbounded_channel();
        let request = axum::extract::Request::builder()
            .uri("/drop")
            .body(axum::body::Body::empty())
            .expect("build request");
        let task = tokio::spawn(handle_axum_request(None, request, request_tx, lifecycle_tx));
        let request = request_rx.recv().await.expect("route enqueues request");
        let ServerRequest::Http { lifecycle, .. } = request;

        task.abort();
        let _ = task.await;
        let notification =
            tokio::time::timeout(std::time::Duration::from_secs(1), lifecycle_rx.recv())
                .await
                .expect("request future drop publishes without timing out")
                .expect("lifecycle channel remains open");
        assert_eq!(notification.id, lifecycle.id);
        assert!(notification.is_disconnected());
        assert!(!notification.is_finished());
    }

    // `RouterDecoder` holds the route table's handler `Value`s across the
    // External park while a `:static` directory batch canonicalizes off-thread.
    // Its `Trace` MUST expose each handler as exactly one GC edge (nothing more,
    // nothing less) or the collector could reclaim a live handler mid-flight (or
    // miscount). Count the edges its `trace` emits against the handler `Value`s.
    #[test]
    fn router_decoder_traces_exactly_its_handler_values() {
        use sema_core::runtime::Trace;
        let h1 = Value::string("handler-one");
        let h2 = Value::string("handler-two");
        let decoder = RouterDecoder {
            routes: vec![
                ("get".to_string(), "/a/*".to_string(), h1.clone()),
                ("get".to_string(), "/b/*".to_string(), h2.clone()),
            ],
            indices: vec![0, 1],
        };
        let mut edges = 0usize;
        decoder.trace(&mut |edge| {
            if let sema_core::cycle::GcEdge::Value(_) = edge {
                edges += 1;
            }
        });
        assert_eq!(
            edges, 2,
            "RouterDecoder must trace exactly one edge per route handler"
        );
    }

    // SRV-1 / invariant I2 (CORE-2 GC): `AcceptLoopContinuation` holds the
    // `handler` and `factory` `Value`s live across the accept loop's External
    // park — its `Trace` MUST expose exactly those two as GC edges, or the
    // collector could reclaim a still-in-flight handler/factory closure.
    #[test]
    fn accept_loop_continuation_traces_exactly_handler_and_factory() {
        use sema_core::runtime::Trace;
        let handler = Value::string("handler");
        let factory = Value::string("factory");
        let cont = AcceptLoopContinuation {
            handler: handler.clone(),
            factory: factory.clone(),
            state: empty_serve_loop_state(),
        };
        let mut edges = 0usize;
        cont.trace(&mut |edge| {
            if let sema_core::cycle::GcEdge::Value(_) = edge {
                edges += 1;
            }
        });
        assert_eq!(
            edges, 2,
            "AcceptLoopContinuation must trace exactly handler + factory"
        );
    }

    // Same invariant for `AfterDispatchContinuation`, which carries the same
    // two `Value`s across the `Call` to the dispatch factory.
    #[test]
    fn after_dispatch_continuation_traces_exactly_handler_and_factory() {
        use sema_core::runtime::Trace;
        let handler = Value::string("handler");
        let factory = Value::string("factory");
        let cont = AfterDispatchContinuation {
            handler: handler.clone(),
            factory: factory.clone(),
            state: empty_serve_loop_state(),
            request_id: ServeRequestId(1),
        };
        let mut edges = 0usize;
        cont.trace(&mut |edge| {
            if let sema_core::cycle::GcEdge::Value(_) = edge {
                edges += 1;
            }
        });
        assert_eq!(
            edges, 2,
            "AfterDispatchContinuation must trace exactly handler + factory"
        );
    }

    // SRV-1 piece c / invariant I2: `WsHandlerContinuation` holds no `Value`
    // (the ws handler is consumed by the `NativeCall` it rides in, not kept by
    // the continuation) — its `Trace` MUST report zero edges, or the collector
    // would be asked to trace nonexistent state.
    #[test]
    fn ws_handler_continuation_traces_no_edges() {
        use sema_core::runtime::Trace;
        let cont = WsHandlerContinuation;
        let mut edges = 0usize;
        cont.trace(&mut |edge| {
            if let sema_core::cycle::GcEdge::Value(_) = edge {
                edges += 1;
            }
        });
        assert_eq!(edges, 0, "WsHandlerContinuation must trace no Value edges");
    }

    fn take_server_ws_external_continuation(
        outcome: NativeOutcome,
    ) -> Box<dyn sema_core::runtime::NativeContinuation> {
        use sema_core::runtime::{NativeSuspend, WaitKind};

        let NativeOutcome::Suspend(NativeSuspend { wait, continuation }) = outcome else {
            panic!("server ws/recv must suspend on an empty connected queue");
        };
        assert!(matches!(wait, WaitKind::External(_)));
        continuation
    }

    fn resume_server_ws_continuation(
        continuation: Box<dyn sema_core::runtime::NativeContinuation>,
    ) -> NativeOutcome {
        use sema_core::runtime::{CancellationView, NativeCallContext, ResumeInput, TaskContext};

        let eval_context = sema_core::EvalContext::new();
        let mut task_context = TaskContext::empty();
        let mut native_context = NativeCallContext {
            eval_context: &eval_context,
            task_context: &mut task_context,
            cancellation: CancellationView::default(),
        };
        continuation
            .resume(&mut native_context, ResumeInput::Returned(Value::nil()))
            .expect("server ws/recv continuation must resume")
    }

    #[test]
    fn server_ws_generation_response_wakes_every_receiver_clone() {
        let (incoming_tx, _incoming_rx) = tokio::sync::mpsc::channel::<WsMsg>(1);
        let (incoming_generation, generation_rx) = tokio::sync::watch::channel(0_u64);
        let (_outgoing_tx, outgoing_rx) = tokio::sync::mpsc::channel::<WsMsg>(1);
        let response = ServerResponse::WebSocket {
            incoming_tx,
            incoming_generation,
            outgoing_rx,
        };
        let ServerResponse::WebSocket {
            incoming_generation,
            ..
        } = response
        else {
            unreachable!("constructed a websocket response")
        };
        let first = arm_server_ws_generation(&generation_rx);
        let second = arm_server_ws_generation(&generation_rx);

        sema_io::io_block_on(async {
            let first_wait = wait_for_server_ws_generation(first);
            let second_wait = wait_for_server_ws_generation(second);
            incoming_generation.send_modify(|generation| *generation += 1);
            tokio::time::timeout(std::time::Duration::from_millis(100), async {
                let (first_result, second_result) = tokio::join!(first_wait, second_wait);
                first_result.expect("first generation wait must succeed");
                second_result.expect("second generation wait must succeed");
            })
            .await
            .expect("one generation must wake every receiver clone");
        });
    }

    #[test]
    fn server_ws_generation_dropping_wait_preserves_installed_receiver() {
        let (incoming_tx, incoming_rx) = tokio::sync::mpsc::channel::<WsMsg>(1);
        let (_generation_tx, generation_rx) = tokio::sync::watch::channel(0_u64);
        let in_rx = std::rc::Rc::new(std::cell::RefCell::new(Some(incoming_rx)));
        let wait = wait_for_server_ws_generation(arm_server_ws_generation(&generation_rx));

        drop(wait);
        assert!(
            in_rx.borrow().is_some(),
            "cancelling a wait must leave the VM-owned receiver installed"
        );
        incoming_tx
            .try_send(WsMsg::Text("still-open".to_string()))
            .unwrap();
        let value = try_server_ws_message(&in_rx).expect("queued message must be ready");
        assert_eq!(value.as_str(), Some("still-open"));
    }

    #[test]
    fn server_ws_generation_queued_text_and_binary_win_immediately() {
        let (incoming_tx, incoming_rx) = tokio::sync::mpsc::channel::<WsMsg>(2);
        let in_rx = std::rc::Rc::new(std::cell::RefCell::new(Some(incoming_rx)));
        incoming_tx.try_send(WsMsg::Text("hi".to_string())).unwrap();
        incoming_tx.try_send(WsMsg::Binary(vec![1, 2, 3])).unwrap();

        let text = try_server_ws_message(&in_rx).expect("text must be ready");
        assert_eq!(text.as_str(), Some("hi"));
        let binary = try_server_ws_message(&in_rx).expect("binary must be ready");
        assert_eq!(binary.as_bytevector(), Some([1, 2, 3].as_slice()));
    }

    #[test]
    fn server_ws_generation_exact_suspended_continuation_returns_published_message() {
        let (incoming_tx, incoming_rx) = tokio::sync::mpsc::channel::<WsMsg>(1);
        let (generation_tx, incoming_generation) = tokio::sync::watch::channel(0_u64);
        let in_rx = std::rc::Rc::new(std::cell::RefCell::new(Some(incoming_rx)));
        let continuation = take_server_ws_external_continuation(
            suspend_server_ws_receive(in_rx, incoming_generation)
                .expect("empty connected receive must suspend"),
        );

        incoming_tx
            .try_send(WsMsg::Text("exact-wake".to_string()))
            .unwrap();
        generation_tx.send_modify(|generation| *generation = generation.wrapping_add(1));

        let outcome = resume_server_ws_continuation(continuation);
        assert!(
            matches!(outcome, NativeOutcome::Return(value) if value.as_str() == Some("exact-wake"))
        );
    }

    #[test]
    fn server_ws_generation_disconnect_wakes_and_resolves_nil() {
        let (incoming_tx, incoming_rx) = tokio::sync::mpsc::channel::<WsMsg>(1);
        let (generation_tx, generation_rx) = tokio::sync::watch::channel(0_u64);
        let in_rx = std::rc::Rc::new(std::cell::RefCell::new(Some(incoming_rx)));
        let wait = wait_for_server_ws_generation(arm_server_ws_generation(&generation_rx));

        drop(incoming_tx);
        drop(generation_tx);
        sema_io::io_block_on(async {
            tokio::time::timeout(std::time::Duration::from_millis(100), wait)
                .await
                .expect("closing the generation sender must wake the wait")
                .expect("generation closure is a readiness wake");
        });
        let value = try_server_ws_message(&in_rx).expect("disconnect must be ready");
        assert!(value.is_nil());
        assert!(in_rx.borrow().is_none());
    }

    #[test]
    fn server_ws_generation_shutdown_recheck_runs_after_all_mpsc_senders_drop() {
        let (incoming_tx, incoming_rx) = tokio::sync::mpsc::channel::<WsMsg>(1);
        let (generation_tx, incoming_generation) = tokio::sync::watch::channel(0_u64);
        let in_rx = std::rc::Rc::new(std::cell::RefCell::new(Some(incoming_rx)));

        let first_continuation = take_server_ws_external_continuation(
            suspend_server_ws_receive(in_rx.clone(), incoming_generation.clone())
                .expect("empty connected receive must suspend"),
        );

        // A generation published while the mpsc sender remains installed is a
        // legitimate empty wake. The exact continuation must rearm.
        generation_tx.send_modify(|generation| *generation = generation.wrapping_add(1));
        let rearmed_continuation =
            take_server_ws_external_continuation(resume_server_ws_continuation(first_continuation));
        let shutdown_generation = arm_server_ws_generation(&incoming_generation);

        sema_io::io_block_on(async {
            let recv_task = tokio::spawn(async move {
                let _incoming_tx = incoming_tx;
                futures::future::pending::<()>().await;
            });
            let send_task = tokio::spawn(async {});
            let shutdown =
                finish_server_ws_bridge_tasks(recv_task, send_task, generation_tx.clone());
            let exact_recheck = async move {
                wait_for_server_ws_generation(shutdown_generation)
                    .await
                    .expect("final shutdown generation must wake the recheck");
                let outcome = resume_server_ws_continuation(rearmed_continuation);
                assert!(matches!(outcome, NativeOutcome::Return(value) if value.is_nil()));
            };
            tokio::time::timeout(std::time::Duration::from_millis(100), async {
                tokio::join!(shutdown, exact_recheck);
            })
            .await
            .expect("bridge shutdown and its exact recheck must finish without a lost wake");
        });

        assert!(
            in_rx.borrow().is_none(),
            "the exact post-shutdown recheck must observe mpsc disconnection"
        );
        assert_eq!(
            generation_tx.receiver_count(),
            1,
            "the original watch receiver remains, so the final wake cannot come from channel closure"
        );
    }

    #[test]
    fn server_ws_generation_explicit_close_wakes_pending_receive() {
        let (_incoming_tx, incoming_rx) = tokio::sync::mpsc::channel::<WsMsg>(1);
        let (generation_tx, generation_rx) = tokio::sync::watch::channel(0_u64);
        let in_rx = std::rc::Rc::new(std::cell::RefCell::new(Some(incoming_rx)));
        let wait = wait_for_server_ws_generation(arm_server_ws_generation(&generation_rx));

        close_server_ws_receiver(&in_rx, &generation_tx);
        sema_io::io_block_on(async {
            tokio::time::timeout(std::time::Duration::from_millis(100), wait)
                .await
                .expect("explicit close must wake the pending receive")
                .expect("explicit close generation wait must succeed");
        });
        let value = try_server_ws_message(&in_rx).expect("cleared receiver must be ready");
        assert!(value.is_nil());
    }

    #[test]
    fn server_ws_generation_continuation_rearms_after_coalesced_empty_wake() {
        use sema_core::runtime::{
            CancellationView, NativeCallContext, NativeContinuation, ResumeInput, TaskContext,
            WaitKind,
        };

        let (_incoming_tx, incoming_rx) = tokio::sync::mpsc::channel::<WsMsg>(1);
        let (generation_tx, incoming_generation) = tokio::sync::watch::channel(0_u64);
        generation_tx.send_modify(|generation| *generation += 1);
        let continuation = ServerWsRecvContinuation {
            in_rx: std::rc::Rc::new(std::cell::RefCell::new(Some(incoming_rx))),
            incoming_generation,
        };
        let eval_context = sema_core::EvalContext::new();
        let mut task_context = TaskContext::empty();
        let mut native_context = NativeCallContext {
            eval_context: &eval_context,
            task_context: &mut task_context,
            cancellation: CancellationView::default(),
        };

        let outcome = Box::new(continuation)
            .resume(&mut native_context, ResumeInput::Returned(Value::nil()))
            .expect("coalesced wake must recheck and rearm");
        let NativeOutcome::Suspend(suspend) = outcome else {
            panic!("an empty queue after a coalesced wake must suspend again");
        };
        assert!(matches!(suspend.wait, WaitKind::External(_)));
    }

    #[test]
    fn server_ws_generation_continuation_traces_no_value_edges() {
        use sema_core::runtime::Trace;

        let (_incoming_tx, incoming_rx) = tokio::sync::mpsc::channel::<WsMsg>(1);
        let (_generation_tx, incoming_generation) = tokio::sync::watch::channel(0_u64);
        let continuation = ServerWsRecvContinuation {
            in_rx: std::rc::Rc::new(std::cell::RefCell::new(Some(incoming_rx))),
            incoming_generation,
        };
        let mut edges = 0;
        assert!(continuation.trace(&mut |_| edges += 1));
        assert_eq!(edges, 0);
    }

    // Regression guard for the real production send path: the actual
    // `http/stream/send` native fn (as built by handle_sse_response) must work
    // when invoked from inside a tokio runtime — the exact condition that
    // panicked with the old bounded `blocking_send`. Reverting make_sse_send_fn
    // to blocking_send makes this test panic.
    #[test]
    fn sse_send_fn_works_inside_runtime() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let send_fn = make_sse_send_fn(tx);
        let native = send_fn.as_native_fn_ref().expect("native fn");
        let ctx = sema_core::EvalContext::new();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(async {
            (native.func)(&ctx, &[Value::string("a")])?;
            (native.func)(&ctx, &[Value::string("b")])
        });
        assert!(result.is_ok(), "send must not error/panic inside a runtime");
        assert_eq!(rx.try_recv().unwrap(), "a");
        assert_eq!(rx.try_recv().unwrap(), "b");

        // When the receiver is dropped (client disconnect), send reports the
        // "SSE stream closed" contract rather than panicking.
        drop(rx);
        let closed = (native.func)(&ctx, &[Value::string("c")]);
        assert!(closed.is_err(), "send after receiver drop must Err");
    }

    // Documents the exact bug the unbounded channel fixes: a bounded
    // `blocking_send` panics ("block within a runtime") when called inside a
    // tokio context — which is what an llm/stream SSE handler did.
    #[test]
    #[should_panic]
    fn bounded_blocking_send_panics_inside_runtime() {
        let (tx, _rx) = tokio::sync::mpsc::channel::<String>(4);
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let _ = tx.blocking_send("x".to_string());
        });
    }

    #[test]
    fn test_match_exact_path() {
        let params = match_path("/users", "/users");
        assert!(params.is_some());
        assert!(params.unwrap().is_empty());
    }

    #[test]
    fn parse_port_rejects_out_of_range() {
        // `p as u16` silently wrapped: 70000 -> 4464, -1 -> 65535. Must error now.
        assert!(parse_port(70000).is_err());
        assert!(parse_port(-1).is_err());
        assert_eq!(
            parse_port(0).unwrap(),
            0,
            "port 0 requests an OS-assigned ephemeral listener"
        );
        assert_eq!(parse_port(3000).unwrap(), 3000);
        assert_eq!(parse_port(65535).unwrap(), 65535);
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
        assert_eq!(params[0], ("id".to_string(), "42".to_string()));
    }

    #[test]
    fn test_match_multiple_params() {
        let params = match_path("/users/:uid/posts/:pid", "/users/1/posts/99").unwrap();
        assert_eq!(params.len(), 2);
        assert_eq!(params[0], ("uid".to_string(), "1".to_string()));
        assert_eq!(params[1], ("pid".to_string(), "99".to_string()));
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
        assert_eq!(params[0], ("*".to_string(), "a/b/c".to_string()));
    }

    #[test]
    fn test_match_trailing_slash_normalized() {
        assert!(match_path("/users", "/users/").is_some());
    }
}
