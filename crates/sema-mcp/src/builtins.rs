//! Sema-facing MCP *client* builtins: connect to an external MCP server and
//! consume its tools from Sema code.
//!
//! Two layers, matching `docs/plans/2026-06-21-mcp-client-spike.md`:
//!
//! - **Layer 1 (protocol primitive):** `mcp/connect`, `mcp/tools`, `mcp/call`,
//!   `mcp/close` — a transport + RPC client, agent-agnostic, like `http/*`.
//! - **Layer 2 (agent adapter):** `mcp/tools->sema` turns an MCP server's tools
//!   into the exact value shape `deftool` produces, so `defagent` consumes them
//!   with zero agent-loop changes.
//!
//! `mcp/connect` spawns a child process, so it is gated on the `PROCESS`
//! capability — a sandbox that denies process spawning cannot open MCP
//! connections, and the other builtins only ever act on a handle that connect
//! already vetted. MCP tools then run with the *server's* authority, not Sema's
//! sandbox: connecting to an untrusted server is like running untrusted code.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::future::Future;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use sema_core::{check_arity, Caps, Env, NativeFn, Sandbox, SemaError, ToolDefinition, Value};
use tokio::runtime::Runtime;

use crate::client::{McpClient, McpClientConfig, McpHttpConfig};

thread_local! {
    // Keep MCP connections in a thread-local map so each Sema evaluator can own its own
    // client state without introducing cross-thread sharing.
    static CONNECTIONS: RefCell<HashMap<String, Rc<RefCell<McpConnection>>>> =
        RefCell::new(HashMap::new());
    // Reuse one current-thread Tokio runtime for all MCP builtins in this evaluator context.
    static TOKIO_RT: RefCell<Option<Runtime>> = const { RefCell::new(None) };
    // The evaluator's sandbox, captured at registration, so the OAuth browser
    // launch (connect-time or mid-session re-auth) can be denied when `PROCESS`
    // is — opening the system browser spawns a process.
    static SANDBOX: RefCell<Sandbox> = RefCell::new(Sandbox::allow_all());
}

/// A browser opener that refuses to launch (spawn) a browser when the sandbox
/// denies `PROCESS`. Only invoked when a browser is actually needed (a full
/// login), so cached/refresh flows are unaffected.
fn gated_browser_opener() -> crate::oauth::loopback::BrowserOpener {
    Box::new(|url: &str| {
        if let Some(err) = SANDBOX.with(|s| {
            let sb = s.borrow();
            if sb.is_unrestricted() {
                None
            } else {
                sb.check(Caps::PROCESS, "mcp/connect (open browser)").err()
            }
        }) {
            return Err(err.to_string());
        }
        crate::oauth::loopback::open_browser(url)
    })
}

static HANDLE_COUNTER: AtomicU64 = AtomicU64::new(1);

struct McpConnection {
    client: McpClient,
    /// Stable server identity (url, or `stdio\0command args`) used to key the
    /// cassette so tool calls record/replay deterministically across runs.
    identity: String,
}

fn block_on<F: Future>(future: F) -> F::Output {
    TOKIO_RT.with(|runtime| {
        let mut runtime = runtime.borrow_mut();
        if runtime.is_none() {
            *runtime = Some(
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("failed to create Tokio runtime for MCP builtin"),
            );
        }
        runtime
            .as_mut()
            .expect("initialized Tokio runtime for MCP builtin")
            .block_on(future)
    })
}

fn next_handle() -> String {
    format!("mcp-{}", HANDLE_COUNTER.fetch_add(1, Ordering::SeqCst))
}

fn register_fn(env: &Env, name: &str, f: impl Fn(&[Value]) -> Result<Value, SemaError> + 'static) {
    env.set_str(name, Value::native_fn(NativeFn::simple(name, f)));
}

/// Refuse a `mcp/connect` unless `cap` is granted. Unrestricted sandboxes pass.
fn gate(sandbox: &Sandbox, cap: Caps) -> Result<(), SemaError> {
    if !sandbox.is_unrestricted() {
        sandbox.check(cap, "mcp/connect")?;
    }
    Ok(())
}

