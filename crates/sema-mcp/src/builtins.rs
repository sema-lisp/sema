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
//! ## Runtime waits
//!
//! During a unified-runtime quantum, connect and every connection operation
//! suspend on cancellable External waits. Operations on an established handle
//! first acquire its ResourceGate, so one JSON-RPC pipe remains serial while
//! different connections overlap. Top-level calls remain synchronous and drive
//! the same transport futures through [`block_on`].
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
//! tombstoned by the External wait's cancellation hook. That hook also fires a
//! pre-armed one-shot; the worker's biased select drops the transport future,
//! and the connection itself — the `McpClient`, hence any child process/socket
//! — drops on the worker thread. Any late completion is discarded by the
//! runtime. A *later* use of that handle fails fast with a
//! `SemaError` naming the reason and a reconnect hint; a task that was merely
//! *queued* (never actually held the checkout) when cancelled leaves the slot
//! untouched so the connection remains usable by others. `mcp/close` is the
//! exception: it removes the public handle before queueing, so cancelling that
//! queued close closes the terminal gate and wakes every remaining waiter.

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
    ResourceGateCloseError, ResourceGateHandle, ResourceGateId, ResumeInput, RuntimeRequest,
    RuntimeResponse, SendPayload, Trace, WaitKind,
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
    // HOST-ADAPTER-ONLY (C5 / ledger C04). A deliberately retained ambient
    // last-wins authority slot for Rust host entry points that have NO evaluator
    // context to capture a sandbox from — `connect_from_config`, `host_capability_allowed`,
    // and the workflow OAuth browser-opener helpers (`browser_open_allowed`).
    // `register_mcp_builtins` writes the registering evaluator's sandbox here, and
    // the reads on lines below `.clone()`/check it before any thread hop.
    //
    // This is NOT the authority for Sema-callable MCP natives: those each capture
    // their registering evaluator's sandbox directly (`ee24c700`), so two
    // interpreters on one thread never cross-authorize (regression:
    // `mcp_builtin_test.rs`). The seam is single-thread host-only: it is never read
    // on a background/worker thread (`resolved_browser_opener`/`BrowserAuthority`
    // carry an already-resolved `bool` across every thread hop) and never inside a
    // runtime quantum. It is pinned by a `HOST_SANDBOX` row in
    // `scripts/unified-runtime-host-adapters.tsv`; adding an unallowlisted
    // `HOST_SANDBOX.with` site fails `scripts/check-unified-runtime-legacy.sh`.
    static HOST_SANDBOX: RefCell<Sandbox> = RefCell::new(Sandbox::allow_all());
}

fn sandbox_allows_browser(sandbox: &Sandbox) -> bool {
    sandbox.is_unrestricted()
        || sandbox
            .check(Caps::PROCESS, "mcp/connect (open browser)")
            .is_ok()
}

/// A browser opener that refuses to launch (spawn) a browser when the sandbox
/// denies `PROCESS`. Only invoked when a browser is actually needed (a full
/// login), so cached/refresh flows are unaffected. Public: the workflow
/// interactive-auth path (`crates/sema/src/workflow_mcp.rs`) reuses this exact
/// opener so a run-start browser login is gated identically to `mcp/connect`'s
/// own interactive path — never a separate, laxer gate.
///
/// Sync host-adapter use only: authority is resolved from `HOST_SANDBOX` when
/// the opener is constructed, before `LoopbackDriver` moves it to its opener
/// thread. Evaluator-owned paths use [`BrowserAuthority`] directly.
pub fn gated_browser_opener() -> crate::oauth::loopback::BrowserOpener {
    resolved_browser_opener(browser_open_allowed())
}

/// Whether the sandbox captured by the most recent [`register_mcp_builtins`]
/// call on this thread currently permits opening a browser (`Caps::PROCESS`).
/// Consult this before attempting an interactive login so a denied host can
/// degrade immediately to the headless `NeedsAuth` path without binding a
/// loopback listener.
///
/// This is a host-compatibility seam for workflow code without an evaluator
/// context. Sema natives resolve the same check from their captured sandbox.
pub fn browser_open_allowed() -> bool {
    HOST_SANDBOX.with(|sandbox| sandbox_allows_browser(&sandbox.borrow()))
}

/// A browser opener carrying an already-resolved decision as a plain `bool`.
/// It never reads `HOST_SANDBOX`; background threads have independent TLS and
/// must not select authority themselves.
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

/// An evaluator or host adapter's browser-open authority, resolved before any
/// worker or browser-opener thread hop.
#[derive(Clone, Copy)]
enum BrowserAuthority {
    Allowed,
    Denied,
}

impl BrowserAuthority {
    fn from_allowed(allowed: bool) -> Self {
        if allowed {
            Self::Allowed
        } else {
            Self::Denied
        }
    }

    fn from_sandbox(sandbox: &Sandbox) -> Self {
        Self::from_allowed(sandbox_allows_browser(sandbox))
    }

