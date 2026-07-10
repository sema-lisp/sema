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
    /// From [`ConnectOpts::interactive_auth`]. `mcp/connect` always connects
    /// with `true`; a connection made via [`connect_from_config`] with
    /// `interactive_auth: false` stays non-interactive for its whole
    /// lifetime — a mid-session 401/403 in `reauthorize` may still refresh
    /// silently, but never falls back to a browser login (see
    /// [`NoInteractiveDriver`]).
    interactive_auth: bool,
    /// From [`ConnectOpts::allowed_tools`]. `None` is unrestricted (today's
    /// `mcp/connect` behavior); `Some(list)` restricts `mcp/call` and filters
    /// `mcp/tools`/`mcp/tools->sema` to exactly these tool names.
    allowed_tools: Option<Vec<String>>,
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
fn register_connection(
    client: McpClient,
    identity: String,
    opts: &ConnectOpts,
) -> Result<Value, SemaError> {
    let handle = next_handle();
    let connection = Rc::new(RefCell::new(McpConnection {
        client,
        identity,
        interactive_auth: opts.interactive_auth,
        allowed_tools: opts.allowed_tools.clone(),
    }));
    CONNECTIONS.with(|connections| {
        connections.borrow_mut().insert(handle.clone(), connection);
    });
    Ok(Value::string(&handle))
}

/// Connect to a stdio MCP server (`:command` + optional `:args`/`:env`/`:cwd`),
/// run the handshake, and register the connection. Stdio servers never speak
/// OAuth, so `opts.interactive_auth` is only carried through for `mcp/call`'s
/// bookkeeping (it is meaningless here); `opts.allowed_tools` applies as usual.
fn connect_stdio(config_json: &serde_json::Value, opts: &ConnectOpts) -> Result<Value, SemaError> {
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
    register_connection(client, identity, opts)
}

/// Connect to a remote Streamable-HTTP MCP server (`:url` + optional
/// `:headers`), run the handshake, and register the connection.
///
/// When `opts.interactive_auth` is `false`, a `401`/`403` OAuth challenge is
/// never chased: [`obtain_access_token`] (and therefore the browser opener and
/// loopback listener it drives) is never called, and the connect fails with
/// [`ConnectOutcome::NeedsAuth`] instead. A caller that wants a silent
/// reconnect from a cached token should inject it directly via
/// `:headers {"Authorization" "Bearer …"}` before calling — that value already
/// flows straight into `McpHttpConfig.headers` below, so no new surface is
/// needed for token injection.
fn connect_http(
    config_json: &serde_json::Value,
    opts: &ConnectOpts,
) -> Result<Value, ConnectOutcome> {
    let url = config_json
        .get("url")
        .and_then(|value| value.as_str())
        .ok_or_else(|| SemaError::eval("mcp/connect requires a :url entry for http transport"))
        .map_err(ConnectOutcome::Sema)?;
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
    .map_err(|err| ConnectOutcome::Sema(SemaError::eval(format!("mcp/connect: {err}"))))?;

    if let Err(err) = block_on(client.initialize()) {
        if let Some(challenge) = client.http_challenge() {
            if !opts.interactive_auth {
                // Non-interactive: report the auth requirement instead of
                // chasing it — no discovery, no browser, no loopback bind.
                let _ = block_on(client.close());
                return Err(ConnectOutcome::NeedsAuth(url.to_string()));
            }
            // A `401` means the server requires OAuth; run the login flow, attach
            // the token, and retry. Some authenticated servers (e.g. Asana) gate
            // *all* requests behind auth and only reveal that they actually speak
            // the legacy HTTP+SSE transport once authorized — so a `404`/`405` on
            // the authenticated Streamable POST means "retry over legacy SSE with
            // the token attached", not a hard failure.
            let token = obtain_access_token(url, &challenge, preconfigured_client_id.as_deref())
                .map_err(ConnectOutcome::Sema)?;
            headers.insert("Authorization".to_string(), format!("Bearer {token}"));
            client.set_bearer_token(&token);
            if let Err(err2) = block_on(client.initialize()) {
                let status = client.http_last_status();
                let _ = block_on(client.close());
                if matches!(status, Some(404) | Some(405)) {
                    return connect_legacy(url, headers, opts);
                }
                return Err(ConnectOutcome::Sema(SemaError::eval(format!(
                    "mcp/connect: handshake failed after authorization: {err2}"
                ))));
            }
        } else if matches!(client.http_last_status(), Some(404) | Some(405)) {
            // Unauthenticated server that only speaks the deprecated 2024-11-05
            // HTTP+SSE transport (POST→4xx→GET-`endpoint`).
            let _ = block_on(client.close());
            return connect_legacy(url, headers, opts);
        } else {
            let _ = block_on(client.close());
            return Err(ConnectOutcome::Sema(SemaError::eval(format!(
                "mcp/connect: {err}"
            ))));
        }
    }
    register_connection(client, url.to_string(), opts).map_err(ConnectOutcome::Sema)
}

