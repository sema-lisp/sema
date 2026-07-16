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
//!
//! ## Async offload (MCP-4 / issue #96)
//!
//! Inside an `async/spawn`'d task (`sema_core::in_async_context()`), all four
//! builtins — and therefore every `mcp/tools->sema` handler, which routes
//! through the same [`call_tool`] core — offload their JSON-RPC round trip onto
//! the shared `sema-io` pool and yield `AwaitIo`, so a slow `mcp/call` no
//! longer stalls sibling scheduler tasks. At top level (no scheduler) every
//! builtin keeps today's fully synchronous, blocking behavior byte-for-byte —
//! see [`block_on`].
//!
//! **Registry / checkout.** `CONNECTIONS` maps a handle to an [`Rc<ConnEntry>`],
//! where [`ConnEntry`] separates the connection's stable, always-readable
//! metadata ([`ConnMeta`] — identity, auth mode, tool allowlist) from its
//! checkout state ([`Slot`]). MCP is serial per connection (one JSON-RPC pipe):
//! `Slot::Available` <-> `Slot::CheckedOut` enforces that only one in-flight
//! operation ever holds a connection's transport, while `Slot::Tombstone`
//! is a one-way trapdoor entered when a checked-out connection is abandoned
//! mid-call (cancellation) or after `mcp/close` finishes. The win is
//! *cross-connection* concurrency and never stalling non-MCP tasks — a single
//! connection's own calls still queue, exactly as the underlying protocol
//! requires.
//!
//! **Cancellation semantics.** If a task is cancelled (`async/cancel`,
//! `async/timeout` expiry) while it holds a connection's checkout, the slot is
//! tombstoned (see [`checkout_offload`]'s abort hook): the in-flight worker's
//! eventual reply is discarded (its channel send fails harmlessly), and the
//! connection itself — the `McpClient`, hence any child process/socket — drops
//! on the worker thread. Any *later* use of that handle fails fast with a
//! `SemaError` naming the reason and a reconnect hint; a task that was merely
//! *queued* (never actually held the checkout) when cancelled leaves the slot
//! untouched (a no-op abort) so the connection remains usable by others.
//!
//! **Lost-wakeup guard.** When a checked-out connection is returned
//! (`mcp/call`/`mcp/tools` succeed) or removed (`mcp/close`), the finishing
//! poller calls `sema_core::notify_io_complete()` — mandatory, because a
//! sibling task queued on the same slot may already have been polled `Pending`
//! earlier in the same `wake_blocked_tasks` sweep; without this poke its
//! next acquisition attempt could miss the wakeup and park until the next
//! scheduler timeout instead of promptly.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::future::Future;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use sema_core::cycle::GcEdge;
use sema_core::runtime::{
    downcast_send_payload, CancelDisposition, CancelHook, CancelHookError, CompletionDecoder,
    CompletionKind, DecodedCompletion, ExternalFailure, InterruptibleResource, NativeCallContext,
    NativeContinuation, NativeOutcome, NativeResult, NativeSuspend, PreparedExternalOperation,
    ResourceGateId, ResumeInput, RuntimeRequest, RuntimeResponse, SendPayload, Trace, WaitKind,
};
use sema_core::{
    check_arity, in_runtime_quantum, Caps, Env, NativeFn, Sandbox, SemaError, ToolDefinition, Value,
};

use crate::client::{McpClient, McpClientConfig, McpHttpConfig};
use crate::protocol::Tool;

thread_local! {
    // Keep MCP connections in a thread-local map so each Sema evaluator can own its own
    // client state without introducing cross-thread sharing.
    static CONNECTIONS: RefCell<HashMap<String, Rc<ConnEntry>>> = RefCell::new(HashMap::new());
    // The evaluator's sandbox, captured at registration, so the OAuth browser
    // launch (connect-time or mid-session re-auth) can be denied when `PROCESS`
    // is — opening the system browser spawns a process.
    static SANDBOX: RefCell<Sandbox> = RefCell::new(Sandbox::allow_all());
}