    fn redirect_driver(
        self,
        timeout: Duration,
    ) -> Result<Box<dyn crate::oauth::loopback::RedirectDriver>, String> {
        match self {
            Self::Allowed => Ok(Box::new(
                crate::oauth::loopback::LoopbackDriver::with_opener(
                    timeout,
                    resolved_browser_opener(true),
                )?,
            )),
            Self::Denied => Ok(Box::new(SandboxDeniedDriver)),
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
    /// The connection's owning per-handle resource-gate capability, created
    /// lazily on the first `in_runtime_quantum` `mcp/call` and reused for its
    /// later calls. The gate provides FIFO mutual exclusion over the serial
    /// JSON-RPC transport, replacing the old executor poll+retry queue: a second
    /// runtime-quantum `mcp/call` on a busy connection parks FIFO on the gate
    /// (no polling) instead of re-attempting the checkout every executor tick.
    /// `None` until the first runtime-quantum call creates it. Only ever touched
    /// on the VM thread.
    gate: RefCell<Option<ResourceGateHandle>>,
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
/// non-runtime) path. Routes through `sema_io::io_block_on`, the ADR
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
/// from inside `async/spawn` could not be driven on a private runtime). Routing every
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

/// Build a dual-ABI native from an op body that speaks the runtime native ABI
/// (`NativeResult`). Under the unified runtime the runtime callback returns the
/// body's `NativeOutcome` (so an External-wait suspend surfaces
/// structurally); outside a runtime quantum, the value callback accepts the
/// plain `Return` produced by the synchronous path.
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
/// network I/O, `:command` spawns a process. Shared by the runtime and
/// synchronous entry points so both gate identically, on the VM thread, BEFORE
/// any offload is spawned.
fn connect_capability(config_json: &serde_json::Value) -> Caps {
    if config_json.get("url").and_then(|v| v.as_str()).is_some() {
        Caps::NETWORK
    } else {
        Caps::PROCESS
    }
}

/// Register a live connection under a fresh opaque handle and return the
/// handle. Always called on the VM thread (touches the `CONNECTIONS`
/// thread-local) — for the runtime path that means from the completion decoder,
/// never from inside the offloaded closure.
fn register_connection(client: McpClient, identity: String, opts: &ConnectOpts) -> Value {
    let handle = next_handle();
    let entry = Rc::new(ConnEntry {
        meta: ConnMeta {
            identity,
            interactive_auth: opts.interactive_auth,
            allowed_tools: opts.allowed_tools.clone(),
        },
        slot: RefCell::new(Slot::Available(Box::new(McpConnection { client }))),
        gate: RefCell::new(None),
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
    browser_authority: BrowserAuthority,
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
                browser_authority,
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
/// return an access token. Uses the default credential store (keychain or file).
/// Cached and refresh tokens stay silent; a fresh consent uses either a real
/// loopback browser driver or the immediate sandbox-denial driver.
async fn obtain_access_token_async(
    url: &str,
    challenge_header: &str,
    preconfigured_client_id: Option<&str>,
    browser_authority: BrowserAuthority,
) -> Result<String, SemaError> {
    use crate::oauth::{discovery, login, store};

    let challenge = discovery::parse_www_authenticate(challenge_header);
    let http = reqwest::Client::new();
    let credential_store = store::default_store();
    let driver = browser_authority
        .redirect_driver(std::time::Duration::from_secs(300))
        .map_err(|e| SemaError::eval(format!("mcp/connect: {e}")))?;

    let config = login::LoginConfig {
        mcp_url: url,
        resource_metadata_url: challenge.resource_metadata.as_deref(),
        requested_scope: challenge.scope.as_deref(),
        preconfigured_client_id,
    };

    login::ensure_access_token(&http, credential_store.as_ref(), &config, driver.as_ref())
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
    browser_authority: BrowserAuthority,
) -> Result<(McpClient, String), ConnectOutcome> {
    if config_json.get("url").and_then(|v| v.as_str()).is_some() {
        connect_http_async(&config_json, &opts, browser_authority).await
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

/// Gate on an explicit sandbox, dispatch on transport, and connect. Sync path
/// only (drives [`connect_dispatch_async`] via the blocking [`block_on`]) —
/// shared by `mcp/connect` and [`connect_from_config`] so transport behavior
/// stays identical while their authority sources remain distinct.
fn connect_with_opts(
    sandbox: &Sandbox,
    config_json: &serde_json::Value,
    opts: &ConnectOpts,
    browser_authority: BrowserAuthority,
) -> Result<Value, ConnectOutcome> {
    gate(sandbox, connect_capability(config_json)).map_err(ConnectOutcome::Sema)?;
    let (client, identity) = block_on(connect_dispatch_async(
        config_json.clone(),
        opts.clone(),
        browser_authority,
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
    let sandbox = HOST_SANDBOX.with(|sandbox| sandbox.borrow().clone());
    let outcome = value_to_config_json(config)
        .map_err(ConnectOutcome::Sema)
        .and_then(|config_json| {
            connect_with_opts(
                &sandbox,
                &config_json,
                &opts,
                BrowserAuthority::from_sandbox(&sandbox),
            )
        });
    outcome.map_err(connect_outcome_to_failure)
}

/// A successfully-connected client produced off the VM thread by
/// [`connect_send`] and registered on the VM thread by [`register_connected`].
/// `McpClient` is `Send` (the compile-time gate `_assert_mcp_connection_is_send`
/// pins that), so this travels back from a blocking worker as a `SendPayload`
/// without any `Value`/`Rc` crossing the thread boundary — the opaque handle
/// `Value` is minted only in `register_connected`, on the VM thread.
pub struct ConnectedClient {
    client: McpClient,
    identity: String,
}

/// Connect to an MCP server from a config value on the CURRENT thread — a plain
/// executor worker where `io_block_on` is legal — WITHOUT touching the
/// `CONNECTIONS` thread-local. Returns the live `Send` client for the caller to
/// register on the VM thread via [`register_connected`]. Browser authority is
/// supplied as a pre-resolved `bool` (captured from the VM-thread sandbox);
/// this function never reads `HOST_SANDBOX`, so the caller MUST apply the
/// capability gate on the VM thread before offloading (see
/// [`host_capability_allowed`]). This is the workflow resolver's
/// off-runtime-quantum connect path; the synchronous [`connect_from_config`]
/// stays the host/CLI entry point.
pub async fn connect_send(
    config: &Value,
    opts: &ConnectOpts,
    browser_allowed: bool,
) -> Result<ConnectedClient, ConnectFailure> {
    let config_json = value_to_config_json(config)
        .map_err(|error| connect_outcome_to_failure(ConnectOutcome::Sema(error)))?;
    match connect_dispatch_async(
        config_json,
        opts.clone(),
        BrowserAuthority::from_allowed(browser_allowed),
    )
    .await
    {
        Ok((client, identity)) => Ok(ConnectedClient { client, identity }),
        Err(outcome) => Err(connect_outcome_to_failure(outcome)),
    }
}

/// Register a client connected off the VM thread (via [`connect_send`]) under a
/// fresh opaque handle in the thread-local `CONNECTIONS` table, on the CURRENT
/// (VM) thread. Returns the same opaque handle-string `Value` `mcp/connect`
/// yields, usable with `mcp/call`/`mcp/close`/… on this thread.
pub fn register_connected(connected: ConnectedClient, opts: &ConnectOpts) -> Value {
    register_connection(connected.client, connected.identity, opts)
}

/// Whether the thread-local host sandbox currently permits `cap`. The workflow
/// resolver captures `Caps::NETWORK`/`Caps::PROCESS` with this on the VM thread
/// BEFORE it offloads connects to a worker (whose `HOST_SANDBOX` is a separate,
/// default-unrestricted thread-local) — so the connect capability gate
/// (`Caps::NETWORK` for `:url`, `Caps::PROCESS` for `:command`) is enforced with
/// the caller's real authority rather than the worker's.
pub fn host_capability_allowed(cap: Caps) -> bool {
    HOST_SANDBOX.with(|sandbox| {
        let sandbox = sandbox.borrow();
        sandbox.is_unrestricted() || sandbox.check(cap, "mcp/connect").is_ok()
    })
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

/// Plain, thread-safe form of a connect error. Every error produced after the
/// VM-thread capability gate is an eval error, optionally carrying user
/// guidance. Flatten it before returning from the blocking executor job so no
/// `SemaError` (and therefore no possible `Value`/`Rc`) crosses threads.
struct ConnectErrorPayload {
    message: String,
    hint: Option<String>,
    note: Option<String>,
}

impl ConnectErrorPayload {
    fn from_sema(error: SemaError) -> Self {
        let hint = error.hint().map(str::to_string);
        let note = error.note().map(str::to_string);
        let message = connect_error_message(error);
        Self {
            message,
            hint,
            note,
        }
    }

    fn into_sema(self) -> SemaError {
        let mut error = SemaError::eval(self.message);
        if let Some(hint) = self.hint {
            error = error.with_hint(hint);
        }
        if let Some(note) = self.note {
            error = error.with_note(note);
        }
        error
    }
}

fn connect_error_message(error: SemaError) -> String {
    match error {
        SemaError::Eval(message) => message,
        SemaError::WithContext { inner, .. } | SemaError::WithTrace { inner, .. } => {
            connect_error_message(*inner)
        }
        other => other.to_string(),
    }
}

enum ConnectPayloadResult {
    Connected {
        client: Box<McpClient>,
        identity: String,
    },
    NeedsAuth(String),
    Failed(ConnectErrorPayload),
    Cancelled,
}

struct McpConnectPayload(ConnectPayloadResult);

type McpCancelSignal = tokio::sync::oneshot::Sender<()>;
type McpCancelWaiter = tokio::sync::oneshot::Receiver<()>;

fn mcp_cancel_channel() -> (McpCancelSignal, McpCancelWaiter) {
    tokio::sync::oneshot::channel()
}

fn fire_cancel_signal(signal: &mut Option<McpCancelSignal>) {
    if let Some(signal) = signal.take() {
        // A send error means the worker completed and dropped its receiver
        // first. Otherwise the biased select is now armed to drop its transport
        // future before the hook reports the resource reaped.
        let _ = signal.send(());
    }
}

struct McpConnectCancelHook {
    signal: Option<McpCancelSignal>,
}

impl Trace for McpConnectCancelHook {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}

impl CancelHook for McpConnectCancelHook {
    fn cancel(&mut self) -> Result<CancelDisposition, CancelHookError> {
        fire_cancel_signal(&mut self.signal);
        Ok(CancelDisposition::Reaped)
    }

    fn reap(&mut self) -> Result<CancelDisposition, CancelHookError> {
        Ok(CancelDisposition::Reaped)
    }
}

/// Pass a direct External completion back to its native caller. The runtime
/// decodes successful completion to `Returned`; cancellation and failures keep
/// their public error behavior without retaining any Sema values while parked.
struct McpReturnContinuation {
    label: &'static str,
}

impl Trace for McpReturnContinuation {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}

impl NativeContinuation for McpReturnContinuation {
    fn resume(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        match input {
            ResumeInput::Returned(value) => Ok(NativeOutcome::Return(value)),
            ResumeInput::Failed(error) => Err(error),
            ResumeInput::Cancelled(reason) => Err(SemaError::eval(format!(
                "{} was cancelled ({reason:?})",
                self.label
            ))),
            ResumeInput::Runtime(_) => Err(SemaError::eval(format!(
                "{}: unexpected runtime response after offload",
                self.label
            ))),
        }
    }
}

const MCP_CONNECT_COMPLETION_KIND: u64 = 0x6d63_7032; // "mcp2"

struct McpConnectDecoder {
    opts: ConnectOpts,
}

impl Trace for McpConnectDecoder {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}

impl CompletionDecoder for McpConnectDecoder {
    fn decode(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        result: Result<SendPayload, ExternalFailure>,
    ) -> DecodedCompletion {
        let payload = result
            .map_err(|failure| SemaError::eval(format!("mcp/connect: {}", failure.message())))?;
        let McpConnectPayload(result) =
            downcast_send_payload::<McpConnectPayload>(payload, "mcp/connect")
                .map_err(|failure| SemaError::eval(failure.message()))?;
        match result {
            ConnectPayloadResult::Connected { client, identity } => {
                Ok(register_connection(*client, identity, &self.opts))
            }
            ConnectPayloadResult::NeedsAuth(url) => Err(connect_outcome_to_sema_error(
                ConnectOutcome::NeedsAuth(url),
            )),
            ConnectPayloadResult::Failed(error) => Err(error.into_sema()),
            ConnectPayloadResult::Cancelled => Err(SemaError::eval("mcp/connect was cancelled")),
        }
    }
}

fn connect_runtime_outcome(
    config_json: serde_json::Value,
    opts: ConnectOpts,
    browser_allowed: bool,
) -> NativeResult {
    let kind = CompletionKind::try_from_raw(MCP_CONNECT_COMPLETION_KIND)
        .expect("mcp/connect completion kind is nonzero");
    let decoder = Box::new(McpConnectDecoder { opts: opts.clone() });
    let (cancel_tx, cancel_rx) = mcp_cancel_channel();
    let resource = InterruptibleResource::new(
        "mcp/connect",
        Box::new(McpConnectCancelHook {
            signal: Some(cancel_tx),
        }),
    );
    let prepared =
        PreparedExternalOperation::interruptible_blocking(kind, decoder, resource, move || {
            let result = sema_io::io_block_on(async move {
                tokio::select! {
                    biased;
                    _ = cancel_rx => ConnectPayloadResult::Cancelled,
                    outcome = connect_dispatch_async(
                        config_json,
                        opts,
                        BrowserAuthority::from_allowed(browser_allowed),
                    ) => match outcome {
                        Ok((client, identity)) => ConnectPayloadResult::Connected {
                            client: Box::new(client),
                            identity,
                        },
                        Err(ConnectOutcome::NeedsAuth(url)) => {
                            ConnectPayloadResult::NeedsAuth(url)
                        }
                        Err(ConnectOutcome::Sema(error)) => ConnectPayloadResult::Failed(
                            ConnectErrorPayload::from_sema(error),
                        ),
                    },
                }
            });
            Ok(Box::new(McpConnectPayload(result)) as SendPayload)
        });
    Ok(NativeOutcome::Suspend(NativeSuspend {
        wait: WaitKind::External(Box::new(prepared)),
        continuation: Box::new(McpReturnContinuation {
            label: "mcp/connect",
        }),
    }))
}

fn connect_builtin(sandbox: &Sandbox, args: &[Value]) -> NativeResult {
    let config_json = config_to_json(args)?;
    let opts = ConnectOpts {
        interactive_auth: true,
        allowed_tools: None,
    };
    if in_runtime_quantum() {
        gate(sandbox, connect_capability(&config_json))?;
        return connect_runtime_outcome(config_json, opts, sandbox_allows_browser(sandbox));
    }
    connect_with_opts(
        sandbox,
        &config_json,
        &opts,
        BrowserAuthority::from_sandbox(sandbox),
    )
    .map(NativeOutcome::Return)
    .map_err(connect_outcome_to_sema_error)
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

fn remove_connection_entry(handle: &str, entry: &Rc<ConnEntry>) {
    CONNECTIONS.with(|connections| {
        let mut connections = connections.borrow_mut();
        if connections
            .get(handle)
            .is_some_and(|stored| Rc::ptr_eq(stored, entry))
        {
            connections.remove(handle);
        }
    });
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

/// Redirect driver used when the owning evaluator denies browser processes.
/// Authentication may still reuse a cached token or refresh silently; a full
/// login fails in `preflight()` before discovery or client registration.
struct SandboxDeniedDriver;

impl SandboxDeniedDriver {
    fn error() -> String {
        SemaError::PermissionDenied {
            function: "mcp/connect (open browser)".to_string(),
            capability: Caps::PROCESS.name().to_string(),
        }
        .to_string()
    }
}

impl crate::oauth::loopback::RedirectDriver for SandboxDeniedDriver {
    fn preflight(&self) -> Result<(), String> {
        Err(Self::error())
    }

    fn redirect_uri(&self) -> String {
        "http://127.0.0.1:1/callback".to_string()
    }

    fn drive(&self, _authorize_url: &str, _expected_state: &str) -> Result<String, String> {
        Err(Self::error())
    }
}

/// A [`RedirectDriver`](crate::oauth::loopback::RedirectDriver) for
/// non-interactive connections (`ConnectOpts::interactive_auth: false`).
/// `reauth_on_challenge`'s refresh-token path never enters full login — a valid
/// stored refresh token self-heals a `401` without any redirect — but its full
/// login fallback (no/expired refresh token, or a `403 insufficient_scope`
/// step-up, which always needs fresh consent) runs `preflight()`. Failing there
/// is what keeps a
/// non-interactive connection from ever popping a browser for the rest of its
/// lifetime.
struct NoInteractiveDriver;

const NO_INTERACTIVE_AUTH: &str = "interactive authentication is disabled for this connection";

impl crate::oauth::loopback::RedirectDriver for NoInteractiveDriver {
    fn preflight(&self) -> Result<(), String> {
        Err(NO_INTERACTIVE_AUTH.to_string())
    }

    fn redirect_uri(&self) -> String {
        // A well-formed fallback keeps the driver valid if a caller bypasses
        // `preflight()`; the non-interactive path never dials this URI.
        "http://127.0.0.1:1/callback".to_string()
    }

    fn drive(&self, _authorize_url: &str, _expected_state: &str) -> Result<String, String> {
        Err(NO_INTERACTIVE_AUTH.to_string())
    }
}

/// React to a mid-session auth challenge (refresh on `401`, step-up re-scope on
/// `403 insufficient_scope`) and return a fresh access token to retry with.
/// `interactive_auth` selects the redirect driver: the real loopback+browser
/// flow, or [`NoInteractiveDriver`] so a login fallback fails cleanly instead
/// of popping a browser mid-run. `browser_authority` is resolved before any
/// thread hop (see [`BrowserAuthority`]) and never touches `HOST_SANDBOX` when
/// offloaded. There is no other thread-local access under either driver.
async fn reauthorize_async(
    url: &str,
    status: Option<u16>,
    challenge: Option<&str>,
    interactive_auth: bool,
    browser_authority: BrowserAuthority,
) -> Result<Option<String>, String> {
    let http = reqwest::Client::new();
    let store = crate::oauth::store::default_store();
    let result = if interactive_auth {
        let driver = browser_authority
            .redirect_driver(Duration::from_secs(300))
            .map_err(|e| format!("mcp/call: {e}"))?;
        crate::oauth::login::reauth_on_challenge(
            &http,
            store.as_ref(),
            url,
            status,
            challenge,
            None,
            driver.as_ref(),
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
/// retry. No thread-local access: `interactive_auth`/`browser_authority` are
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
    browser_authority: BrowserAuthority,
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
        browser_authority,
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
    sandbox: &Sandbox,
    handle: &str,
    tool_name: &str,
    arguments_json: serde_json::Value,
    materialize: impl FnOnce(serde_json::Value) -> Result<Value, String> + 'static,
) -> NativeResult {
    let entry = lookup_entry(handle)?;
    check_tool_allowed(&entry.meta.allowed_tools, tool_name)?;
    let key = cassette_key(&entry.meta.identity, tool_name, &arguments_json);

    let cassette_recorder = match sema_core::mcp_cassette_decide(&key) {
        Some(sema_core::McpCassetteDecision::Replay(recorded)) => {
            return materialize(recorded)
                .map(NativeOutcome::Return)
                .map_err(|e| SemaError::eval(format!("mcp/call: {e}")));
        }
        Some(sema_core::McpCassetteDecision::Miss) => return Err(replay_miss_error()),
        Some(sema_core::McpCassetteDecision::Record(recorder)) => Some(recorder),
        None => None,
    };

    // A runtime quantum routes the blocking JSON-RPC round trip through the
    // thread-pool executor as an external wait (the same
    // `NativeOutcome::Suspend` mechanism `sleep` uses) so two `mcp/call`s to
    // DIFFERENT connections overlap on separate workers instead of serializing
    // on the VM thread.
    if in_runtime_quantum() {
        let interactive_auth = entry.meta.interactive_auth;
        let browser_allowed = sandbox_allows_browser(sandbox);
        return mcp_call_runtime_outcome(
            entry,
            cassette_recorder,
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
        BrowserAuthority::from_sandbox(sandbox),
    ));
    checkin(&entry, conn);
    let raw = result.map_err(|e| SemaError::eval(format!("mcp/call: {e}")))?;
    if let Some(recorder) = cassette_recorder {
        recorder.record(&raw);
    }
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
//   4. `Runtime(ReleaseResourceGate)` for a reusable connection, or
//      `Runtime(CloseResourceGate)` for terminal teardown — wake the FIFO head,
//      then deliver / raise.
//
// Calls to DIFFERENT connections overlap on separate workers (each has its own
// gate); calls to the SAME connection serialize through its one gate. A
// mid-flight cancel fires the worker's one-shot, tombstones the slot, removes
// the exact mapping, and closes the gate so every queued sibling wakes Closed.

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

/// Tears down and tombstones an in-flight `mcp/call`. The one-shot makes the
/// worker's biased select drop the transport future; the late completion is
/// discarded, so the `McpConnection` it owns drops off-thread and the slot must
/// never return to `Available`. The wait runtime invokes `cancel`/`reap` on the
/// VM thread, so holding an `Rc<ConnEntry>` is sound; a checked-out slot owns no
/// connection, so `trace` is trivially complete.
struct McpCallCancelHook {
    entry: Rc<ConnEntry>,
    signal: Option<McpCancelSignal>,
    lifecycle: Rc<McpGateLifecycle>,
}

impl Trace for McpCallCancelHook {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}

impl CancelHook for McpCallCancelHook {
    fn cancel(&mut self) -> Result<CancelDisposition, CancelHookError> {
        fire_cancel_signal(&mut self.signal);
        tombstone_slot(&self.entry, "cancelled mid-call");
        self.lifecycle.mark_terminal();
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
    cassette_recorder: Option<sema_core::McpCassetteRecorder>,
    materialize: Materialize,
    lifecycle: Rc<McpGateLifecycle>,
}

impl Trace for McpCallDecoder {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        // `entry` owns no checked-out connection, the recorder retains only a
        // host JSON tape, and `materialize` captures no `Value` (see
        // [`Materialize`]) — nothing to trace.
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
            cassette_recorder,
            materialize,
            lifecycle,
        } = *self;
        let payload = match result {
            Ok(payload) => payload,
            Err(failure) => {
                // Worker panic (or an undeliverable payload): the connection
                // dropped off-thread — tombstone so a later use fails cleanly.
                tombstone_slot(&entry, "mcp/call worker failed");
                lifecycle.mark_terminal();
                return Err(SemaError::eval(format!("mcp/call: {}", failure.message())));
            }
        };
        let McpCallPayload { conn, result } =
            match downcast_send_payload::<McpCallPayload>(payload, "mcp/call") {
                Ok(payload) => payload,
                Err(failure) => {
                    tombstone_slot(&entry, "mcp/call payload decode failed");
                    lifecycle.mark_terminal();
                    return Err(SemaError::eval(format!("mcp/call: {}", failure.message())));
                }
            };
        checkin(&entry, conn);
        let raw = result.map_err(|e| SemaError::eval(format!("mcp/call: {e}")))?;
        if let Some(recorder) = cassette_recorder {
            recorder.record(&raw);
        }
        materialize(raw).map_err(|e| SemaError::eval(format!("mcp/call: {e}")))
    }
}

trait McpGatedAction: Trace {
    fn label(&self) -> &'static str;
    fn terminal_if_cancelled_while_queued(&self) -> bool {
        false
    }
    fn prepare(
        self: Box<Self>,
        entry: Rc<ConnEntry>,
        conn: McpConnection,
        lifecycle: Rc<McpGateLifecycle>,
    ) -> PreparedExternalOperation;
}

/// Shared state for the existing connection gate lifecycle (create → acquire →
/// external → release). The boxed action contains only operation-specific plain
/// data and builds the External wait after the gate grants exclusive ownership.
struct McpGatedState {
    entry: Rc<ConnEntry>,
    action: Box<dyn McpGatedAction>,
}

/// Close-once state shared by the connection decoder, cancel hook, and final
/// continuation. Clearing the mapping compares the exact capability id so late
/// teardown cannot erase a replacement gate.
struct McpGateLifecycle {
    entry: Rc<ConnEntry>,
    gate: ResourceGateHandle,
    terminal: Cell<bool>,
}

impl McpGateLifecycle {
    fn new(entry: Rc<ConnEntry>, gate: ResourceGateHandle) -> Rc<Self> {
        Rc::new(Self {
            entry,
            gate,
            terminal: Cell::new(false),
        })
    }

    fn mark_terminal(&self) {
        if self.terminal.replace(true) {
            return;
        }
        let mut stored = self.entry.gate.borrow_mut();
        if stored
            .as_ref()
            .is_some_and(|handle| handle.id() == self.gate.id())
        {
            stored.take();
        }
    }

    fn finish_request(&self, continuation: Box<dyn NativeContinuation>) -> RuntimeRequest {
        if self.terminal.get() {
            RuntimeRequest::CloseResourceGate {
                gate: self.gate.id(),
                continuation,
            }
        } else {
            RuntimeRequest::ReleaseResourceGate {
                gate: self.gate.id(),
                continuation,
            }
        }
    }
}

impl Trace for McpGateLifecycle {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}

struct McpCallAction {
    cassette_recorder: Option<sema_core::McpCassetteRecorder>,
    tool_name: String,
    arguments_json: serde_json::Value,
    materialize: Materialize,
    interactive_auth: bool,
    browser_allowed: bool,
}

impl Trace for McpCallAction {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        // The recorder capability owns host cassette state only; `materialize`
        // is constrained by [`Materialize`] to avoid Sema graph captures.
        true
    }
}

impl McpGatedAction for McpCallAction {
    fn label(&self) -> &'static str {
        "mcp/call"
    }

    fn prepare(
        self: Box<Self>,
        entry: Rc<ConnEntry>,
        conn: McpConnection,
        lifecycle: Rc<McpGateLifecycle>,
    ) -> PreparedExternalOperation {
        let McpCallAction {
            cassette_recorder,
            tool_name,
            arguments_json,
            materialize,
            interactive_auth,
            browser_allowed,
        } = *self;
        let kind = CompletionKind::try_from_raw(MCP_CALL_COMPLETION_KIND)
            .expect("mcp/call completion kind is nonzero");
        let decoder = Box::new(McpCallDecoder {
            entry: entry.clone(),
            cassette_recorder,
            materialize,
            lifecycle: Rc::clone(&lifecycle),
        });
        let (cancel_tx, cancel_rx) = mcp_cancel_channel();
        let resource = InterruptibleResource::new(
            "mcp/call",
            Box::new(McpCallCancelHook {
                entry,
                signal: Some(cancel_tx),
                lifecycle,
            }),
        );
        PreparedExternalOperation::interruptible_blocking(kind, decoder, resource, move || {
            let mut conn = conn;
            let call = call_tool_async(
                &mut conn,
                &tool_name,
                arguments_json,
                interactive_auth,
                BrowserAuthority::from_allowed(browser_allowed),
            );
            let result = sema_io::io_block_on(async move {
                tokio::select! {
                    biased;
                    _ = cancel_rx => Err("cancelled".to_string()),
                    result = call => result,
                }
            });
            Ok(Box::new(McpCallPayload { conn, result }) as SendPayload)
        })
    }
}

/// Stage 0: a freshly-created gate arrives; store it on the connection entry,
/// then suspend on its slot. Holds no `Value`.
struct McpCreateGateCont {
    state: McpGatedState,
}

impl Trace for McpCreateGateCont {
    fn trace(&self, sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        self.state.action.trace(sink)
    }
}

impl NativeContinuation for McpCreateGateCont {
    fn resume(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        let state = self.state;
        let label = state.action.label();
        match input {
            ResumeInput::Runtime(RuntimeResponse::ResourceGate(handle)) => {
                let gate = handle.id();
                *state.entry.gate.borrow_mut() = Some(handle.clone());
                let lifecycle = McpGateLifecycle::new(Rc::clone(&state.entry), handle);
                Ok(NativeOutcome::Suspend(NativeSuspend {
                    wait: WaitKind::ResourceSlot(gate),
                    continuation: Box::new(McpAcquireCont { state, lifecycle }),
                }))
            }
            ResumeInput::Failed(error) => Err(error),
            ResumeInput::Cancelled(reason) => Err(SemaError::eval(format!(
                "{label} was cancelled before its connection gate was created ({reason:?})"
            ))),
            ResumeInput::Returned(_) | ResumeInput::Runtime(_) => Err(SemaError::eval(format!(
                "{label}: unexpected runtime response creating connection gate"
            ))),
        }
    }
}

/// Stage 1: the gate slot is granted; check the connection out and offload the
/// blocking JSON-RPC round trip as an External wait. A tombstoned/busy slot is
/// terminal, so it closes the gate and wakes every queued acquirer. Holds no
/// `Value`.
struct McpAcquireCont {
    state: McpGatedState,
    lifecycle: Rc<McpGateLifecycle>,
}

impl Trace for McpAcquireCont {
    fn trace(&self, sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        self.state.action.trace(sink)
    }
}

impl NativeContinuation for McpAcquireCont {
    fn resume(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        let McpAcquireCont { state, lifecycle } = *self;
        let label = state.action.label();
        match input {
            // Slot granted: we now own `gate`.
            ResumeInput::Runtime(RuntimeResponse::Value(_)) => {
                let McpGatedState { entry, action } = state;
                match try_checkout(&entry) {
                    Ok(Some(conn)) => {
                        let prepared = action.prepare(entry, conn, Rc::clone(&lifecycle));
                        Ok(NativeOutcome::Suspend(NativeSuspend {
                            wait: WaitKind::External(Box::new(prepared)),
                            continuation: Box::new(McpReleaseReturnCont { lifecycle, label }),
                        }))
                    }
                    // Slot tombstoned/missing (a prior cancel orphaned it): close
                    // the gate and wake every queued acquirer. `Ok(None)` (busy)
                    // is unreachable while we hold the gate, but is terminal too.
                    Ok(None) => {
                        lifecycle.mark_terminal();
                        Ok(NativeOutcome::Runtime(lifecycle.finish_request(Box::new(
                            McpFinalCont::Fail(SemaError::eval(format!(
                                "{label}: connection unexpectedly busy while holding its gate"
                            ))),
                        ))))
                    }
                    Err(error) => {
                        lifecycle.mark_terminal();
                        Ok(NativeOutcome::Runtime(
                            lifecycle.finish_request(Box::new(McpFinalCont::Fail(error))),
                        ))
                    }
                }
            }
            // Gate closed while we were queued: never owned it, just raise.
            ResumeInput::Failed(error) => Err(error),
            // Cancelled while queued: the runtime's ResourceSlot cancel arm already
            // removed us from the FIFO; we never owned the gate, so the connection
            // is untouched.
            ResumeInput::Cancelled(reason) => {
                let error = SemaError::eval(format!(
                    "{label} was cancelled while waiting for its connection ({reason:?})"
                ));
                if state.action.terminal_if_cancelled_while_queued() {
                    lifecycle.mark_terminal();
                    Ok(NativeOutcome::Runtime(
                        lifecycle.finish_request(Box::new(McpFinalCont::Fail(error))),
                    ))
                } else {
                    Err(error)
                }
            }
            ResumeInput::Returned(_) | ResumeInput::Runtime(_) => Err(SemaError::eval(format!(
                "{label}: unexpected runtime response acquiring connection"
            ))),
        }
    }
}

/// Stage 2: the blocking call completed / failed / was cancelled — release a
/// reusable gate or close a terminal one, then deliver the decoded value or
/// raise. The lifecycle is trace-trivial.
struct McpReleaseReturnCont {
    lifecycle: Rc<McpGateLifecycle>,
    label: &'static str,
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
        let cancelled = matches!(input, ResumeInput::Cancelled(_));
        let final_cont: Box<dyn NativeContinuation> = match input {
            ResumeInput::Returned(value) => Box::new(McpFinalCont::Value(value)),
            ResumeInput::Failed(error) => Box::new(McpFinalCont::Fail(error)),
            ResumeInput::Cancelled(reason) => Box::new(McpFinalCont::Fail(SemaError::eval(
                format!("{} was cancelled ({reason:?})", self.label),
            ))),
            ResumeInput::Runtime(_) => Box::new(McpFinalCont::Fail(SemaError::eval(format!(
                "{}: unexpected runtime response after offload",
                self.label
            )))),
        };
        if cancelled {
            self.lifecycle.mark_terminal();
        }
        Ok(NativeOutcome::Runtime(
            self.lifecycle.finish_request(final_cont),
        ))
    }
}

/// Stage 3: the gate transition completed; deliver the resolved outcome.
/// `McpFinalCont::Value` carries the decoded result across the runtime-request
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
        input: ResumeInput,
    ) -> NativeResult {
        match input {
            ResumeInput::Runtime(RuntimeResponse::Value(_)) => {}
            ResumeInput::Failed(error) => return Err(error),
            ResumeInput::Cancelled(reason) => {
                return Err(SemaError::eval(format!(
                    "MCP resource-gate transition was cancelled ({reason:?})"
                )))
            }
            ResumeInput::Returned(_) | ResumeInput::Runtime(_) => {
                return Err(SemaError::eval(
                    "MCP resource-gate transition returned an unexpected response",
                ))
            }
        }
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
/// an External wait once the gate is owned. Acquisition is event-driven; no
/// executor polling is involved.
fn mcp_call_runtime_outcome(
    entry: Rc<ConnEntry>,
    cassette_recorder: Option<sema_core::McpCassetteRecorder>,
    tool_name: String,
    arguments_json: serde_json::Value,
    materialize: Materialize,
    interactive_auth: bool,
    browser_allowed: bool,
) -> NativeResult {
    let action = McpCallAction {
        cassette_recorder,
        tool_name,
        arguments_json,
        materialize,
        interactive_auth,
        browser_allowed,
    };
    mcp_gated_runtime_outcome(entry, Box::new(action))
}

fn mcp_gated_runtime_outcome(
    entry: Rc<ConnEntry>,
    action: Box<dyn McpGatedAction>,
) -> NativeResult {
    let state = McpGatedState { entry, action };
    let gate = state.entry.gate.borrow().clone();
    match gate {
        Some(gate) => {
            let gate_id = gate.id();
            let lifecycle = McpGateLifecycle::new(Rc::clone(&state.entry), gate);
            Ok(NativeOutcome::Suspend(NativeSuspend {
                wait: WaitKind::ResourceSlot(gate_id),
                continuation: Box::new(McpAcquireCont { state, lifecycle }),
            }))
        }
        None => Ok(NativeOutcome::Runtime(RuntimeRequest::CreateResourceGate {
            continuation: Box::new(McpCreateGateCont { state }),
        })),
    }
}

// ── Shared unified-runtime path for `mcp/tools` and `mcp/close` ─────────────

const MCP_CONNECTION_OP_COMPLETION_KIND: u64 = 0x6d63_7033; // "mcp3"

/// VM-thread result shaping for `tools/list`. Implementations capture only
/// plain strings; no `Value` is retained across the External wait.
type ToolsMaterialize = Box<dyn FnOnce(Vec<Tool>, &ConnMeta) -> Value>;

enum McpConnectionOperation {
    ListTools,
    Close,
}

enum McpConnectionOperationResult {
    Tools(Result<Vec<Tool>, String>),
    Close(Result<(), String>),
}

struct McpConnectionOperationPayload {
    conn: McpConnection,
    result: McpConnectionOperationResult,
}

enum McpConnectionFinish {
    Tools {
        label: &'static str,
        materialize: ToolsMaterialize,
    },
    Close {
        handle: String,
    },
}

impl McpConnectionFinish {
    fn label(&self) -> &'static str {
        match self {
            Self::Tools { label, .. } => label,
            Self::Close { .. } => "mcp/close",
        }
    }
}

struct McpConnectionCancelHook {
    entry: Rc<ConnEntry>,
    label: &'static str,
    signal: Option<McpCancelSignal>,
    lifecycle: Rc<McpGateLifecycle>,
}

impl Trace for McpConnectionCancelHook {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}

impl CancelHook for McpConnectionCancelHook {
    fn cancel(&mut self) -> Result<CancelDisposition, CancelHookError> {
        fire_cancel_signal(&mut self.signal);
        tombstone_slot(&self.entry, format!("cancelled during {}", self.label));
        self.lifecycle.mark_terminal();
        Ok(CancelDisposition::Reaped)
    }

    fn reap(&mut self) -> Result<CancelDisposition, CancelHookError> {
        Ok(CancelDisposition::Reaped)
    }
}

struct McpConnectionDecoder {
    entry: Rc<ConnEntry>,
    finish: McpConnectionFinish,
    lifecycle: Rc<McpGateLifecycle>,
}

impl Trace for McpConnectionDecoder {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        // `entry` never owns a Value, and ToolsMaterialize captures only host
        // authority plus plain strings (the connection handle for tools->sema).
        true
    }
}

impl CompletionDecoder for McpConnectionDecoder {
    fn decode(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        result: Result<SendPayload, ExternalFailure>,
    ) -> DecodedCompletion {
        let McpConnectionDecoder {
            entry,
            finish,
            lifecycle,
        } = *self;
        let label = finish.label();
        let payload = match result {
            Ok(payload) => payload,
            Err(failure) => {
                tombstone_slot(&entry, format!("{label} worker failed"));
                lifecycle.mark_terminal();
                return Err(SemaError::eval(format!("{label}: {}", failure.message())));
            }
        };
        let McpConnectionOperationPayload { conn, result } =
            match downcast_send_payload::<McpConnectionOperationPayload>(payload, label) {
                Ok(payload) => payload,
                Err(failure) => {
                    tombstone_slot(&entry, format!("{label} payload decode failed"));
                    lifecycle.mark_terminal();
                    return Err(SemaError::eval(format!("{label}: {}", failure.message())));
                }
            };

        match (finish, result) {
            (
                McpConnectionFinish::Tools { label, materialize },
                McpConnectionOperationResult::Tools(result),
            ) => {
                checkin(&entry, conn);
                let tools = result.map_err(|error| SemaError::eval(format!("{label}: {error}")))?;
                Ok(materialize(tools, &entry.meta))
            }
            (
                McpConnectionFinish::Close { handle },
                McpConnectionOperationResult::Close(result),
            ) => {
                tombstone_slot(&entry, "connection closed");
                drop(conn);
                // The handle and transport are terminal even when the protocol
                // close itself reported an error.
                lifecycle.mark_terminal();
                remove_connection_entry(&handle, &entry);
                result.map_err(|error| SemaError::eval(format!("mcp/close: {error}")))?;
                Ok(Value::nil())
            }
            _ => {
                tombstone_slot(&entry, format!("{label} result kind mismatch"));
                lifecycle.mark_terminal();
                Err(SemaError::eval(format!(
                    "{label}: worker returned an unexpected operation result"
                )))
            }
        }
    }
}

struct McpConnectionAction {
    operation: McpConnectionOperation,
    finish: McpConnectionFinish,
}

impl Trace for McpConnectionAction {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}

impl McpGatedAction for McpConnectionAction {
    fn label(&self) -> &'static str {
        self.finish.label()
    }

    fn terminal_if_cancelled_while_queued(&self) -> bool {
        matches!(self.operation, McpConnectionOperation::Close)
    }

    fn prepare(
        self: Box<Self>,
        entry: Rc<ConnEntry>,
        conn: McpConnection,
        lifecycle: Rc<McpGateLifecycle>,
    ) -> PreparedExternalOperation {
        let McpConnectionAction { operation, finish } = *self;
        let label = finish.label();
        let kind = CompletionKind::try_from_raw(MCP_CONNECTION_OP_COMPLETION_KIND)
            .expect("MCP connection operation completion kind is nonzero");
        let decoder = Box::new(McpConnectionDecoder {
            entry: entry.clone(),
            finish,
            lifecycle: Rc::clone(&lifecycle),
        });
        let (cancel_tx, cancel_rx) = mcp_cancel_channel();
        let resource = InterruptibleResource::new(
            label,
            Box::new(McpConnectionCancelHook {
                entry,
                label,
                signal: Some(cancel_tx),
                lifecycle,
            }),
        );
        PreparedExternalOperation::interruptible_blocking(kind, decoder, resource, move || {
            let mut conn = conn;
            let result = match operation {
                McpConnectionOperation::ListTools => {
                    let list = list_tools_async(&mut conn);
                    McpConnectionOperationResult::Tools(sema_io::io_block_on(async move {
                        tokio::select! {
                            biased;
                            _ = cancel_rx => Err("cancelled".to_string()),
                            result = list => result,
                        }
                    }))
                }
                McpConnectionOperation::Close => {
                    let close = close_async(&mut conn);
                    McpConnectionOperationResult::Close(sema_io::io_block_on(async move {
                        tokio::select! {
                            biased;
                            _ = cancel_rx => Err("cancelled".to_string()),
                            result = close => result,
                        }
                    }))
                }
            };
            Ok(Box::new(McpConnectionOperationPayload { conn, result }) as SendPayload)
        })
    }
}

fn mcp_connection_runtime_outcome(
    entry: Rc<ConnEntry>,
    operation: McpConnectionOperation,
    finish: McpConnectionFinish,
) -> NativeResult {
    mcp_gated_runtime_outcome(entry, Box::new(McpConnectionAction { operation, finish }))
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
    sandbox: &Sandbox,
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
        let sandbox = sandbox.clone();
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
                &sandbox,
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
) -> NativeResult {
    let entry = lookup_entry(handle)?;
    if in_runtime_quantum() {
        return mcp_connection_runtime_outcome(
            entry,
            McpConnectionOperation::ListTools,
            McpConnectionFinish::Tools {
                label,
                materialize: Box::new(on_ready),
            },
        );
    }
    let mut conn = try_checkout(&entry)?.ok_or_else(|| busy_sync_error(handle, label))?;
    let result = block_on(list_tools_async(&mut conn));
    checkin(&entry, conn);
    let tools = result.map_err(|err| SemaError::eval(format!("{label}: {err}")))?;
    Ok(NativeOutcome::Return(on_ready(tools, &entry.meta)))
}

// ── `mcp/close` ─────────────────────────────────────────────────────────────

fn gate_belongs_to_current_runtime(gate: &ResourceGateHandle) -> bool {
    sema_core::current_root().is_some_and(|root| root.runtime() == gate.id().runtime())
}

fn close_gate_through_owner(gate: &ResourceGateHandle) -> Result<(), SemaError> {
    match gate.close() {
        Ok(_) | Err(ResourceGateCloseError::RuntimeUnavailable) => Ok(()),
        Err(error) => Err(SemaError::eval(format!(
            "mcp/close: could not close the connection gate through its owning runtime: {error}"
        ))),
    }
}

fn clear_entry_gate(entry: &ConnEntry, gate_id: ResourceGateId) {
    let mut stored = entry.gate.borrow_mut();
    if stored.as_ref().is_some_and(|gate| gate.id() == gate_id) {
        stored.take();
    }
}

struct McpForeignCloseDecoder {
    entry: Rc<ConnEntry>,
}

impl Trace for McpForeignCloseDecoder {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}

impl CompletionDecoder for McpForeignCloseDecoder {
    fn decode(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        result: Result<SendPayload, ExternalFailure>,
    ) -> DecodedCompletion {
        let payload = result.map_err(|failure| {
            tombstone_slot(&self.entry, "mcp/close terminal worker failed");
            SemaError::eval(format!("mcp/close: {}", failure.message()))
        })?;
        let McpConnectionOperationPayload { conn, result } = downcast_send_payload::<
            McpConnectionOperationPayload,
        >(payload, "mcp/close")
        .map_err(|failure| {
            tombstone_slot(&self.entry, "mcp/close terminal payload decode failed");
            SemaError::eval(format!("mcp/close: {}", failure.message()))
        })?;
        tombstone_slot(&self.entry, "connection closed");
        drop(conn);
        match result {
            McpConnectionOperationResult::Close(result) => {
                result.map_err(|error| SemaError::eval(format!("mcp/close: {error}")))?;
                Ok(Value::nil())
            }
            McpConnectionOperationResult::Tools(_) => Err(SemaError::eval(
                "mcp/close: terminal worker returned an unexpected result",
            )),
        }
    }
}

struct McpForeignCloseCancelHook {
    entry: Rc<ConnEntry>,
    signal: Option<McpCancelSignal>,
}

impl Trace for McpForeignCloseCancelHook {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}

impl CancelHook for McpForeignCloseCancelHook {
    fn cancel(&mut self) -> Result<CancelDisposition, CancelHookError> {
        fire_cancel_signal(&mut self.signal);
        tombstone_slot(&self.entry, "cancelled during mcp/close");
        Ok(CancelDisposition::Reaped)
    }

    fn reap(&mut self) -> Result<CancelDisposition, CancelHookError> {
        Ok(CancelDisposition::Reaped)
    }
}

fn foreign_runtime_close(
    handle: &str,
    entry: Rc<ConnEntry>,
    gate: ResourceGateHandle,
) -> NativeResult {
    let mut conn = try_checkout(&entry)?.ok_or_else(|| busy_sync_error(handle, "mcp/close"))?;
    if let Err(error) = close_gate_through_owner(&gate) {
        checkin(&entry, conn);
        return Err(error);
    }
    clear_entry_gate(&entry, gate.id());
    remove_connection_entry(handle, &entry);

    let (cancel_tx, cancel_rx) = mcp_cancel_channel();
    let decoder = Box::new(McpForeignCloseDecoder {
        entry: Rc::clone(&entry),
    });
    let resource = InterruptibleResource::new(
        "mcp/close",
        Box::new(McpForeignCloseCancelHook {
            entry,
            signal: Some(cancel_tx),
        }),
    );
    let prepared = PreparedExternalOperation::interruptible_blocking(
        CompletionKind::try_from_raw(MCP_CONNECTION_OP_COMPLETION_KIND)
            .expect("MCP connection operation completion kind is nonzero"),
        decoder,
        resource,
        move || {
            let close = close_async(&mut conn);
            let result = sema_io::io_block_on(async move {
                tokio::select! {
                    biased;
                    _ = cancel_rx => Err("cancelled".to_string()),
                    result = close => result,
                }
            });
            Ok(Box::new(McpConnectionOperationPayload {
                conn,
                result: McpConnectionOperationResult::Close(result),
            }) as SendPayload)
        },
    );
    Ok(NativeOutcome::Suspend(NativeSuspend {
        wait: WaitKind::External(Box::new(prepared)),
        continuation: Box::new(McpReturnContinuation { label: "mcp/close" }),
    }))
}

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
    let entry = CONNECTIONS.with(|connections| connections.borrow().get(handle_str).cloned());
    let Some(entry) = entry else {
        return;
    };
    let mut conn = match try_checkout(&entry) {
        Ok(Some(conn)) => Some(conn),
        Ok(None) => return,
        Err(_) => None,
    };
    let gate = entry.gate.borrow().clone();
    if let Some(gate) = gate.as_ref() {
        if let Err(error) = close_gate_through_owner(gate) {
            if let Some(conn) = conn.take() {
                checkin(&entry, conn);
            }
            eprintln!("mcp close_handle could not close its live runtime resource gate: {error}");
            return;
        }
        clear_entry_gate(&entry, gate.id());
    }
    remove_connection_entry(handle_str, &entry);
    if let Some(conn) = conn {
        close_conn_best_effort(conn);
    }
}

/// Best-effort transport close of an owned, already-deregistered connection.
/// Inside a runtime quantum (workflow teardown closes `:mcp` handles from
/// `finish_run`, which runs during a native callback), the synchronous
/// `io_block_on` would hit its active-quantum guard AND block the cooperative
/// scheduler — so fire-and-forget the async close on a background blocking worker
/// (`McpConnection` is `Send`; the connection is already unreachable, so nothing
/// waits on it). On a host/plain thread, close synchronously as before.
fn close_conn_best_effort(conn: McpConnection) {
    if in_runtime_quantum() {
        spawn_background_close(conn);
    } else {
        let mut conn = conn;
        let _ = block_on(close_async(&mut conn));
    }
}

/// Fire-and-forget the async transport close on a background blocking worker,
/// where `io_block_on` is legal (no active runtime quantum on that thread). Kept
/// out of `close_conn_best_effort`'s `in_runtime_quantum()` branch on purpose:
/// the `io_block_on` lives on the worker, not the VM thread, so it must not sit
/// textually inside an active-runtime branch (the source-policy guard rejects
/// that shape).
fn spawn_background_close(conn: McpConnection) {
    sema_io::io_spawn_blocking(move || {
        let mut conn = conn;
        let _ = sema_io::io_block_on(close_async(&mut conn));
    });
}

fn close_connection(args: &[Value]) -> NativeResult {
    check_arity!(args, "mcp/close", 1);
    let handle = require_handle(args, "mcp/close")?;
    let entry = lookup_entry(handle)?;
    if in_runtime_quantum() {
        let gate = entry.gate.borrow().clone();
        if let Some(gate) = gate {
            if !gate_belongs_to_current_runtime(&gate) {
                return foreign_runtime_close(handle, entry, gate);
            }
        }
        return mcp_connection_runtime_outcome(
            entry,
            McpConnectionOperation::Close,
            McpConnectionFinish::Close {
                handle: handle.to_string(),
            },
        );
    }
    let mut conn = try_checkout(&entry)?.ok_or_else(|| busy_sync_error(handle, "mcp/close"))?;
    let gate = entry.gate.borrow().clone();
    if let Some(gate) = gate.as_ref() {
        if let Err(error) = close_gate_through_owner(gate) {
            checkin(&entry, conn);
            return Err(error);
        }
        clear_entry_gate(&entry, gate.id());
    }
    remove_connection_entry(handle, &entry);
    let result = block_on(close_async(&mut conn));
    result.map_err(|err| SemaError::eval(format!("mcp/close: {err}")))?;
    Ok(NativeOutcome::Return(Value::nil()))
}

pub fn register_mcp_builtins(env: &Env, sandbox: &Sandbox) {
    // Host helpers without an evaluator context retain their established
    // latest-registration behavior. Every Sema-callable authority check below
    // captures `sandbox` in its own native instead of consulting this slot.
    HOST_SANDBOX.with(|slot| *slot.borrow_mut() = sandbox.clone());

    // `mcp/connect` picks its transport from the config map at runtime, so the
    // capability it needs is not fixed: a `:url` server is network I/O
    // (`NETWORK`), a `:command` server spawns a process (`PROCESS`). Gating and
    // dispatch live in the shared connect helpers used by
    // `connect_from_config`; this just supplies the interactive, unrestricted
    // options `mcp/connect` has always used.
    let connect_sandbox = sandbox.clone();
    env.set_str(
        "mcp/connect",
        Value::native_fn(dual_native("mcp/connect".to_string(), move |args| {
            connect_builtin(&connect_sandbox, args)
        })),
    );

    env.set_str(
        "mcp/tools",
        Value::native_fn(dual_native("mcp/tools".to_string(), |args| {
            check_arity!(args, "mcp/tools", 1);
            let handle = require_handle(args, "mcp/tools")?;
            fetch_tools(handle, "mcp/tools", |tools, meta| {
                tools_to_value(tools, &meta.allowed_tools)
            })
        })),
    );

    let tools_sandbox = sandbox.clone();
    env.set_str(
        "mcp/tools->sema",
        Value::native_fn(dual_native("mcp/tools->sema".to_string(), move |args| {
            check_arity!(args, "mcp/tools->sema", 1);
            let handle = require_handle(args, "mcp/tools->sema")?;
            let connection_handle = handle.to_string();
            let sandbox = tools_sandbox.clone();
            fetch_tools(handle, "mcp/tools->sema", move |tools, meta| {
                tool_defs_to_value(tools, &meta.allowed_tools, &connection_handle, &sandbox)
            })
        })),
    );

    let call_sandbox = sandbox.clone();
    env.set_str(
        "mcp/call",
        Value::native_fn(dual_native("mcp/call".to_string(), move |args| {
            check_arity!(args, "mcp/call", 3);
            let handle = require_handle(args, "mcp/call")?;
            let tool_name = args[1].as_str().ok_or_else(|| {
                SemaError::type_error("string", args[1].type_name())
                    .with_hint("mcp/call expects the tool name as a string")
            })?;
            let arguments_json = sema_core::value_to_json_lossy(&args[2]);
            call_tool(&call_sandbox, handle, tool_name, arguments_json, |raw| {
                Ok(result_to_value(&raw))
            })
        })),
    );

    env.set_str(
        "mcp/close",
        Value::native_fn(dual_native("mcp/close".to_string(), close_connection)),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oauth::loopback::RedirectDriver;
    use sema_core::runtime::{
        CompletionDelivery, CompletionRegistrar, CompletionSender, ExternalCompletion,
        RuntimeScopedIdCounter,
    };
    use std::sync::Arc;

    struct ClosedInbox;

    impl CompletionSender for ClosedInbox {
        fn send(&self, _: ExternalCompletion) -> CompletionDelivery {
            CompletionDelivery::InboxClosed
        }
    }

    fn gate_id() -> ResourceGateId {
        let (runtime, _registrar, _issuers) =
            CompletionRegistrar::register(Arc::new(ClosedInbox)).unwrap();
        RuntimeScopedIdCounter::new(runtime).allocate().unwrap()
    }

    fn gate_handle(id: ResourceGateId) -> ResourceGateHandle {
        ResourceGateHandle::new(id, Rc::new(|_| Ok(true)))
    }

    #[test]
    fn no_interactive_driver_never_dials_and_fails_cleanly() {
        let driver = NoInteractiveDriver;
        // The defense-only fallback remains a well-formed loopback URI.
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
    fn denied_redirect_preflight_runs_before_oauth_discovery() {
        let config = crate::oauth::login::LoginConfig {
            mcp_url: "http://127.0.0.1:1/mcp",
            resource_metadata_url: None,
            requested_scope: None,
            preconfigured_client_id: None,
        };
        let error = sema_io::io_block_on(crate::oauth::login::login(
            &reqwest::Client::new(),
            &config,
            None,
            &SandboxDeniedDriver,
        ))
        .expect_err("sandbox denial must reject before network discovery");

        assert!(error.contains("Permission denied"), "{error}");
        assert!(error.contains("process"), "{error}");
    }

    #[test]
    fn denied_browser_authority_fails_without_loopback_timeout() {
        let driver = BrowserAuthority::from_sandbox(&Sandbox::deny(Caps::PROCESS))
            .redirect_driver(Duration::from_secs(300))
            .expect("denied authority uses a non-binding redirect driver");
        let started = std::time::Instant::now();
        let error = driver
            .drive("https://example.com/authorize", "state")
            .expect_err("denied browser authority must refuse interactive consent");

        assert!(error.contains("Permission denied"), "{error}");
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "denied authority waited for a loopback timeout: {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn gated_host_opener_captures_authority_at_construction() {
        HOST_SANDBOX.with(|slot| *slot.borrow_mut() = Sandbox::deny(Caps::PROCESS));
        let opener = gated_browser_opener();
        HOST_SANDBOX.with(|slot| *slot.borrow_mut() = Sandbox::allow_all());

        let error = opener("https://example.com/authorize")
            .expect_err("later host registration must not widen a constructed opener");
        assert!(error.contains("Permission denied"), "{error}");
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
        _assert_send::<McpConnectPayload>();
        _assert_send::<McpConnectionOperationPayload>();
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
            gate: RefCell::new(None),
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

    fn checked_out_entry() -> Rc<ConnEntry> {
        Rc::new(ConnEntry {
            meta: ConnMeta {
                identity: "trace-test".to_string(),
                interactive_auth: false,
                allowed_tools: None,
            },
            slot: RefCell::new(Slot::CheckedOut),
            gate: RefCell::new(None),
        })
    }

    fn tools_finish() -> McpConnectionFinish {
        McpConnectionFinish::Tools {
            label: "mcp/tools",
            materialize: Box::new(|tools, meta| tools_to_value(tools, &meta.allowed_tools)),
        }
    }

    #[test]
    fn mcp_runtime_connection_continuations_trace_exact_value_edges() {
        let connect = McpReturnContinuation {
            label: "mcp/connect",
        };
        let mut edges = 0;
        assert!(connect.trace(&mut |_| edges += 1));
        assert_eq!(edges, 0);

        let create = McpCreateGateCont {
            state: McpGatedState {
                entry: checked_out_entry(),
                action: Box::new(McpConnectionAction {
                    operation: McpConnectionOperation::ListTools,
                    finish: tools_finish(),
                }),
            },
        };
        let mut edges = 0;
        assert!(create.trace(&mut |_| edges += 1));
        assert_eq!(edges, 0);

        let decoder_entry = checked_out_entry();
        let decoder = McpConnectionDecoder {
            entry: Rc::clone(&decoder_entry),
            finish: tools_finish(),
            lifecycle: McpGateLifecycle::new(decoder_entry, gate_handle(gate_id())),
        };
        let mut edges = 0;
        assert!(decoder.trace(&mut |_| edges += 1));
        assert_eq!(edges, 0);

        let final_value = McpFinalCont::Value(Value::string("kept"));
        let mut edges = 0;
        assert!(final_value.trace(&mut |_| edges += 1));
        assert_eq!(edges, 1);

        let final_error = McpFinalCont::Fail(SemaError::UserException(Value::string("kept")));
        let mut edges = 0;
        assert!(final_error.trace(&mut |_| edges += 1));
        assert_eq!(edges, 1);
    }

    #[test]
    fn mcp_final_cont_does_not_swallow_runtime_transition_failure() {
        let eval_context = sema_core::EvalContext::new();
        let task_context = sema_core::runtime::TaskContextHandle::default();
        let mut context = NativeCallContext {
            eval_context: &eval_context,
            task_context,
            call_env: None,
            cancellation: sema_core::runtime::CancellationView::default(),
        };
        let error = match Box::new(McpFinalCont::Value(Value::nil())).resume(
            &mut context,
            ResumeInput::Failed(SemaError::eval("wrong runtime close")),
        ) {
            Err(error) => error,
            Ok(_) => panic!("failed MCP gate transition must override stored success"),
        };
        assert!(error.to_string().contains("wrong runtime close"), "{error}");
    }

    #[test]
    fn mcp_worker_loss_clears_mapping_and_closes_gate() {
        let entry = checked_out_entry();
        let gate = gate_id();
        let handle = gate_handle(gate);
        *entry.gate.borrow_mut() = Some(handle.clone());
        let lifecycle = McpGateLifecycle::new(Rc::clone(&entry), handle);
        let decoder = McpConnectionDecoder {
            entry: Rc::clone(&entry),
            finish: tools_finish(),
            lifecycle: Rc::clone(&lifecycle),
        };
        let eval_context = sema_core::EvalContext::new();
        let task_context = sema_core::runtime::TaskContextHandle::default();
        let mut context = NativeCallContext {
            eval_context: &eval_context,
            task_context,
            call_env: None,
            cancellation: sema_core::runtime::CancellationView::default(),
        };
        assert!(Box::new(decoder)
            .decode(&mut context, Err(ExternalFailure::rejected()))
            .is_err());
        assert!(lifecycle.terminal.get());
        assert!(entry.gate.borrow().is_none());

        let outcome = Box::new(McpReleaseReturnCont {
            lifecycle: Rc::clone(&lifecycle),
            label: "mcp/tools",
        })
        .resume(
            &mut context,
            ResumeInput::Failed(SemaError::eval("worker failed")),
        )
        .unwrap();
        let NativeOutcome::Runtime(RuntimeRequest::CloseResourceGate { gate: closed, .. }) =
            outcome
        else {
            panic!("MCP worker loss must close its terminal gate")
        };
        assert_eq!(closed, gate);
    }

    #[test]
    fn queued_close_cancellation_is_terminal_but_queued_tools_cancellation_is_not() {
        fn entry_with_gate(gate: ResourceGateId) -> Rc<ConnEntry> {
            let entry = checked_out_entry();
            *entry.gate.borrow_mut() = Some(gate_handle(gate));
            entry
        }

        let eval_context = sema_core::EvalContext::new();
        let task_context = sema_core::runtime::TaskContextHandle::default();
        let mut context = NativeCallContext {
            eval_context: &eval_context,
            task_context,
            call_env: None,
            cancellation: sema_core::runtime::CancellationView::default(),
        };

        let close_gate = gate_id();
        let close_entry = entry_with_gate(close_gate);
        let close_lifecycle = McpGateLifecycle::new(
            Rc::clone(&close_entry),
            close_entry.gate.borrow().as_ref().unwrap().clone(),
        );
        let close = McpAcquireCont {
            state: McpGatedState {
                entry: Rc::clone(&close_entry),
                action: Box::new(McpConnectionAction {
                    operation: McpConnectionOperation::Close,
                    finish: McpConnectionFinish::Close {
                        handle: "test-close".to_string(),
                    },
                }),
            },
            lifecycle: close_lifecycle,
        };
        let outcome = Box::new(close)
            .resume(
                &mut context,
                ResumeInput::Cancelled(sema_core::runtime::CancelReason::Explicit),
            )
            .unwrap();
        assert!(close_entry.gate.borrow().is_none());
        assert!(matches!(
            outcome,
            NativeOutcome::Runtime(RuntimeRequest::CloseResourceGate { gate, .. })
                if gate == close_gate
        ));

        let tools_gate = gate_id();
        let tools_entry = entry_with_gate(tools_gate);
        let tools_lifecycle = McpGateLifecycle::new(
            Rc::clone(&tools_entry),
            tools_entry.gate.borrow().as_ref().unwrap().clone(),
        );
        let tools = McpAcquireCont {
            state: McpGatedState {
                entry: Rc::clone(&tools_entry),
                action: Box::new(McpConnectionAction {
                    operation: McpConnectionOperation::ListTools,
                    finish: tools_finish(),
                }),
            },
            lifecycle: tools_lifecycle,
        };
        assert!(Box::new(tools)
            .resume(
                &mut context,
                ResumeInput::Cancelled(sema_core::runtime::CancelReason::Explicit),
            )
            .is_err());
        assert_eq!(
            tools_entry
                .gate
                .borrow()
                .as_ref()
                .map(ResourceGateHandle::id),
            Some(tools_gate),
            "queued cancellation alone must retain the usable connection gate"
        );
    }
}