/// Connect over the deprecated 2024-11-05 HTTP+SSE transport. `headers` carries
/// any bearer token obtained during the OAuth flow, so authenticated legacy
/// servers (e.g. Asana) work.
fn connect_legacy(
    url: &str,
    headers: HashMap<String, String>,
    opts: &ConnectOpts,
) -> Result<Value, ConnectOutcome> {
    let mut legacy = block_on(McpClient::connect_legacy_sse(McpHttpConfig {
        url: url.to_string(),
        headers,
    }))
    .map_err(|e| ConnectOutcome::Sema(SemaError::eval(format!("mcp/connect: {e}"))))?;
    block_on(legacy.initialize()).map_err(|e| {
        ConnectOutcome::Sema(SemaError::eval(format!("mcp/connect (legacy SSE): {e}")))
    })?;
    register_connection(legacy, url.to_string(), opts).map_err(ConnectOutcome::Sema)
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

/// Convert a Sema config map — `mcp/connect`'s single argument, or a config
/// value a caller of [`connect_from_config`] builds programmatically — into the
/// JSON shape the transport dispatch and connect helpers below expect.
fn value_to_config_json(config: &Value) -> Result<serde_json::Value, SemaError> {
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

fn config_to_json(args: &[Value]) -> Result<serde_json::Value, SemaError> {
    check_arity!(args, "mcp/connect", 1);
    value_to_config_json(&args[0])
}

// ── the public non-interactive / least-privilege connect entry point ──────

/// Options for [`connect_from_config`]. `mcp/connect` (the interactive Sema
/// builtin) always connects with `ConnectOpts { interactive_auth: true,
/// allowed_tools: None }` — today's unrestricted, browser-capable behavior.
#[derive(Debug, Clone, Default)]
pub struct ConnectOpts {
    /// When `false`, a `401`/`403` OAuth challenge encountered while
    /// connecting NEVER launches an interactive flow — no system browser, no
    /// loopback listener bound. The connect fails with
    /// [`ConnectFailure::NeedsAuth`] instead, naming the server URL that
    /// challenged. A caller that already holds a cached/valid token should
    /// inject it directly via `:headers {"Authorization" "Bearer …"}` on the
    /// config map before calling — that flows straight into
    /// `McpHttpConfig.headers`, so no separate injection surface exists or is
    /// needed. A connection made with `false` also stays non-interactive for
    /// its whole lifetime: a mid-session 401/403 during `mcp/call` may still
    /// self-heal via a stored refresh token, but never falls back to a
    /// browser login (see the crate-internal `NoInteractiveDriver`).
    pub interactive_auth: bool,
    /// When `Some(list)`, the resulting connection is restricted to exactly
    /// these tool names: `mcp/call` of any other tool name fails before any
    /// cassette lookup or network call, and `mcp/tools`/`mcp/tools->sema` only
    /// ever surface the allowed subset — an agent given this connection's
    /// tools never even sees an undeclared one. `Some(vec![])` is a valid
    /// degenerate case (no tools callable). `None` is unrestricted — today's
    /// `mcp/connect` behavior.
    pub allowed_tools: Option<Vec<String>>,
}

/// Why [`connect_from_config`] failed to establish a connection.
#[derive(Debug)]
pub enum ConnectFailure {
    /// The server demands OAuth consent and `opts.interactive_auth` was
    /// `false`, so no browser/loopback flow was attempted. `url` is the MCP
    /// server endpoint that issued the challenge — typically used to gate a
    /// workflow run and prompt the user to authenticate out-of-band (e.g. via
    /// a dashboard or `sema mcp login`), then retry the connect once a token
    /// has been persisted.
    NeedsAuth { url: String },
    /// Any other connect failure: bad config, network error, process spawn
    /// failure, sandbox denial, handshake failure, and — on the interactive
    /// path — a failed OAuth login. The message is meant for end users; any
    /// structured hint the underlying error carried is folded into it.
    Failed(String),
}

/// Outcome of the shared connect helpers, before it is narrowed for a
/// particular caller: the native `mcp/connect` builtin (always
/// `interactive_auth: true`) unwraps this straight back into the original
/// `SemaError` — hint and note intact, byte-identical to before this
/// function existed — while [`connect_from_config`] collapses it into the
/// simpler public [`ConnectFailure`].
enum ConnectOutcome {
    NeedsAuth(String),
    Sema(SemaError),
}

/// Gate on the sandbox captured by the most recent [`register_mcp_builtins`]
/// call on this thread (unrestricted if that was never called), dispatch on
/// transport, and connect. Shared by `mcp/connect` and [`connect_from_config`]
/// so the two can never drift.
fn connect_with_opts(
    config_json: &serde_json::Value,
    opts: &ConnectOpts,
) -> Result<Value, ConnectOutcome> {
    let sandbox = SANDBOX.with(|s| s.borrow().clone());
    if config_json.get("url").and_then(|v| v.as_str()).is_some() {
        gate(&sandbox, Caps::NETWORK).map_err(ConnectOutcome::Sema)?;
        connect_http(config_json, opts)
    } else {
        gate(&sandbox, Caps::PROCESS).map_err(ConnectOutcome::Sema)?;
        connect_stdio(config_json, opts).map_err(ConnectOutcome::Sema)
    }
}

/// Connect to an MCP server from a config map (the same shape `mcp/connect`
/// accepts: `{:url ...}` for Streamable HTTP / legacy SSE, `{:command ...}`
/// for stdio), honoring [`ConnectOpts`]. This is `mcp/connect`'s underlying
/// implementation, exposed as a Rust entry point for callers that need
/// options the Sema builtin doesn't take — namely a workflow runtime that
/// must connect declared MCP servers headlessly (never popping a browser
/// mid-run) and enforce a least-privilege `:tools` manifest.
///
/// Returns the same opaque handle `Value` `mcp/connect` does, registered in
/// the same thread-local connection table — so a handle obtained here works
/// with `mcp/call`/`mcp/tools`/`mcp/tools->sema`/`mcp/close` exactly as if it
/// came from `(mcp/connect ...)`, as long as those builtins are evaluated on
/// the same thread. Sandbox gates (`Caps::NETWORK` for `:url`, `Caps::PROCESS`
/// for `:command`) apply exactly as they do for `mcp/connect`.
pub fn connect_from_config(config: &Value, opts: ConnectOpts) -> Result<Value, ConnectFailure> {
    let outcome = value_to_config_json(config)
        .map_err(ConnectOutcome::Sema)
        .and_then(|config_json| connect_with_opts(&config_json, &opts));
    outcome.map_err(connect_outcome_to_failure)
}

/// Collapse a [`ConnectOutcome`] into the public [`ConnectFailure`]. Any hint
/// the original `SemaError` carried is appended to the message — `Display`
/// alone would drop it (hints are a separate structured field), and
/// `ConnectFailure::Failed` is intentionally a plain string.
fn connect_outcome_to_failure(outcome: ConnectOutcome) -> ConnectFailure {
    match outcome {
        ConnectOutcome::NeedsAuth(url) => ConnectFailure::NeedsAuth { url },
        ConnectOutcome::Sema(err) => {
            let mut message = err.to_string();
            if let Some(hint) = err.hint() {
                message.push_str(&format!(" (hint: {hint})"));
            }
            ConnectFailure::Failed(message)
        }
    }
}

/// Unwrap a [`ConnectOutcome`] produced with `interactive_auth: true` (the
/// native `mcp/connect` builtin's own path) back into the original
/// `SemaError`, hint/note intact — this is what keeps `mcp/connect`'s error
/// messages byte-identical across the refactor. `NeedsAuth` cannot occur here
/// (it is only returned when `opts.interactive_auth` is `false`); handled
/// defensively rather than with a panic in case that invariant ever changes.
fn connect_outcome_to_sema_error(outcome: ConnectOutcome) -> SemaError {
    match outcome {
        ConnectOutcome::Sema(err) => err,
        ConnectOutcome::NeedsAuth(url) => SemaError::eval(format!(
            "mcp/connect: server at {url} requires authorization (unexpected: an interactive \
             connect should have attempted login)"
        )),
    }
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

/// True when `tool_name` is callable under a connection's `:tools` manifest:
/// `None` is unrestricted, `Some(list)` allows only the named tools (including
/// the degenerate empty list, which allows none). Shared by the `mcp/call`
/// enforcement below and the `mcp/tools`/`mcp/tools->sema` list filters.
fn tool_is_allowed(allowed_tools: &Option<Vec<String>>, tool_name: &str) -> bool {
    match allowed_tools {
        None => true,
        Some(list) => list.iter().any(|t| t == tool_name),
    }
}

/// Enforce a connection's `:tools` manifest before any cassette lookup or
/// network call: an undeclared tool never reaches the wire, even in cassette
/// record mode.
fn check_tool_allowed(connection: &McpConnection, tool_name: &str) -> Result<(), SemaError> {
    if tool_is_allowed(&connection.allowed_tools, tool_name) {
        return Ok(());
    }
    // `tool_is_allowed` only returns `false` for `Some(list)`, so this is populated.
    let allowed = connection.allowed_tools.as_deref().unwrap_or_default();
    let manifest = if allowed.is_empty() {
        "(none)".to_string()
    } else {
        allowed.join(", ")
    };
    Err(SemaError::eval(format!(
        "mcp/call: tool `{tool_name}` is not declared in this connection's :tools manifest [{manifest}]"
    ))
    .with_hint("declared in the workflow's :mcp :tools manifest; add it there to allow it"))
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
    check_tool_allowed(&connection.borrow(), tool_name)?;
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
    let interactive_auth = connection.borrow().interactive_auth;
    let token = match reauthorize(&url, status, challenge.as_deref(), interactive_auth) {
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

/// A [`RedirectDriver`](crate::oauth::loopback::RedirectDriver) for
/// non-interactive connections (`ConnectOpts::interactive_auth: false`).
/// `reauth_on_challenge`'s refresh-token path never calls `drive()` — a valid
/// stored refresh token self-heals a `401` without any redirect — but its full
/// login fallback (no/expired refresh token, or a `403 insufficient_scope`
/// step-up, which always needs fresh consent) does call it. Failing `drive()`
/// cleanly here, with no changes to `oauth/login.rs`, is what keeps a
/// non-interactive connection from ever popping a browser for the rest of its
/// lifetime.
struct NoInteractiveDriver;

impl crate::oauth::loopback::RedirectDriver for NoInteractiveDriver {
    fn redirect_uri(&self) -> String {
        // `login()` reads this to build the authorize URL / register a DCR
        // client BEFORE calling `drive()`, which always errors below — so this
        // is never actually dialed, but must be a well-formed loopback URI.
        "http://127.0.0.1:1/callback".to_string()
    }

    fn drive(&self, _authorize_url: &str, _expected_state: &str) -> Result<String, String> {
        Err("interactive authentication is disabled for this connection".to_string())
    }
}

/// React to a mid-session auth challenge (refresh on `401`, step-up re-scope on
/// `403 insufficient_scope`) and return a fresh access token to retry with.
/// `interactive_auth` (from the connection's [`ConnectOpts`]) selects the
/// redirect driver: the real loopback+browser flow, or [`NoInteractiveDriver`]
/// so a login fallback fails cleanly instead of popping a browser mid-run.
fn reauthorize(
    url: &str,
    status: Option<u16>,
    challenge: Option<&str>,
    interactive_auth: bool,
) -> Result<Option<String>, SemaError> {
    let http = reqwest::Client::new();
    let store = crate::oauth::store::default_store();
    let result = if interactive_auth {
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
    } else {
        block_on(crate::oauth::login::reauth_on_challenge(
            &http,
            store.as_ref(),
            url,
            status,
            challenge,
            None,
            &NoInteractiveDriver,
        ))
    };
    result.map_err(|e| SemaError::eval(format!("mcp/call: re-authorization failed: {e}")))
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

/// Best-effort close of a connection by its opaque handle `Value`, same
/// semantics as the `mcp/close` builtin but NEVER errors (a missing/already-closed
/// handle, or a close-protocol failure, is silently ignored). For callers outside
/// Sema code that need "close everything, and don't let a close failure mask the
/// real outcome" — namely the workflow `:mcp` auth-resolution seam's
/// `WorkflowMcpResolver::close`, which `workflow/run` calls from its
/// failure/cleanup paths (`docs/plans/2026-06-24-workflow-mcp-auth.md` §3).
pub fn close_handle(handle: &Value) {
    let Some(handle_str) = handle.as_str() else {
        return;
    };
    let connection = CONNECTIONS.with(|connections| connections.borrow_mut().remove(handle_str));
    if let Some(connection) = connection {
        let mut connection = connection.borrow_mut();
        let _ = block_on(connection.client.close());
    }
}

pub fn register_mcp_builtins(env: &Env, sandbox: &Sandbox) {
    // Capture the sandbox so the OAuth browser launch can honor the PROCESS cap.
    SANDBOX.with(|s| *s.borrow_mut() = sandbox.clone());

    // `mcp/connect` picks its transport from the config map at runtime, so the
    // capability it needs is not fixed: a `:url` server is network I/O
    // (`NETWORK`), a `:command` server spawns a process (`PROCESS`). Gating and
    // dispatch live in `connect_with_opts`, shared with `connect_from_config`;
    // this just supplies the interactive, unrestricted options `mcp/connect`
    // has always used and unwraps the outcome back to the original `SemaError`.
    register_fn(env, "mcp/connect", |args| {
        let config_json = config_to_json(args)?;
        let opts = ConnectOpts {
            interactive_auth: true,
            allowed_tools: None,
        };
        connect_with_opts(&config_json, &opts).map_err(connect_outcome_to_sema_error)
    });

    register_fn(env, "mcp/tools", |args| {
        check_arity!(args, "mcp/tools", 1);
        let handle = require_handle(args, "mcp/tools")?;
        let connection = lookup_connection(handle)?;
        let mut connection = connection.borrow_mut();
        let tools = block_on(connection.client.list_tools())
            .map_err(|err| SemaError::eval(format!("mcp/tools: {err}")))?;
        let items = tools
            .into_iter()
            .filter(|tool| tool_is_allowed(&connection.allowed_tools, &tool.name))
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
            if !tool_is_allowed(&connection.allowed_tools, &tool.name) {
                // Not in the connection's :tools manifest — an agent given
                // this connection's tools must never even see it.
                continue;
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oauth::loopback::RedirectDriver;

    #[test]
    fn no_interactive_driver_never_dials_and_fails_cleanly() {
        let driver = NoInteractiveDriver;
        // Well-formed enough for `login()` to build an authorize URL / DCR
        // request with it, even though it is never actually dialed.
        assert!(driver.redirect_uri().starts_with("http://127.0.0.1"));
        let err = driver
            .drive("https://example.com/authorize", "some-state")
            .expect_err("a non-interactive driver must refuse to drive a login");
        assert_eq!(
            err,
            "interactive authentication is disabled for this connection"
        );
    }

    #[test]
    fn tool_is_allowed_none_is_unrestricted() {
        assert!(tool_is_allowed(&None, "anything"));
    }

    #[test]
    fn tool_is_allowed_some_restricts_to_the_list() {
        let allowed = Some(vec!["create_task".to_string(), "search_tasks".to_string()]);
        assert!(tool_is_allowed(&allowed, "create_task"));
        assert!(!tool_is_allowed(&allowed, "delete_everything"));
    }

    #[test]
    fn tool_is_allowed_empty_list_allows_nothing() {
        assert!(!tool_is_allowed(&Some(Vec::new()), "anything"));
    }
}