/// A browser opener that refuses to launch (spawn) a browser when the sandbox
/// denies `PROCESS`. Only invoked when a browser is actually needed (a full
/// login), so cached/refresh flows are unaffected. Public: the workflow
/// interactive-auth path (`crates/sema/src/workflow_mcp.rs`) reuses this exact
/// opener so a run-start browser login is gated identically to `mcp/connect`'s
/// own interactive path — never a separate, laxer gate.
///
/// SYNC-PATH USE ONLY: this reads the `SANDBOX` thread-local at INVOCATION
/// time (not construction time). The async-offload path must never use this —
/// see [`resolved_browser_opener`] and [`OpenerSource`].
pub fn gated_browser_opener() -> crate::oauth::loopback::BrowserOpener {
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

/// Whether the sandbox captured by the most recent [`register_mcp_builtins`]
/// call on this thread currently permits opening a browser (`Caps::PROCESS`).
/// Consult this BEFORE attempting an interactive login, not just
/// [`gated_browser_opener`]'s internal check: `LoopbackDriver::drive` runs the
/// opener on a spawned thread and discards its `Err` (`let _ = opener(&url)`),
/// so a denied opener alone would silently sit waiting for a redirect that can
/// never arrive, until the full login timeout elapses. Checking here lets a
/// denied sandbox degrade immediately to the headless `NeedsAuth` path.
///
/// Also the seam the async-offload path uses to resolve the browser-open
/// decision on the VM thread BEFORE offloading (see [`OpenerSource`]).
pub fn browser_open_allowed() -> bool {
    SANDBOX.with(|s| {
        let sb = s.borrow();
        sb.is_unrestricted()
            || sb
                .check(Caps::PROCESS, "mcp/connect (open browser)")
                .is_ok()
    })
}

/// A browser opener for the OFFLOADED reauth/connect path: the sandbox
/// decision is resolved ONCE, on the VM thread (via [`browser_open_allowed`])
/// BEFORE any thread hop, and baked into the returned closure as a plain
/// `bool`. It must NEVER read the `SANDBOX` thread-local itself — the
/// offloaded reauth code (and, one layer further in, `LoopbackDriver::drive`'s
/// own spawned thread) runs on background threads where that thread-local is
/// unpopulated (defaults to unrestricted), which would silently defeat the
/// gate. Mirrors the `TestOpenerFn` pattern in
/// `crates/sema/src/workflow_view/connect.rs`, which resolves its opener
/// choice on the calling thread for the identical reason.
fn resolved_browser_opener(allowed: bool) -> crate::oauth::loopback::BrowserOpener {
    Box::new(move |url: &str| {
        if !allowed {
            return Err(SemaError::PermissionDenied {
                function: "mcp/connect (open browser)".to_string(),
                capability: Caps::PROCESS.name().to_string(),
            }
            .to_string());
        }
        crate::oauth::loopback::open_browser(url)
    })
}

/// Where an OAuth login/reauth attempt's browser-open decision comes from.
/// `Live` (sync path, unchanged): read live off `SANDBOX` at the moment a
/// browser is actually needed — today's behavior. `Resolved` (async-offload
/// path): the decision was already made on the VM thread; the offloaded code
/// must never touch `SANDBOX` itself.
#[derive(Clone, Copy)]
enum OpenerSource {
    Live,
    Resolved(bool),
}

impl OpenerSource {
    fn opener(self) -> crate::oauth::loopback::BrowserOpener {
        match self {
            OpenerSource::Live => gated_browser_opener(),
            OpenerSource::Resolved(allowed) => resolved_browser_opener(allowed),
        }
    }
}

static HANDLE_COUNTER: AtomicU64 = AtomicU64::new(1);

/// A live MCP client connection. Holds only the transport — the identity/
/// auth-mode/tool-allowlist facts that must be readable WITHOUT a checkout
/// live in [`ConnMeta`] instead (see `ConnEntry`).
struct McpConnection {
    client: McpClient,
}

// ── Step 0 (task brief): compile-time Send gate ─────────────────────────────
//
// The async-offload design moves `McpConnection` values onto a background
// thread (`sema_io::io_spawn_blocking`) and back via a `tokio::sync::oneshot`
// channel. This assertion is the hard gate: if `McpConnection` (transitively,
// `McpClient` — the reqwest client, the stdio `Child`/pipes, the legacy-SSE
// transport's boxed `Stream`) ever stops being `Send`, this fails to compile
// loudly here instead of manifesting as a mysterious error deep in the
// offload plumbing.
#[allow(dead_code)]
fn _assert_send<T: Send>() {}
#[allow(dead_code)]
fn _assert_mcp_connection_is_send() {
    _assert_send::<McpConnection>();
}

/// Stable, cheap-to-clone facts about a connection that must be readable
/// WITHOUT checking it out — needed by `mcp/call`'s pre-offload step (tool
/// allowlist check, cassette identity) even while the connection itself is
/// checked out by another in-flight call. Set once at connect time, never
/// mutated afterward.
#[derive(Clone)]
struct ConnMeta {
    /// Stable server identity (url, or `stdio\0command args`) used to key the
    /// cassette so tool calls record/replay deterministically across runs.
    identity: String,
    /// From [`ConnectOpts::interactive_auth`]. `mcp/connect` always connects
    /// with `true`; a connection made via [`connect_from_config`] with
    /// `interactive_auth: false` stays non-interactive for its whole
    /// lifetime — a mid-session 401/403 in `reauthorize_async` may still
    /// refresh silently, but never falls back to a browser login (see
    /// [`NoInteractiveDriver`]).
    interactive_auth: bool,
    /// From [`ConnectOpts::allowed_tools`]. `None` is unrestricted (today's
    /// `mcp/connect` behavior); `Some(list)` restricts `mcp/call` and filters
    /// `mcp/tools`/`mcp/tools->sema` to exactly these tool names.
    allowed_tools: Option<Vec<String>>,
}

/// Checkout state for a connection's transport. MCP is serial-per-connection
/// (one JSON-RPC pipe): only one call may hold the transport at a time.
/// `Available` <-> `CheckedOut` under normal operation; `Tombstone` is a
/// one-way trapdoor entered when a checked-out connection's holder is
/// cancelled mid-call, or once `mcp/close` finishes — any further use errors
/// with the reason, naming a reconnect hint (see [`tombstone_error`]).
enum Slot {
    // Boxed: `McpConnection` (an `McpClient`, which nests a transport enum
    // wide enough to trip clippy's large-enum-variant lint against the unit
    // `CheckedOut` variant) shouldn't inflate every `Slot` on the stack.
    Available(Box<McpConnection>),
    CheckedOut,
    Tombstone(String),
}

/// One registry entry: metadata that's always readable, plus the checkout
/// state. Behind an `Rc` so a queued async call can hold its own reference
/// across many scheduler polls even if the handle is looked up again (or
/// closed) elsewhere on the VM thread in the meantime.
struct ConnEntry {
    meta: ConnMeta,
    slot: RefCell<Slot>,
    /// The connection's per-handle [`ResourceGateId`] under the unified runtime,
    /// created lazily on the first `in_runtime_quantum` `mcp/call` and reused for
    /// its later calls. The gate provides FIFO mutual exclusion over the serial
    /// JSON-RPC transport, replacing the old executor poll+retry queue: a second
    /// runtime-quantum `mcp/call` on a busy connection parks FIFO on the gate
    /// (no polling) instead of re-attempting the checkout every executor tick.
    /// `None` until the first runtime-quantum call creates it. Only ever touched
    /// on the VM thread.
    gate: Cell<Option<ResourceGateId>>,
}

/// A `SemaError` naming why a tombstoned handle can no longer be used, with a
/// reconnect hint. Constructed fresh from the stored reason string each time
/// (never itself stored — `SemaError` must never cross a thread boundary).
fn tombstone_error(reason: &str) -> SemaError {
    SemaError::eval(format!("mcp connection lost: {reason}"))
        .with_hint("reconnect with mcp/connect (or the workflow's :mcp manifest) and retry")
}

/// Attempt to take exclusive ownership of a connection's transport.
/// `Ok(Some(conn))` — acquired; the slot is now `CheckedOut`.
/// `Ok(None)` — busy (another call holds it); the caller should retry later.
/// `Err` — the slot is tombstoned (a prior cancellation/close orphaned it).
fn try_checkout(entry: &ConnEntry) -> Result<Option<McpConnection>, SemaError> {
    let mut slot = entry.slot.borrow_mut();
    match &*slot {
        Slot::Available(_) => {
            let Slot::Available(conn) = std::mem::replace(&mut *slot, Slot::CheckedOut) else {
                unreachable!("just matched Available")
            };
            Ok(Some(*conn))
        }
        Slot::CheckedOut => Ok(None),
        Slot::Tombstone(reason) => Err(tombstone_error(reason)),
    }
}

/// Return a connection to `Available` after a call completes (success OR
/// failure — a failed tool call doesn't mean the transport itself is broken).
fn checkin(entry: &ConnEntry, conn: McpConnection) {
    *entry.slot.borrow_mut() = Slot::Available(Box::new(conn));
}

/// Tombstone a connection's slot: cancellation mid-call, or a completed
/// `mcp/close`. Always called on the VM thread.
fn tombstone_slot(entry: &ConnEntry, reason: impl Into<String>) {
    *entry.slot.borrow_mut() = Slot::Tombstone(reason.into());
}

/// A connection's slot is `CheckedOut` when a SYNCHRONOUS (non-async-context)
/// call reaches it. This should be unreachable in practice: the cooperative
/// scheduler never runs concurrently with top-level code (a plain top-level
/// call blocks the one VM thread until it returns), so nothing else can be
/// mid-checkout when synchronous code touches a handle. Errors cleanly rather
/// than panicking/deadlocking if that invariant is ever violated by a bug.
fn busy_sync_error(handle: &str, label: &str) -> SemaError {
    SemaError::eval(format!(
        "{label}: mcp connection {handle} is unexpectedly busy (checked out by an in-flight \
         async call); this should not happen outside async context"
    ))
}

/// Drive a future to completion on the CALLING thread — the SYNC (top-level,
/// non-async-context) path. Routes through `sema_io::io_block_on`, the ADR
/// #69 sanctioned mechanism, rather than a private per-thread runtime.
///
/// This is a DELIBERATE, brief-sanctioned deviation from keeping the
/// pre-existing private `TOKIO_RT` thread-local (the brief's own wording
/// offered both: "keep TOKIO_RT/block_on... or route through the sanctioned
/// sema-io equivalent"). It is not optional, though: `tokio::process::Child`/
/// `ChildStdin`/`ChildStdout` (and a `reqwest::Client`'s pooled connections)
/// are permanently bound to the runtime that created them — a resource
/// created under a PRIVATE runtime (the old `TOKIO_RT`) can never be polled to
/// completion under a DIFFERENT one (the `sema-io` pool the async-offload path
/// uses) without hanging forever (verified empirically while implementing
/// MCP-4: a connection made via a synchronous `mcp/connect` and then called
/// from inside `async/spawn` parked on `AwaitIo` indefinitely). Routing every
/// MCP connection's entire lifecycle — connect AND every later call, sync or
/// offloaded — through the ONE shared pool is what makes "connect once
/// (sync), call from async tasks later" (the common/expected pattern, and
/// exactly what the workflow `:mcp` pre-phase does) actually work. Observable
/// behavior is unchanged: `io_block_on` blocks the calling thread until the
/// future resolves, exactly like the old private runtime did.
fn block_on<F: Future>(future: F) -> F::Output {
    sema_io::io_block_on(future)
}

fn next_handle() -> String {
    format!("mcp-{}", HANDLE_COUNTER.fetch_add(1, Ordering::SeqCst))
}

fn register_fn(env: &Env, name: &str, f: impl Fn(&[Value]) -> Result<Value, SemaError> + 'static) {
    env.set_str(name, Value::native_fn(NativeFn::simple(name, f)));
}

/// Build a dual-ABI native from an op body that speaks the runtime native ABI
/// (`NativeResult`). Under the unified runtime the runtime callback returns the
/// body's `NativeOutcome` (so an `mcp/call` external-wait suspend surfaces
/// structurally); outside it (bare eval / legacy scheduler) the legacy value
/// callback unwraps the plain `Return` the body produces there. Shared by
/// `mcp/call` and each dynamically-built `mcp/tools->sema` handler so both
/// inherit the offload-aware suspension with one body.
fn dual_native(name: String, body: impl Fn(&[Value]) -> NativeResult + 'static) -> NativeFn {
    let body = Rc::new(body);
    let for_func = body.clone();
    let for_runtime = body;
    let err_name = name.clone();
    NativeFn::simple_with_runtime(
        name,
        move |args| match for_func(args)? {
            NativeOutcome::Return(value) => Ok(value),
            _ => Err(SemaError::eval(format!(
                "{err_name}: native suspended outside the cooperative runtime"
            ))),
        },
        move |_ctx, args| for_runtime(args),
    )
}

/// Refuse a `mcp/connect` unless `cap` is granted. Unrestricted sandboxes pass.
fn gate(sandbox: &Sandbox, cap: Caps) -> Result<(), SemaError> {
    if !sandbox.is_unrestricted() {
        sandbox.check(cap, "mcp/connect")?;
    }
    Ok(())
}

/// The capability a connect needs, from the config's transport: `:url` is
/// network I/O, `:command` spawns a process. Shared by the sync and
/// async-offload entry points so both gate identically, on the VM thread,
/// BEFORE any offload is spawned.
fn connect_capability(config_json: &serde_json::Value) -> Caps {
    if config_json.get("url").and_then(|v| v.as_str()).is_some() {
        Caps::NETWORK
    } else {
        Caps::PROCESS
    }
}

/// Register a live connection under a fresh opaque handle and return the
/// handle. Always called on the VM thread (touches the `CONNECTIONS`
/// thread-local) — for the async-offload connect path that means from the
/// poller's finish step, never from inside the offloaded closure.
fn register_connection(client: McpClient, identity: String, opts: &ConnectOpts) -> Value {
    let handle = next_handle();
    let entry = Rc::new(ConnEntry {
        meta: ConnMeta {
            identity,
            interactive_auth: opts.interactive_auth,
            allowed_tools: opts.allowed_tools.clone(),
        },
        slot: RefCell::new(Slot::Available(Box::new(McpConnection { client }))),
        gate: Cell::new(None),
    });
    CONNECTIONS.with(|connections| {
        connections.borrow_mut().insert(handle.clone(), entry);
    });
    Value::string(&handle)
}

// ── Connect: async core (no thread-local access; safe under either driver) ──

/// Connect to a stdio MCP server (`:command` + optional `:args`/`:env`/`:cwd`)
/// and run the handshake. Stdio servers never speak OAuth. No thread-local
/// access — legal to drive via the sync `block_on` or `sema_io::io_block_on`.
async fn connect_stdio_async(
    config_json: &serde_json::Value,
) -> Result<(McpClient, String), SemaError> {
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
    let mut client = McpClient::connect(McpClientConfig {
        command: command.to_string(),
        args: args_vec,
        env: (!env_map.is_empty()).then_some(env_map),
        cwd,
    })
    .await
    .map_err(|err| SemaError::eval(format!("mcp/connect: {err}")))?;
    if let Err(err) = client.initialize().await {
        let _ = client.close().await;
        return Err(SemaError::eval(format!("mcp/connect: {err}")));
    }
    Ok((client, identity))
}

/// Connect to a remote Streamable-HTTP MCP server (`:url` + optional
/// `:headers`) and run the handshake.
///
/// When `opts.interactive_auth` is `false`, a `401`/`403` OAuth challenge is
/// never chased: [`obtain_access_token_async`] (and therefore the browser
/// opener and loopback listener it drives) is never called, and the connect
/// fails with [`ConnectOutcome::NeedsAuth`] instead. A caller that wants a
/// silent reconnect from a cached token should inject it directly via
/// `:headers {"Authorization" "Bearer …"}` before calling — that value already
/// flows straight into `McpHttpConfig.headers` below, so no new surface is
/// needed for token injection.
async fn connect_http_async(
    config_json: &serde_json::Value,
    opts: &ConnectOpts,
    opener_source: OpenerSource,
) -> Result<(McpClient, String), ConnectOutcome> {
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

    let mut client = McpClient::connect_http(McpHttpConfig {
        url: url.to_string(),
        headers: headers.clone(),
    })
    .await
    .map_err(|err| ConnectOutcome::Sema(SemaError::eval(format!("mcp/connect: {err}"))))?;

    if let Err(err) = client.initialize().await {
        if let Some(challenge) = client.http_challenge() {
            if !opts.interactive_auth {
                // Non-interactive: report the auth requirement instead of
                // chasing it — no discovery, no browser, no loopback bind.
                let _ = client.close().await;
                return Err(ConnectOutcome::NeedsAuth(url.to_string()));
            }
            // A `401` means the server requires OAuth; run the login flow, attach
            // the token, and retry. Some authenticated servers (e.g. Asana) gate
            // *all* requests behind auth and only reveal that they actually speak
            // the legacy HTTP+SSE transport once authorized — so a `404`/`405` on
            // the authenticated Streamable POST means "retry over legacy SSE with
            // the token attached", not a hard failure.
            let token = obtain_access_token_async(
                url,
                &challenge,
                preconfigured_client_id.as_deref(),
                opener_source,
            )
            .await
            .map_err(ConnectOutcome::Sema)?;
            headers.insert("Authorization".to_string(), format!("Bearer {token}"));
            client.set_bearer_token(&token);
            if let Err(err2) = client.initialize().await {
                let status = client.http_last_status();
                let _ = client.close().await;
                if matches!(status, Some(404) | Some(405)) {
                    return connect_legacy_async(url, headers).await;
                }
                return Err(ConnectOutcome::Sema(SemaError::eval(format!(
                    "mcp/connect: handshake failed after authorization: {err2}"
                ))));
            }
        } else if matches!(client.http_last_status(), Some(404) | Some(405)) {
            // Unauthenticated server that only speaks the deprecated 2024-11-05
            // HTTP+SSE transport (POST→4xx→GET-`endpoint`).
            let _ = client.close().await;
            return connect_legacy_async(url, headers).await;
        } else {
            let _ = client.close().await;
            return Err(ConnectOutcome::Sema(SemaError::eval(format!(
                "mcp/connect: {err}"
            ))));
        }
    }
    Ok((client, url.to_string()))
}

/// Connect over the deprecated 2024-11-05 HTTP+SSE transport. `headers` carries
/// any bearer token obtained during the OAuth flow, so authenticated legacy
/// servers (e.g. Asana) work.
async fn connect_legacy_async(
    url: &str,
    headers: HashMap<String, String>,
) -> Result<(McpClient, String), ConnectOutcome> {
    let mut legacy = McpClient::connect_legacy_sse(McpHttpConfig {
        url: url.to_string(),
        headers,
    })
    .await
    .map_err(|e| ConnectOutcome::Sema(SemaError::eval(format!("mcp/connect: {e}"))))?;
    legacy.initialize().await.map_err(|e| {
        ConnectOutcome::Sema(SemaError::eval(format!("mcp/connect (legacy SSE): {e}")))
    })?;
    Ok((legacy, url.to_string()))
}

/// Run (or reuse) the OAuth login for a remote server that answered `401`, and
/// return an access token. Uses the default credential store (keychain or file)
/// and a real loopback + system-browser flow.
async fn obtain_access_token_async(
    url: &str,
    challenge_header: &str,
    preconfigured_client_id: Option<&str>,
    opener_source: OpenerSource,
) -> Result<String, SemaError> {
    use crate::oauth::{discovery, login, loopback, store};

    let challenge = discovery::parse_www_authenticate(challenge_header);
    let http = reqwest::Client::new();
    let credential_store = store::default_store();
    let driver = loopback::LoopbackDriver::with_opener(
        std::time::Duration::from_secs(300),
        opener_source.opener(),
    )
    .map_err(|e| SemaError::eval(format!("mcp/connect: {e}")))?;

    let config = login::LoginConfig {
        mcp_url: url,
        resource_metadata_url: challenge.resource_metadata.as_deref(),
        requested_scope: challenge.scope.as_deref(),
        preconfigured_client_id,
    };

    login::ensure_access_token(&http, credential_store.as_ref(), &config, &driver)
        .await
        .map_err(|e| {
            SemaError::eval(format!("mcp/connect: OAuth login failed: {e}")).with_hint(
                "a browser should have opened to complete login; or pass a token via \
                 :headers {\"Authorization\" \"Bearer …\"}",
            )
        })
}

/// Dispatch on transport and connect (no sandbox gate — the caller runs
/// [`gate`] itself, on the VM thread, BEFORE calling this so a denied sandbox
/// fails fast without ever spawning an offload).
async fn connect_dispatch_async(
    config_json: serde_json::Value,
    opts: ConnectOpts,
    opener_source: OpenerSource,
) -> Result<(McpClient, String), ConnectOutcome> {
    if config_json.get("url").and_then(|v| v.as_str()).is_some() {
        connect_http_async(&config_json, &opts, opener_source).await
    } else {
        connect_stdio_async(&config_json)
            .await
            .map_err(ConnectOutcome::Sema)
    }
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
/// transport, and connect. SYNC PATH ONLY (drives [`connect_dispatch_async`]
/// via the blocking [`block_on`]) — shared by `mcp/connect`'s synchronous
/// branch and [`connect_from_config`] so the two can never drift. The
/// async-offload branch is [`connect_offload`], below.
fn connect_with_opts(
    config_json: &serde_json::Value,
    opts: &ConnectOpts,
) -> Result<Value, ConnectOutcome> {
    let sandbox = SANDBOX.with(|s| s.borrow().clone());
    gate(&sandbox, connect_capability(config_json)).map_err(ConnectOutcome::Sema)?;
    let (client, identity) = block_on(connect_dispatch_async(
        config_json.clone(),
        opts.clone(),
        OpenerSource::Live,
    ))?;
    Ok(register_connection(client, identity, opts))
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
///
/// Always synchronous (the pre-phase workflow auth-resolution step that calls
/// this runs before any concurrent fan-out starts, never inside
/// `async/spawn`) — see `docs/plans/2026-06-24-workflow-mcp-auth.md` §3.
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

fn lookup_entry(handle: &str) -> Result<Rc<ConnEntry>, SemaError> {
    CONNECTIONS
        .with(|connections| connections.borrow().get(handle).cloned())
        .ok_or_else(|| {
            SemaError::eval(format!(
                "mcp connection {handle} is not registered; it may have been closed"
            ))
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
fn check_tool_allowed(
    allowed_tools: &Option<Vec<String>>,
    tool_name: &str,
) -> Result<(), SemaError> {
    if tool_is_allowed(allowed_tools, tool_name) {
        return Ok(());
    }
    // `tool_is_allowed` only returns `false` for `Some(list)`, so this is populated.
    let allowed = allowed_tools.as_deref().unwrap_or_default();
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

fn replay_miss_error() -> SemaError {
    SemaError::eval("mcp/call: no cassette recording for this call (replay miss)".to_string())
        .with_hint(
            "re-record the tape with SEMA_LLM_CASSETTE_MODE=record, or the call arguments \
             drifted from what was recorded",
        )
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
/// `interactive_auth` selects the redirect driver: the real loopback+browser
/// flow, or [`NoInteractiveDriver`] so a login fallback fails cleanly instead
/// of popping a browser mid-run. `opener_source` resolves the browser opener
/// (see [`OpenerSource`]) — never touches `SANDBOX` when offloaded. No
/// thread-local access otherwise: legal under either driver.
async fn reauthorize_async(
    url: &str,
    status: Option<u16>,
    challenge: Option<&str>,
    interactive_auth: bool,
    opener_source: OpenerSource,
) -> Result<Option<String>, String> {
    let http = reqwest::Client::new();
    let store = crate::oauth::store::default_store();
    let result = if interactive_auth {
        let driver = crate::oauth::loopback::LoopbackDriver::with_opener(
            Duration::from_secs(300),
            opener_source.opener(),
        )
        .map_err(|e| format!("mcp/call: {e}"))?;
        crate::oauth::login::reauth_on_challenge(
            &http,
            store.as_ref(),
            url,
            status,
            challenge,
            None,
            &driver,
        )
        .await
    } else {
        crate::oauth::login::reauth_on_challenge(
            &http,
            store.as_ref(),
            url,
            status,
            challenge,
            None,
            &NoInteractiveDriver,
        )
        .await
    };
    result.map_err(|e| format!("mcp/call: re-authorization failed: {e}"))
}

/// Async core of one `tools/call`, including the mid-session reauth-on-401/403
/// retry. No thread-local access: `interactive_auth`/`opener_source` are
/// resolved by the caller (see the module doc on browser-opener hoisting).
/// Returns the RAW (unprefixed) error string on failure — every branch below
/// mirrors the pre-offload implementation's `"mcp/call: {err}"` prefix, added
/// uniformly by callers (see [`call_tool`]/[`call_tool_offload`]) since every
/// path used that exact literal prefix.
async fn call_tool_async(
    conn: &mut McpConnection,
    tool_name: &str,
    arguments_json: serde_json::Value,
    interactive_auth: bool,
    opener_source: OpenerSource,
) -> Result<serde_json::Value, String> {
    let err = match conn
        .client
        .call_tool(tool_name, arguments_json.clone())
        .await
    {
        Ok(value) => return Ok(value),
        Err(err) => err,
    };

    // A mid-session `401` (token expired) or `403 insufficient_scope` (needs
    // step-up) on a remote HTTP server means "re-authorize and retry once".
    let status = conn.client.http_last_status();
    let challenge = conn.client.http_challenge();
    let url = conn.client.http_url();
    if !matches!(status, Some(401) | Some(403)) {
        return Err(err);
    }
    let Some(url) = url else {
        return Err(err);
    };
    let token = match reauthorize_async(
        &url,
        status,
        challenge.as_deref(),
        interactive_auth,
        opener_source,
    )
    .await
    {
        Ok(Some(token)) => token,
        // Not an auth challenge we handle, or re-auth failed — surface the
        // original error.
        _ => return Err(err),
    };

    // Streamable HTTP swaps the header; legacy SSE reconnects its stream.
    conn.client.reauthorize_bearer(&token).await?;
    conn.client.call_tool(tool_name, arguments_json).await
}

async fn list_tools_async(conn: &mut McpConnection) -> Result<Vec<Tool>, String> {
    conn.client.list_tools().await
}

async fn close_async(conn: &mut McpConnection) -> Result<(), String> {
    conn.client.close().await
}

// ── Checkout + offload machinery shared by `mcp/call`, `mcp/tools`, `mcp/close` ──

// ── `mcp/call` (+ `mcp/tools->sema` handlers): shared entry point ──────────

/// Shared entry point for one `tools/call`, used by BOTH the plain `mcp/call`
/// builtin and each `mcp/tools->sema` handler. `materialize` turns the raw
/// JSON-RPC result into the native's return value: `mcp/call` just converts
/// it; the tool-adapter handler additionally treats `isError: true` as a Sema
/// error so the agent loop feeds the failure back to the model. This is what
/// lets the adapter handlers "inherit the async behavior with no extra work"
/// (task brief) — `materialize` runs inside the offload's `finish` step (VM
/// thread), so a handler never needs its own post-processing after a
/// synchronous return that, in async context, wouldn't have happened yet.
///
/// Handles both the sync (blocking) and async-offload paths. Pre-offload,
/// on the VM thread, always: the tool-allowlist check and the cassette
/// decide — a replay hit returns synchronously (no offload spawned), a
/// record-miss in replay mode errors synchronously.
fn call_tool(
    handle: &str,
    tool_name: &str,
    arguments_json: serde_json::Value,
    materialize: impl FnOnce(serde_json::Value) -> Result<Value, String> + 'static,
) -> NativeResult {
    let entry = lookup_entry(handle)?;
    check_tool_allowed(&entry.meta.allowed_tools, tool_name)?;
    let key = cassette_key(&entry.meta.identity, tool_name, &arguments_json);

    match sema_core::mcp_cassette_decide(&key) {
        Some(sema_core::McpCassetteDecision::Replay(recorded)) => {
            return materialize(recorded)
                .map(NativeOutcome::Return)
                .map_err(|e| SemaError::eval(format!("mcp/call: {e}")));
        }
        Some(sema_core::McpCassetteDecision::Miss) => return Err(replay_miss_error()),
        // Record mode or no cassette → perform the real call, then record it.
        _ => {}
    }

    // Unified-runtime path: a spawned task runs in a "runtime quantum", not the
    // legacy cooperative scheduler. Route the blocking JSON-RPC round trip
    // through the runtime's thread-pool executor as an external wait (the same
    // `NativeOutcome::Suspend` mechanism `sleep` uses) so two `mcp/call`s to
    // DIFFERENT connections overlap on separate workers instead of serializing
    // on the VM thread. Checked before the legacy `in_async_context` path.
    if in_runtime_quantum() {
        let interactive_auth = entry.meta.interactive_auth;
        let browser_allowed = browser_open_allowed();
        return mcp_call_runtime_outcome(
            entry,
            key,
            tool_name.to_string(),
            arguments_json,
            Box::new(materialize),
            interactive_auth,
            browser_allowed,
        );
    }

    let mut conn = try_checkout(&entry)?.ok_or_else(|| busy_sync_error(handle, "mcp/call"))?;
    let result = block_on(call_tool_async(
        &mut conn,
        tool_name,
        arguments_json,
        entry.meta.interactive_auth,
        OpenerSource::Live,
    ));
    checkin(&entry, conn);
    let raw = result.map_err(|e| SemaError::eval(format!("mcp/call: {e}")))?;
    sema_core::mcp_cassette_record(&key, &raw);
    materialize(raw)
        .map(NativeOutcome::Return)
        .map_err(|e| SemaError::eval(format!("mcp/call: {e}")))
}

// ── `mcp/call` unified-runtime (in_runtime_quantum) path ────────────────────
//
// Under the unified async runtime a spawned task runs in a "runtime quantum",
// driven by the runtime's own thread-pool executor, rather than the legacy
// cooperative scheduler + `sema-io` pool. Per-connection serialization is
// enforced by a first-class [`ResourceGate`] (mirroring the sqlite/kv checkout
// pattern in `sema-stdlib/src/runtime_offload.rs`), not by an executor poll
// loop. The lifecycle of one `mcp/call`:
//
//   1. `Runtime(CreateResourceGate)` if the connection has no gate yet; the id
//      is stored on the [`ConnEntry`] and reused for later calls.
//   2. `Suspend(ResourceSlot(gate))` — a free gate grants immediately; a busy
//      one parks the acquirer FIFO (no polling). On grant the `McpConnection` is
//      taken out of the slot (`try_checkout`).
//   3. `Suspend(External)` — the blocking JSON-RPC round trip runs off the VM
//      thread on the executor's blocking tier; the decoder checks the connection
//      back in and materializes the result on the VM thread.
//   4. `Runtime(ReleaseResourceGate)` — wake the FIFO head, then deliver / raise.
//
// Calls to DIFFERENT connections overlap on separate workers (each has its own
// gate); calls to the SAME connection serialize through its one gate. A
// mid-flight cancel tombstones the slot (the connection is stuck in the blocking
// worker, best-effort) and still releases the gate so a queued sibling wakes and
// fails fast on the tombstone.

/// Completion tag for the runtime `mcp/call` external op. A tag only needs to be
/// consistent between issue and prepared op; collisions with other external ops
/// are harmless (it is not a uniqueness key).
const MCP_CALL_COMPLETION_KIND: u64 = 0x6d63_7031; // "mcp1"

/// Turns the raw JSON-RPC result into the native's return value. **Must not
/// capture a Sema `Value`** — both call sites (`mcp/call` and the
/// `mcp/tools->sema` handlers) close over only plain data, so the decoder /
/// acquire continuation that hold one can report a complete (empty) GC trace.
type Materialize = Box<dyn FnOnce(serde_json::Value) -> Result<Value, String>>;

/// The `Send` payload the blocking `mcp/call` job hands back to the VM thread:
/// the connection (so it can be checked back in) plus the raw JSON-RPC result.
/// Only `Send` data — `McpConnection` is `Send` (see
/// `_assert_mcp_connection_is_send`) and `serde_json::Value`/`String` are plain.
struct McpCallPayload {
    conn: McpConnection,
    result: Result<serde_json::Value, String>,
}

/// Tombstones a connection whose in-flight `mcp/call` is cancelled mid-flight:
/// the blocking job keeps running on its worker and its eventual result is
/// discarded (a late completion), so the `McpConnection` it owns drops
/// off-thread and the slot must never return to `Available`. The wait runtime
/// invokes `cancel`/`reap` on the VM thread, so holding an `Rc<ConnEntry>` is
/// sound; a checked-out slot owns no connection, so `trace` is trivially
/// complete.
struct McpCallCancelHook {
    entry: Rc<ConnEntry>,
}

impl Trace for McpCallCancelHook {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}

impl CancelHook for McpCallCancelHook {
    fn cancel(&mut self) -> Result<CancelDisposition, CancelHookError> {
        tombstone_slot(&self.entry, "cancelled mid-call");
        Ok(CancelDisposition::Reaped)
    }
    fn reap(&mut self) -> Result<CancelDisposition, CancelHookError> {
        Ok(CancelDisposition::Reaped)
    }
}

/// Decodes the worker's completion for a real `mcp/call` on the VM thread:
/// checks the connection back in, records the cassette, and materializes the
/// result. A worker panic / undeliverable payload tombstones the slot (the
/// connection is lost) and surfaces as an evaluation error.
struct McpCallDecoder {
    entry: Rc<ConnEntry>,
    cassette_key: String,
    materialize: Materialize,
}

impl Trace for McpCallDecoder {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        // `entry` (a checked-out slot) owns no connection and `materialize`
        // captures no `Value` (see [`Materialize`]) — nothing to trace.
        true
    }
}

impl CompletionDecoder for McpCallDecoder {
    fn decode(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        result: Result<SendPayload, ExternalFailure>,
    ) -> DecodedCompletion {
        let McpCallDecoder {
            entry,
            cassette_key,
            materialize,
        } = *self;
        let payload = match result {
            Ok(payload) => payload,
            Err(failure) => {
                // Worker panic (or an undeliverable payload): the connection
                // dropped off-thread — tombstone so a later use fails cleanly.
                tombstone_slot(&entry, "mcp/call worker failed");
                return Err(SemaError::eval(format!("mcp/call: {}", failure.message())));
            }
        };
        let McpCallPayload { conn, result } =
            match downcast_send_payload::<McpCallPayload>(payload, "mcp/call") {
                Ok(payload) => payload,
                Err(failure) => {
                    tombstone_slot(&entry, "mcp/call payload decode failed");
                    return Err(SemaError::eval(format!("mcp/call: {}", failure.message())));
                }
            };
        checkin(&entry, conn);
        let raw = result.map_err(|e| SemaError::eval(format!("mcp/call: {e}")))?;
        sema_core::mcp_cassette_record(&cassette_key, &raw);
        materialize(raw).map_err(|e| SemaError::eval(format!("mcp/call: {e}")))
    }
}

/// The `mcp/call` state carried between gate lifecycle stages (create → acquire
/// → external → release). Threaded through the continuations so the connection
/// stays serialized on its [`ResourceGate`] without any executor polling. Holds
/// no live `Value` (see [`Materialize`]); `arguments_json` is plain JSON.
struct McpCallState {
    entry: Rc<ConnEntry>,
    cassette_key: String,
    tool_name: String,
    arguments_json: serde_json::Value,
    materialize: Materialize,
    interactive_auth: bool,
    browser_allowed: bool,
}

/// Stage 0: a freshly-created gate arrives; store it on the connection entry,
/// then suspend on its slot. Holds no `Value`.
struct McpCreateGateCont {
    state: McpCallState,
}

impl Trace for McpCreateGateCont {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}

impl NativeContinuation for McpCreateGateCont {
    fn resume(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        let state = self.state;
        match input {
            ResumeInput::Runtime(RuntimeResponse::ResourceGate(gate)) => {
                state.entry.gate.set(Some(gate));
                Ok(NativeOutcome::Suspend(NativeSuspend {
                    wait: WaitKind::ResourceSlot(gate),
                    continuation: Box::new(McpAcquireCont { state, gate }),
                }))
            }
            ResumeInput::Failed(error) => Err(error),
            ResumeInput::Cancelled(reason) => Err(SemaError::eval(format!(
                "mcp/call was cancelled before its connection gate was created ({reason:?})"
            ))),
            ResumeInput::Returned(_) | ResumeInput::Runtime(_) => Err(SemaError::eval(
                "mcp/call: unexpected runtime response creating connection gate",
            )),
        }
    }
}

/// Stage 1: the gate slot is granted; check the connection out and offload the
/// blocking JSON-RPC round trip as an External wait. A tombstoned/busy slot
/// releases the gate (waking the next acquirer, who also fails fast) then raises.
/// Holds no `Value`.
struct McpAcquireCont {
    state: McpCallState,
    gate: ResourceGateId,
}

impl Trace for McpAcquireCont {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}

impl NativeContinuation for McpAcquireCont {
    fn resume(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        let McpAcquireCont { state, gate } = *self;
        match input {
            // Slot granted: we now own `gate`.
            ResumeInput::Runtime(RuntimeResponse::Value(_)) => {
                let McpCallState {
                    entry,
                    cassette_key,
                    tool_name,
                    arguments_json,
                    materialize,
                    interactive_auth,
                    browser_allowed,
                } = state;
                match try_checkout(&entry) {
                    Ok(Some(conn)) => {
                        let kind = CompletionKind::try_from_raw(MCP_CALL_COMPLETION_KIND)
                            .expect("mcp/call completion kind is nonzero");
                        let decoder = Box::new(McpCallDecoder {
                            entry: entry.clone(),
                            cassette_key,
                            materialize,
                        });
                        let resource = InterruptibleResource::new(
                            "mcp/call",
                            Box::new(McpCallCancelHook { entry }),
                        );
                        let prepared = PreparedExternalOperation::interruptible_blocking(
                            kind,
                            decoder,
                            resource,
                            move || {
                                let mut conn = conn;
                                let result = sema_io::io_block_on(call_tool_async(
                                    &mut conn,
                                    &tool_name,
                                    arguments_json,
                                    interactive_auth,
                                    OpenerSource::Resolved(browser_allowed),
                                ));
                                Ok(Box::new(McpCallPayload { conn, result }) as SendPayload)
                            },
                        );
                        Ok(NativeOutcome::Suspend(NativeSuspend {
                            wait: WaitKind::External(Box::new(prepared)),
                            continuation: Box::new(McpReleaseReturnCont { gate }),
                        }))
                    }
                    // Slot tombstoned/missing (a prior cancel orphaned it): we own
                    // the gate, so release it (waking the next acquirer, who also
                    // fails fast) then raise. `Ok(None)` (busy) is unreachable
                    // while we hold the gate, but is treated the same as an error.
                    Ok(None) => Ok(NativeOutcome::Runtime(
                        RuntimeRequest::ReleaseResourceGate {
                            gate,
                            continuation: Box::new(McpFinalCont::Fail(SemaError::eval(
                                "mcp/call: connection unexpectedly busy while holding its gate",
                            ))),
                        },
                    )),
                    Err(error) => Ok(NativeOutcome::Runtime(
                        RuntimeRequest::ReleaseResourceGate {
                            gate,
                            continuation: Box::new(McpFinalCont::Fail(error)),
                        },
                    )),
                }
            }
            // Gate closed while we were queued: never owned it, just raise.
            ResumeInput::Failed(error) => Err(error),
            // Cancelled while queued: the runtime's ResourceSlot cancel arm already
            // removed us from the FIFO; we never owned the gate, so the connection
            // is untouched.
            ResumeInput::Cancelled(reason) => Err(SemaError::eval(format!(
                "mcp/call was cancelled while waiting for its connection ({reason:?})"
            ))),
            ResumeInput::Returned(_) | ResumeInput::Runtime(_) => Err(SemaError::eval(
                "mcp/call: unexpected runtime response acquiring connection",
            )),
        }
    }
}

/// Stage 2: the blocking call completed / failed / was cancelled — release the
/// gate (waking the FIFO head), then deliver the decoded value or raise. Holds
/// only the gate id (`Copy`), so it is trivially edge-free.
struct McpReleaseReturnCont {
    gate: ResourceGateId,
}

impl Trace for McpReleaseReturnCont {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}

impl NativeContinuation for McpReleaseReturnCont {
    fn resume(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        let final_cont: Box<dyn NativeContinuation> = match input {
            ResumeInput::Returned(value) => Box::new(McpFinalCont::Value(value)),
            ResumeInput::Failed(error) => Box::new(McpFinalCont::Fail(error)),
            ResumeInput::Cancelled(reason) => Box::new(McpFinalCont::Fail(SemaError::eval(
                format!("mcp/call was cancelled ({reason:?})"),
            ))),
            ResumeInput::Runtime(_) => Box::new(McpFinalCont::Fail(SemaError::eval(
                "mcp/call: unexpected runtime response after offload",
            ))),
        };
        Ok(NativeOutcome::Runtime(
            RuntimeRequest::ReleaseResourceGate {
                gate: self.gate,
                continuation: final_cont,
            },
        ))
    }
}

/// Stage 3: the gate is released; deliver the resolved outcome to the caller.
/// `McpFinalCont::Value` carries the decoded result across the gate-release
/// round-trip and traces it; the error arm traces any embedded `Value`.
enum McpFinalCont {
    Value(Value),
    Fail(SemaError),
}

impl Trace for McpFinalCont {
    fn trace(&self, sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        match self {
            Self::Value(value) => {
                sink(GcEdge::Value(value));
                true
            }
            Self::Fail(error) => error.trace(sink),
        }
    }
}

impl NativeContinuation for McpFinalCont {
    fn resume(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        _input: ResumeInput,
    ) -> NativeResult {
        match *self {
            McpFinalCont::Value(value) => Ok(NativeOutcome::Return(value)),
            McpFinalCont::Fail(error) => Err(error),
        }
    }
}

/// Build the `NativeOutcome` for one `mcp/call` under the unified runtime.
/// Acquires the connection's [`ResourceGate`] (creating it on first use) so
/// calls on the SAME connection serialize FIFO while calls to DIFFERENT
/// connections overlap; the actual JSON-RPC round trip runs off the VM thread as
/// an External wait once the gate is owned. The old poll+retry queue is gone.
fn mcp_call_runtime_outcome(
    entry: Rc<ConnEntry>,
    cassette_key: String,
    tool_name: String,
    arguments_json: serde_json::Value,
    materialize: Materialize,
    interactive_auth: bool,
    browser_allowed: bool,
) -> NativeResult {
    let state = McpCallState {
        entry,
        cassette_key,
        tool_name,
        arguments_json,
        materialize,
        interactive_auth,
        browser_allowed,
    };
    match state.entry.gate.get() {
        Some(gate) => Ok(NativeOutcome::Suspend(NativeSuspend {
            wait: WaitKind::ResourceSlot(gate),
            continuation: Box::new(McpAcquireCont { state, gate }),
        })),
        None => Ok(NativeOutcome::Runtime(RuntimeRequest::CreateResourceGate {
            continuation: Box::new(McpCreateGateCont { state }),
        })),
    }
}

// ── `mcp/tools` / `mcp/tools->sema`: shared listing entry point ────────────

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

/// Build `mcp/tools`' plain `{:name :description :input-schema}` list,
/// filtered to the connection's `:tools` manifest.
fn tools_to_value(tools: Vec<Tool>, allowed_tools: &Option<Vec<String>>) -> Value {
    let items = tools
        .into_iter()
        .filter(|tool| tool_is_allowed(allowed_tools, &tool.name))
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
    Value::list(items)
}

/// Build `mcp/tools->sema`'s `deftool`-shaped list: each entry's handler
/// routes through [`call_tool`] (the shared, offload-aware call path), so it
/// automatically offloads when invoked from inside an async task — no extra
/// work in the handler itself.
fn tool_defs_to_value(
    tools: Vec<Tool>,
    allowed_tools: &Option<Vec<String>>,
    connection_handle: &str,
) -> Value {
    let mut items = Vec::new();
    for tool in tools {
        if !tool_is_allowed(allowed_tools, &tool.name) {
            // Not in the connection's :tools manifest — an agent given
            // this connection's tools must never even see it.
            continue;
        }
        let (parameters, ordered) = schema_to_params(&tool.input_schema);
        let tool_name = tool.name.clone();
        let connection_handle = connection_handle.to_string();
        let handler_name = format!("mcp/{tool_name}");
        let handler = Value::native_fn(dual_native(handler_name, move |args| {
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
            let tool_name_for_err = tool_name.clone();
            call_tool(
                &connection_handle,
                &tool_name,
                serde_json::Value::Object(arguments),
                move |raw| {
                    // Surface a tool-reported failure as an error so the agent
                    // loop feeds it back to the model instead of treating it
                    // as success.
                    if result_is_error(&raw) {
                        let detail = result_text(&raw).unwrap_or_else(|| raw.to_string());
                        return Err(format!(
                            "mcp tool `{tool_name_for_err}` returned an error: {detail}"
                        ));
                    }
                    Ok(result_to_value(&raw))
                },
            )
        }));
        items.push(Value::tool_def(ToolDefinition {
            name: tool.name,
            description: tool.description,
            parameters,
            handler,
        }));
    }
    Value::list(items)
}

/// Shared `mcp/tools` / `mcp/tools->sema` entry: checkout+offload-aware
/// `tools/list`, then `on_ready` (VM thread) shapes the raw tools into either
/// builtin's Sema value.
fn fetch_tools(
    handle: &str,
    label: &'static str,
    on_ready: impl FnOnce(Vec<Tool>, &ConnMeta) -> Value + 'static,
) -> Result<Value, SemaError> {
    let entry = lookup_entry(handle)?;
    let mut conn = try_checkout(&entry)?.ok_or_else(|| busy_sync_error(handle, label))?;
    let result = block_on(list_tools_async(&mut conn));
    checkin(&entry, conn);
    let tools = result.map_err(|err| SemaError::eval(format!("{label}: {err}")))?;
    Ok(on_ready(tools, &entry.meta))
}

// ── `mcp/close` ─────────────────────────────────────────────────────────────

/// Best-effort close of a connection by its opaque handle `Value`, same
/// semantics as the `mcp/close` builtin but NEVER errors (a missing/already-closed
/// handle, a busy/tombstoned slot, or a close-protocol failure, is silently
/// ignored). For callers outside Sema code that need "close everything, and
/// don't let a close failure mask the real outcome" — namely the workflow
/// `:mcp` auth-resolution seam's `WorkflowMcpResolver::close`, which
/// `workflow/run` calls from its failure/cleanup paths
/// (`docs/plans/2026-06-24-workflow-mcp-auth.md` §3). Always synchronous, like
/// the rest of that pre-phase resolution path.
pub fn close_handle(handle: &Value) {
    let Some(handle_str) = handle.as_str() else {
        return;
    };
    let entry = CONNECTIONS.with(|connections| connections.borrow_mut().remove(handle_str));
    let Some(entry) = entry else {
        return;
    };
    if let Ok(Some(mut conn)) = try_checkout(&entry) {
        let _ = block_on(close_async(&mut conn));
    }
}

pub fn register_mcp_builtins(env: &Env, sandbox: &Sandbox) {
    // Capture the sandbox so the OAuth browser launch can honor the PROCESS cap.
    SANDBOX.with(|s| *s.borrow_mut() = sandbox.clone());

    // `mcp/connect` picks its transport from the config map at runtime, so the
    // capability it needs is not fixed: a `:url` server is network I/O
    // (`NETWORK`), a `:command` server spawns a process (`PROCESS`). Gating and
    // dispatch live in `connect_with_opts`/`connect_offload`, shared with
    // `connect_from_config`; this just supplies the interactive, unrestricted
    // options `mcp/connect` has always used.
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
        fetch_tools(handle, "mcp/tools", |tools, meta| {
            tools_to_value(tools, &meta.allowed_tools)
        })
    });

    register_fn(env, "mcp/tools->sema", |args| {
        check_arity!(args, "mcp/tools->sema", 1);
        let handle = require_handle(args, "mcp/tools->sema")?;
        let connection_handle = handle.to_string();
        fetch_tools(handle, "mcp/tools->sema", move |tools, meta| {
            tool_defs_to_value(tools, &meta.allowed_tools, &connection_handle)
        })
    });

    env.set_str(
        "mcp/call",
        Value::native_fn(dual_native("mcp/call".to_string(), |args| {
            check_arity!(args, "mcp/call", 3);
            let handle = require_handle(args, "mcp/call")?;
            let tool_name = args[1].as_str().ok_or_else(|| {
                SemaError::type_error("string", args[1].type_name())
                    .with_hint("mcp/call expects the tool name as a string")
            })?;
            let arguments_json = sema_core::value_to_json_lossy(&args[2]);
            call_tool(handle, tool_name, arguments_json, |raw| {
                Ok(result_to_value(&raw))
            })
        })),
    );

    register_fn(env, "mcp/close", |args| {
        check_arity!(args, "mcp/close", 1);
        let handle = require_handle(args, "mcp/close")?;
        let entry = CONNECTIONS.with(|connections| connections.borrow_mut().remove(handle));
        let Some(entry) = entry else {
            return Err(SemaError::eval(format!(
                "mcp connection {handle} is not registered; it may have already been closed"
            )));
        };
        let mut conn = try_checkout(&entry)?.ok_or_else(|| busy_sync_error(handle, "mcp/close"))?;
        let result = block_on(close_async(&mut conn));
        result.map_err(|err| SemaError::eval(format!("mcp/close: {err}")))?;
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

    #[test]
    fn mcp_connection_is_send() {
        _assert_mcp_connection_is_send();
    }

    #[test]
    fn checkout_busy_then_tombstoned() {
        let entry = Rc::new(ConnEntry {
            meta: ConnMeta {
                identity: "test".to_string(),
                interactive_auth: false,
                allowed_tools: None,
            },
            slot: RefCell::new(Slot::CheckedOut),
            gate: Cell::new(None),
        });
        // Busy: nothing to check out yet.
        assert!(matches!(try_checkout(&entry), Ok(None)));
        // Tombstoned: errors, names the reason.
        tombstone_slot(&entry, "cancelled mid-call");
        let err = match try_checkout(&entry) {
            Err(e) => e,
            Ok(_) => panic!("tombstoned slot must error"),
        };
        assert!(err.to_string().contains("connection lost"));
        assert!(err.to_string().contains("cancelled mid-call"));
    }
}