/// Register a live connection under a fresh opaque handle and return the handle.
fn register_connection(client: McpClient, identity: String) -> Result<Value, SemaError> {
    let handle = next_handle();
    let connection = Rc::new(RefCell::new(McpConnection { client, identity }));
    CONNECTIONS.with(|connections| {
        connections.borrow_mut().insert(handle.clone(), connection);
    });
    Ok(Value::string(&handle))
}

/// Connect to a stdio MCP server (`:command` + optional `:args`/`:env`/`:cwd`),
/// run the handshake, and register the connection.
fn connect_stdio(config_json: &serde_json::Value) -> Result<Value, SemaError> {
    let command = config_json
        .get("command")
        .and_then(|value| value.as_str())
        .ok_or_else(|| {
            SemaError::eval("mcp/connect requires a :command (stdio) or :url (http) entry")
                .with_hint(
                    "stdio: {:command \"python3\" :args [\"-c\" \"script\"]}; \
                     http: {:url \"https://…/mcp\"}",
                )
        })?;
    // Every `:args` element must be a string — silently dropping a non-string
    // would launch the server with a different command line than the user wrote.
    let args_vec: Vec<String> = match config_json.get("args") {
        None => Vec::new(),
        Some(serde_json::Value::Array(values)) => {
            let mut out = Vec::with_capacity(values.len());
            for value in values {
                let s = value.as_str().ok_or_else(|| {
                    SemaError::eval("mcp/connect: every :args element must be a string")
                })?;
                out.push(s.to_string());
            }
            out
        }
        Some(_) => {
            return Err(SemaError::eval(
                "mcp/connect: :args must be a list of strings",
            ));
        }
    };
    let env_map = config_json
        .get("env")
        .and_then(|value| value.as_object())
        .map(|object| {
            object
                .iter()
                .filter_map(|(key, value)| value.as_str().map(|s| (key.to_string(), s.to_string())))
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default();
    let cwd = config_json
        .get("cwd")
        .and_then(|value| value.as_str())
        .map(std::path::PathBuf::from);

    // Unambiguous identity for the cassette key: args as a JSON array (so
    // ["a b"] and ["a","b"] don't collide) plus cwd. `env` is deliberately
    // excluded — a rotated token there should not invalidate a recorded tape.
    let identity = serde_json::json!({
        "t": "stdio",
        "command": command,
        "args": args_vec,
        "cwd": cwd.as_ref().map(|p| p.display().to_string()),
    })
    .to_string();
    let mut client = block_on(McpClient::connect(McpClientConfig {
        command: command.to_string(),
        args: args_vec,
        env: (!env_map.is_empty()).then_some(env_map),
        cwd,
    }))
    .map_err(|err| SemaError::eval(format!("mcp/connect: {err}")))?;
    if let Err(err) = block_on(client.initialize()) {
        let _ = block_on(client.close());
        return Err(SemaError::eval(format!("mcp/connect: {err}")));
    }
    register_connection(client, identity)
}

/// Connect to a remote Streamable-HTTP MCP server (`:url` + optional
/// `:headers`), run the handshake, and register the connection.
fn connect_http(config_json: &serde_json::Value) -> Result<Value, SemaError> {
    let url = config_json
        .get("url")
        .and_then(|value| value.as_str())
        .ok_or_else(|| SemaError::eval("mcp/connect requires a :url entry for http transport"))?;
    let mut headers = config_json
        .get("headers")
        .and_then(|value| value.as_object())
        .map(|object| {
            object
                .iter()
                .filter_map(|(key, value)| value.as_str().map(|s| (key.to_string(), s.to_string())))
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default();

    // A user-configured pre-registered client id: `:auth {:client-id "…"}`.
    let preconfigured_client_id = config_json
        .get("auth")
        .and_then(|auth| auth.get("client-id"))
        .and_then(|value| value.as_str())
        .map(str::to_string);

    let mut client = block_on(McpClient::connect_http(McpHttpConfig {
        url: url.to_string(),
        headers: headers.clone(),
    }))
    .map_err(|err| SemaError::eval(format!("mcp/connect: {err}")))?;

    if let Err(err) = block_on(client.initialize()) {
        if let Some(challenge) = client.http_challenge() {
            // A `401` means the server requires OAuth; run the login flow, attach
            // the token, and retry. Some authenticated servers (e.g. Asana) gate
            // *all* requests behind auth and only reveal that they actually speak
            // the legacy HTTP+SSE transport once authorized — so a `404`/`405` on
            // the authenticated Streamable POST means "retry over legacy SSE with
            // the token attached", not a hard failure.
            let token = obtain_access_token(url, &challenge, preconfigured_client_id.as_deref())?;
            headers.insert("Authorization".to_string(), format!("Bearer {token}"));
            client.set_bearer_token(&token);
            if let Err(err2) = block_on(client.initialize()) {
                let status = client.http_last_status();
                let _ = block_on(client.close());
                if matches!(status, Some(404) | Some(405)) {
                    return connect_legacy(url, headers);
                }
                return Err(SemaError::eval(format!(
                    "mcp/connect: handshake failed after authorization: {err2}"
                )));
            }
        } else if matches!(client.http_last_status(), Some(404) | Some(405)) {
            // Unauthenticated server that only speaks the deprecated 2024-11-05
            // HTTP+SSE transport (POST→4xx→GET-`endpoint`).
            let _ = block_on(client.close());
            return connect_legacy(url, headers);
        } else {
            let _ = block_on(client.close());
            return Err(SemaError::eval(format!("mcp/connect: {err}")));
        }
    }
    register_connection(client, url.to_string())
}

/// Connect over the deprecated 2024-11-05 HTTP+SSE transport. `headers` carries
/// any bearer token obtained during the OAuth flow, so authenticated legacy
/// servers (e.g. Asana) work.
fn connect_legacy(url: &str, headers: HashMap<String, String>) -> Result<Value, SemaError> {
    let mut legacy = block_on(McpClient::connect_legacy_sse(McpHttpConfig {
        url: url.to_string(),
        headers,
    }))
    .map_err(|e| SemaError::eval(format!("mcp/connect: {e}")))?;
    block_on(legacy.initialize())
        .map_err(|e| SemaError::eval(format!("mcp/connect (legacy SSE): {e}")))?;
    register_connection(legacy, url.to_string())
}

/// Run (or reuse) the OAuth login for a remote server that answered `401`, and
/// return an access token. Uses the default credential store (keychain or file)
/// and a real loopback + system-browser flow.
fn obtain_access_token(
    url: &str,
    challenge_header: &str,
    preconfigured_client_id: Option<&str>,
) -> Result<String, SemaError> {
    use crate::oauth::{discovery, login, loopback, store};

    let challenge = discovery::parse_www_authenticate(challenge_header);
    let http = reqwest::Client::new();
    let credential_store = store::default_store();
    let driver = loopback::LoopbackDriver::with_opener(
        std::time::Duration::from_secs(300),
        gated_browser_opener(),
    )
    .map_err(|e| SemaError::eval(format!("mcp/connect: {e}")))?;

    let config = login::LoginConfig {
        mcp_url: url,
        resource_metadata_url: challenge.resource_metadata.as_deref(),
        requested_scope: challenge.scope.as_deref(),
        preconfigured_client_id,
    };

    block_on(login::ensure_access_token(
        &http,
        credential_store.as_ref(),
        &config,
        &driver,
    ))
    .map_err(|e| {
        SemaError::eval(format!("mcp/connect: OAuth login failed: {e}")).with_hint(
            "a browser should have opened to complete login; or pass a token via \
             :headers {\"Authorization\" \"Bearer …\"}",
        )
    })
}

fn config_to_json(args: &[Value]) -> Result<serde_json::Value, SemaError> {
    check_arity!(args, "mcp/connect", 1);
    let config = &args[0];
    let map = config.as_map_ref().ok_or_else(|| {
        SemaError::type_error("map", config.type_name())
            .with_hint("mcp/connect expects a single config map; use {:command ...}")
    })?;
    let mut config_json = serde_json::Map::new();
    for (key, value) in map.iter() {
        let key_str = key
            .as_keyword()
            .or_else(|| key.as_str().map(|s| s.to_string()))
            .unwrap_or_else(|| key.to_string());
        config_json.insert(key_str, sema_core::value_to_json_lossy(value));
    }
    Ok(serde_json::Value::Object(config_json))
}

fn require_handle<'a>(args: &'a [Value], fn_name: &str) -> Result<&'a str, SemaError> {
    args[0].as_str().ok_or_else(|| {
        SemaError::type_error("string", args[0].type_name()).with_hint(format!(
            "{fn_name} expects the opaque handle returned by mcp/connect"
        ))
    })
}

fn lookup_connection(handle: &str) -> Result<Rc<RefCell<McpConnection>>, SemaError> {
    CONNECTIONS.with(|connections| {
        connections.borrow().get(handle).cloned().ok_or_else(|| {
            SemaError::eval(format!(
                "mcp connection {handle} is not registered; it may have been closed"
            ))
        })
    })
}

/// Cassette key for one MCP `tools/call`: a hash of the server identity + tool +
/// (canonical) arguments. Stable across runs so record/replay correlate.
fn cassette_key(identity: &str, tool: &str, args: &serde_json::Value) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(b"mcp-call\0");
    hasher.update(identity.as_bytes());
    hasher.update(b"\0");
    hasher.update(tool.as_bytes());
    hasher.update(b"\0");
    // serde_json serializes object keys sorted (BTreeMap) → deterministic.
    hasher.update(serde_json::to_string(args).unwrap_or_default().as_bytes());
    format!("mcp-{:x}", hasher.finalize())
}

/// Invoke a tool, routing through the cassette when one is active: a replay hit
/// returns the recorded result without touching the network; otherwise the real
/// call runs and its result is recorded.
fn call_tool_via_connection(
    handle: &str,
    tool_name: &str,
    arguments_json: serde_json::Value,
) -> Result<serde_json::Value, SemaError> {
    let connection = lookup_connection(handle)?;
    let key = cassette_key(&connection.borrow().identity, tool_name, &arguments_json);

    match sema_core::mcp_cassette_decide(&key) {
        Some(sema_core::McpCassetteDecision::Replay(recorded)) => return Ok(recorded),
        Some(sema_core::McpCassetteDecision::Miss) => {
            return Err(SemaError::eval(
                "mcp/call: no cassette recording for this call (replay miss)".to_string(),
            )
            .with_hint(
                "re-record the tape with SEMA_LLM_CASSETTE_MODE=record, or the call arguments \
                 drifted from what was recorded",
            ));
        }
        // Record mode or no cassette → perform the real call, then record it.
        _ => {}
    }

    let result = call_tool_real(&connection, tool_name, arguments_json)?;
    sema_core::mcp_cassette_record(&key, &result);
    Ok(result)
}

/// The real (network) tool call, with one mid-session re-auth retry.
fn call_tool_real(
    connection: &Rc<RefCell<McpConnection>>,
    tool_name: &str,
    arguments_json: serde_json::Value,
) -> Result<serde_json::Value, SemaError> {
    let first = {
        let mut conn = connection.borrow_mut();
        block_on(conn.client.call_tool(tool_name, arguments_json.clone()))
    };
    let err = match first {
        Ok(value) => return Ok(value),
        Err(err) => err,
    };

    // A mid-session `401` (token expired) or `403 insufficient_scope` (needs
    // step-up) on a remote HTTP server means "re-authorize and retry once".
    let (status, challenge, url) = {
        let conn = connection.borrow();
        (
            conn.client.http_last_status(),
            conn.client.http_challenge(),
            conn.client.http_url(),
        )
    };
    if !matches!(status, Some(401) | Some(403)) {
        return Err(SemaError::eval(format!("mcp/call: {err}")));
    }
    let Some(url) = url else {
        return Err(SemaError::eval(format!("mcp/call: {err}")));
    };
    let token = match reauthorize(&url, status, challenge.as_deref()) {
        Ok(Some(token)) => token,
        // Not an auth challenge we handle, or re-auth failed — surface the
        // original error.
        _ => return Err(SemaError::eval(format!("mcp/call: {err}"))),
    };

    let mut conn = connection.borrow_mut();
    // Streamable HTTP swaps the header; legacy SSE reconnects its stream.
    block_on(conn.client.reauthorize_bearer(&token))
        .map_err(|e| SemaError::eval(format!("mcp/call: {e}")))?;
    block_on(conn.client.call_tool(tool_name, arguments_json))
        .map_err(|err| SemaError::eval(format!("mcp/call: {err}")))
}

/// React to a mid-session auth challenge (refresh on `401`, step-up re-scope on
/// `403 insufficient_scope`) and return a fresh access token to retry with.
fn reauthorize(
    url: &str,
    status: Option<u16>,
    challenge: Option<&str>,
) -> Result<Option<String>, SemaError> {
    let http = reqwest::Client::new();
    let store = crate::oauth::store::default_store();
    let driver = crate::oauth::loopback::LoopbackDriver::with_opener(
        std::time::Duration::from_secs(300),
        gated_browser_opener(),
    )
    .map_err(|e| SemaError::eval(format!("mcp/call: {e}")))?;
    block_on(crate::oauth::login::reauth_on_challenge(
        &http,
        store.as_ref(),
        url,
        status,
        challenge,
        None,
        &driver,
    ))
    .map_err(|e| SemaError::eval(format!("mcp/call: re-authorization failed: {e}")))
}

/// Concatenate the `text` blocks of a `tools/call` result, if any.
fn result_text(result: &serde_json::Value) -> Option<String> {
    let content = result.get("content")?.as_array()?;
    let parts: Vec<String> = content
        .iter()
        .filter(|item| item.get("type").and_then(|t| t.as_str()) == Some("text"))
        .filter_map(|item| {
            item.get("text")
                .and_then(|t| t.as_str())
                .map(str::to_string)
        })
        .collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n"))
    }
}

/// Normalize a `tools/call` result into a Sema value: plain text collapses to a
/// string (what an agent wants to feed back to the model), anything richer
/// (images, resources, `structuredContent`) is handed over as the full map.
fn result_to_value(result: &serde_json::Value) -> Value {
    match result_text(result) {
        Some(text) => Value::string(&text),
        None => sema_core::json_to_value(result),
    }
}

fn result_is_error(result: &serde_json::Value) -> bool {
    result
        .get("isError")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// Invert an MCP `inputSchema` (JSON Schema) into the `{param-name -> spec}` map
/// that `deftool` produces, so the agent loop's schema handling treats an MCP
/// tool exactly like a local one. Returns the params map plus the parameter
/// names in the map's own key order — the order the agent loop passes them to a
/// native handler, which the handler needs to rebuild the arguments object.
fn schema_to_params(schema: &serde_json::Value) -> (Value, Vec<String>) {
    let required: HashSet<&str> = schema
        .get("required")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();

    let mut params: BTreeMap<Value, Value> = BTreeMap::new();
    if let Some(props) = schema.get("properties").and_then(|v| v.as_object()) {
        for (name, spec) in props {
            let mut entry: BTreeMap<Value, Value> = BTreeMap::new();
            let ty = spec
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("string");
            entry.insert(Value::keyword("type"), Value::keyword(ty));
            if let Some(desc) = spec.get("description").and_then(|v| v.as_str()) {
                entry.insert(Value::keyword("description"), Value::string(desc));
            }
            if let Some(enum_vals) = spec.get("enum").and_then(|v| v.as_array()) {
                let items: Vec<Value> = enum_vals.iter().map(sema_core::json_to_value).collect();
                entry.insert(Value::keyword("enum"), Value::list(items));
            }
            // deftool marks a param optional with `:optional #t`; anything not in
            // the schema's `required` list is optional.
            if !required.contains(name.as_str()) {
                entry.insert(Value::keyword("optional"), Value::bool(true));
            }
            params.insert(Value::keyword(name), Value::map(entry));
        }
    }

    let ordered: Vec<String> = params.keys().filter_map(|k| k.as_keyword()).collect();
    (Value::map(params), ordered)
}

pub fn register_mcp_builtins(env: &Env, sandbox: &Sandbox) {
    // Capture the sandbox so the OAuth browser launch can honor the PROCESS cap.
    SANDBOX.with(|s| *s.borrow_mut() = sandbox.clone());

    // `mcp/connect` picks its transport from the config map at runtime, so the
    // capability it needs is not fixed: a `:url` server is network I/O
    // (`NETWORK`), a `:command` server spawns a process (`PROCESS`). Gate inside
    // the handler rather than via a single fixed-capability wrapper.
    {
        let sandbox = sandbox.clone();
        register_fn(env, "mcp/connect", move |args| {
            let config_json = config_to_json(args)?;
            if config_json.get("url").and_then(|v| v.as_str()).is_some() {
                gate(&sandbox, Caps::NETWORK)?;
                connect_http(&config_json)
            } else {
                gate(&sandbox, Caps::PROCESS)?;
                connect_stdio(&config_json)
            }
        });
    }

    register_fn(env, "mcp/tools", |args| {
        check_arity!(args, "mcp/tools", 1);
        let handle = require_handle(args, "mcp/tools")?;
        let connection = lookup_connection(handle)?;
        let mut connection = connection.borrow_mut();
        let tools = block_on(connection.client.list_tools())
            .map_err(|err| SemaError::eval(format!("mcp/tools: {err}")))?;
        let items = tools
            .into_iter()
            .map(|tool| {
                let mut entry = BTreeMap::new();
                entry.insert(Value::keyword("name"), Value::string(&tool.name));
                entry.insert(
                    Value::keyword("description"),
                    Value::string(&tool.description),
                );
                entry.insert(
                    Value::keyword("input-schema"),
                    sema_core::json_to_value(&tool.input_schema),
                );
                Value::map(entry)
            })
            .collect();
        Ok(Value::list(items))
    });

    register_fn(env, "mcp/tools->sema", |args| {
        check_arity!(args, "mcp/tools->sema", 1);
        let handle = require_handle(args, "mcp/tools->sema")?;
        let connection = lookup_connection(handle)?;
        let mut connection = connection.borrow_mut();
        let tools = block_on(connection.client.list_tools())
            .map_err(|err| SemaError::eval(format!("mcp/tools->sema: {err}")))?;

        let mut items = Vec::new();
        for tool in tools {
            let (parameters, ordered) = schema_to_params(&tool.input_schema);
            let tool_name = tool.name.clone();
            let connection_handle = handle.to_string();
            let handler_name = format!("mcp/{tool_name}");
            let handler = Value::native_fn(NativeFn::simple(&handler_name, move |args| {
                // The agent loop passes arguments positionally in `ordered` order
                // (see `json_args_to_sema`); rebuild the named arguments object,
                // dropping the ones the model left unset (nil).
                let mut arguments = serde_json::Map::new();
                for (name, value) in ordered.iter().zip(args.iter()) {
                    if value.is_nil() {
                        continue;
                    }
                    arguments.insert(name.clone(), sema_core::value_to_json_lossy(value));
                }
                let result = call_tool_via_connection(
                    &connection_handle,
                    &tool_name,
                    serde_json::Value::Object(arguments),
                )?;
                // Surface a tool-reported failure as an error so the agent loop
                // feeds it back to the model instead of treating it as success.
                if result_is_error(&result) {
                    let detail = result_text(&result).unwrap_or_else(|| result.to_string());
                    return Err(SemaError::eval(format!(
                        "mcp tool `{tool_name}` returned an error: {detail}"
                    )));
                }
                Ok(result_to_value(&result))
            }));
            items.push(Value::tool_def(ToolDefinition {
                name: tool.name,
                description: tool.description,
                parameters,
                handler,
            }));
        }
        Ok(Value::list(items))
    });

    register_fn(env, "mcp/call", |args| {
        check_arity!(args, "mcp/call", 3);
        let handle = require_handle(args, "mcp/call")?;
        let tool_name = args[1].as_str().ok_or_else(|| {
            SemaError::type_error("string", args[1].type_name())
                .with_hint("mcp/call expects the tool name as a string")
        })?;
        let arguments_json = sema_core::value_to_json_lossy(&args[2]);
        let result = call_tool_via_connection(handle, tool_name, arguments_json)?;
        Ok(result_to_value(&result))
    });

    register_fn(env, "mcp/close", |args| {
        check_arity!(args, "mcp/close", 1);
        let handle = require_handle(args, "mcp/close")?;
        let connection = CONNECTIONS.with(|connections| connections.borrow_mut().remove(handle));
        let Some(connection) = connection else {
            return Err(SemaError::eval(format!(
                "mcp connection {handle} is not registered; it may have already been closed"
            )));
        };
        let mut connection = connection.borrow_mut();
        block_on(connection.client.close())
            .map_err(|err| SemaError::eval(format!("mcp/close: {err}")))?;
        Ok(Value::nil())
    });
}
