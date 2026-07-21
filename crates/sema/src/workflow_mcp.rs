//! The runtime [`WorkflowMcpResolver`] binding: resolves a workflow's declared
//! `:mcp` servers to live, authenticated connections over `sema-mcp`, without ever
//! popping a browser mid-run (the headless precursor — see
//! `docs/plans/2026-06-24-workflow-mcp-auth.md` §3). This is the ONLY place in the
//! binary crate that bridges `sema-stdlib`'s MCP-ignorant [`McpDecl`] to
//! `sema-mcp`'s connect/OAuth machinery — the crate-dependency law (AGENTS.md)
//! forbids `sema-stdlib`/`sema-workflow` from depending on `sema-mcp` directly, so
//! all of that knowledge lives here, behind the resolver trait `workflow/run`
//! calls through.
//!
//! Register via [`register_real_resolver`] alongside `sema_mcp::register_mcp_builtins`
//! so every path that can run a workflow (REPL, `sema run`, `sema workflow run`,
//! and any embedder that opts in) gets it.

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use sema_core::cycle::GcEdge;
use sema_core::runtime::{
    downcast_send_payload, CancelDisposition, CancelHook, CancelHookError, CompletionDecoder,
    CompletionKind, DecodedCompletion, ExternalFailure, InterruptibleResource, NativeCallContext,
    PreparedExternalOperation, SendPayload, Trace,
};
use sema_core::{Caps, SemaError, Value};
use sema_mcp::oauth::login::{self, LoginConfig};
use sema_mcp::oauth::loopback::BrowserOpener;
use sema_mcp::oauth::scoped::{store_encryption_key, MemoryStore, ScopedFileStore};
use sema_mcp::oauth::store::{self, TokenSet, TokenStore};
use sema_mcp::{
    browser_open_allowed, close_handle, connect_from_config, connect_send, gated_browser_opener,
    host_capability_allowed, login_interactive, register_connected, ConnectFailure, ConnectOpts,
    ConnectedClient,
};
use sema_stdlib::workflow_mcp::{
    self, AuthGrant, McpAuthDecl, McpDecl, McpPersist, McpSpecDecl, ServerResolution,
    WorkflowMcpResolver,
};

/// Clock skew (seconds) a cached token is treated as expired before its nominal
/// expiry — mirrors `oauth::login`'s own `EXPIRY_SKEW_SECS` (private to that
/// module), reused here via the public [`TokenSet::is_expired`].
const EXPIRY_SKEW_SECS: u64 = 60;

/// The test/embedder browser-opener override — see [`set_interactive_login_opener`].
type TestOpenerFn = fn(&str) -> Result<(), String>;

thread_local! {
    // See `set_interactive_auth`. Defaults to `false` — a fresh thread/process
    // (REPL, `sema run`, an embedder that never calls `set_interactive_auth`)
    // gets exactly today's headless behavior.
    static INTERACTIVE_AUTH: Cell<bool> = const { Cell::new(false) };
    // TEST/embedder seam: see `set_interactive_login_opener`.
    static TEST_LOGIN_OPENER: Cell<Option<TestOpenerFn>> = const { Cell::new(None) };
}

/// Enable (or disable) inline interactive MCP auth at the run-start resolution
/// gate: when `true`, a declared server this process finds no usable session
/// for runs the SAME browser/loopback login `sema mcp login` performs, right
/// where the headless precursor would otherwise gate with
/// `{:status :needs-auth}` (plan `docs/plans/2026-06-24-workflow-mcp-auth.md`
/// §3). `run_workflow_command` (`crates/sema/src/main.rs`) is the only caller
/// in the binary; it enables this iff stdin AND stderr are TTYs, `CI` is
/// unset/empty, and `--no-auth-prompt` was not passed.
///
/// Resolution happens before any workflow phase runs, so a browser prompt here
/// has no task to park or resume. A mid-run re-authentication request (for
/// example, after token revocation) is outside this run-start gate's scope.
///
/// Thread-local because a Sema interpreter (and thus a workflow run) is
/// single-threaded.
pub fn set_interactive_auth(enabled: bool) {
    INTERACTIVE_AUTH.with(|c| c.set(enabled));
}

fn interactive_auth_enabled() -> bool {
    INTERACTIVE_AUTH.with(|c| c.get())
}

/// TEST/embedder seam: override the browser opener the interactive run-start
/// auth path uses, so a test can drive the OAuth redirect programmatically
/// (the `crates/sema-mcp/tests/mcp_oauth_test.rs` loopback-driving pattern —
/// a blocking GET that follows the authorization server's redirect to the
/// loopback listener) instead of popping a real browser. `None` (the default)
/// reverts to the real, sandbox-gated browser opener
/// (`sema_mcp::gated_browser_opener`).
///
/// A plain `fn` pointer, not a capturing closure: `LoopbackDriver::drive` runs
/// the opener on a thread IT spawns, and thread-locals never propagate to a
/// spawned thread, so the opener to use must be resolved and boxed on the
/// calling thread before handoff — trivial for a `Copy` fn pointer, impossible
/// for a thread-local lookup performed from inside the opener itself.
pub fn set_interactive_login_opener(opener: Option<TestOpenerFn>) {
    TEST_LOGIN_OPENER.with(|c| c.set(opener));
}

/// The opener to hand to [`login_interactive`]: the test override if one is
/// set, else the real sandbox-gated browser opener. Resolved on the calling
/// (evaluator) thread — see [`set_interactive_login_opener`].
fn interactive_login_opener() -> BrowserOpener {
    match TEST_LOGIN_OPENER.with(|c| c.get()) {
        Some(f) => Box::new(f),
        None => gated_browser_opener(),
    }
}

/// Install the real, `sema-mcp`-backed resolver as the process' active
/// [`WorkflowMcpResolver`]. Idempotent (replacing is cheap and side-effect-free).
pub fn register_real_resolver() {
    workflow_mcp::set_workflow_mcp_resolver(Rc::new(RealResolver));
}

struct RealResolver;

impl WorkflowMcpResolver for RealResolver {
    fn resolve(&self, decls: &[McpDecl], workflow: &str, run_id: &str) -> Vec<ServerResolution> {
        decls
            .iter()
            .map(|decl| resolve_one(decl, workflow, run_id))
            .collect()
    }

    fn resolve_prepared(
        &self,
        decls: &[McpDecl],
        workflow: &str,
        run_id: &str,
    ) -> Box<PreparedExternalOperation> {
        // Capture the VM-thread authority (sandbox capabilities, interactive-auth
        // config, browser opener) BEFORE offloading — the worker's own
        // sandbox/opener thread-locals are a separate, default set.
        let cfg = ResolveConfig::capture();
        let decls = decls.to_vec();
        let workflow = workflow.to_string();
        let run_id = run_id.to_string();

        let kind = CompletionKind::try_from_raw(WORKFLOW_MCP_RESOLVE_KIND)
            .expect("workflow mcp resolve completion kind is nonzero");
        let resource = InterruptibleResource::new("workflow/mcp-resolve", Box::new(ResolveCancelHook));
        let decoder = Box::new(ResolveDecoder);

        // The job is a SYNCHRONOUS blocking closure (not one `io_block_on`'d
        // future), for a hard reason: the resolve calls `login_interactive`, which
        // spins its OWN current-thread tokio runtime and `block_on`s it. Nesting
        // that inside a single `io_block_on(async { … })` would panic ("cannot
        // start a runtime from within a runtime"). Run sequentially instead: each
        // connect/refresh enters+exits `io_block_on` on its own, and
        // `login_interactive` runs BETWEEN them, on this plain blocking worker
        // where no runtime is being driven — exactly like the host path does on
        // the VM thread, just off the quantum. Cancellation is therefore
        // best-effort (the sibling of `checkout_external`'s blocking tier): a
        // mid-flight resolve is not interrupted, but its completion is discarded
        // on cancel, dropping every `ConnectedClient` in the payload and so
        // closing any connection it had opened — no handle registered, no leak.
        let prepared =
            PreparedExternalOperation::interruptible_blocking(kind, decoder, resource, move || {
                let outs = resolve_all_send(&decls, &workflow, &run_id, &cfg);
                Ok(Box::new(outs) as SendPayload)
            });
        Box::new(prepared)
    }

    fn close(&self, handles: &[Value]) {
        for handle in handles {
            close_handle(handle);
        }
    }
}

/// Completion tag for the workflow `:mcp` resolve offload (`"wfr1"`).
const WORKFLOW_MCP_RESOLVE_KIND: u64 = 0x7766_7231;

/// A browser opener shared across the VM-thread/worker boundary — `Send + Sync`
/// so it moves into the offloaded resolve job, `Fn` (not `FnOnce`) so it can be
/// re-wrapped fresh per interactive attempt.
type SharedOpener = Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>;

/// VM-thread authority + interactive config captured before the resolve offload,
/// so the worker resolves with the CALLER's sandbox/opener rather than its own
/// (default, unrestricted) thread-locals. `Send` (only bools + a `Send + Sync`
/// opener `Arc`), so it moves into the blocking job.
struct ResolveConfig {
    interactive_auth: bool,
    browser_allowed: bool,
    network_allowed: bool,
    process_allowed: bool,
    /// Shared browser opener captured on the VM thread — wrapped fresh into a
    /// `BrowserOpener` per interactive attempt. `None` when interactive auth is off.
    opener: Option<SharedOpener>,
}

impl ResolveConfig {
    fn capture() -> Self {
        let interactive_auth = interactive_auth_enabled();
        let opener: Option<SharedOpener> = if interactive_auth {
            // `interactive_login_opener()` reads the TEST/embedder opener seam on
            // this (VM) thread; re-share it as an `Arc` the worker can wrap once
            // per interactive attempt.
            Some(Arc::from(interactive_login_opener()))
        } else {
            None
        };
        Self {
            interactive_auth,
            browser_allowed: browser_open_allowed(),
            network_allowed: host_capability_allowed(Caps::NETWORK),
            process_allowed: host_capability_allowed(Caps::PROCESS),
            opener,
        }
    }

    /// A fresh `BrowserOpener` delegating to the shared opener `Arc` — `None` when
    /// interactive auth is off. Built per interactive attempt (`login_interactive`
    /// consumes the opener it is handed).
    fn make_opener(&self) -> Option<BrowserOpener> {
        self.opener.as_ref().map(|arc| {
            let arc = Arc::clone(arc);
            Box::new(move |url: &str| arc(url)) as BrowserOpener
        })
    }
}

/// `Send` per-server outcome the offloaded resolve job hands back; the VM-thread
/// decoder ([`ResolveDecoder`]) turns each into a `ServerResolution`, minting the
/// opaque handle `Value` for `Connected` only there. Carries no `Value`/`Rc` — a
/// `Connected` holds the live `Send` `ConnectedClient` until registration.
enum SendResolution {
    Connected {
        alias: String,
        // Boxed: `ConnectedClient` wraps a whole `McpClient` (reqwest client +
        // transport), which would otherwise make this variant dwarf the others.
        connected: Box<ConnectedClient>,
        opts: ConnectOpts,
        auth: Option<AuthGrant>,
    },
    NeedsAuth {
        alias: String,
        url: String,
        scopes: Vec<String>,
        tools: Vec<String>,
        persist: String,
    },
    Failed {
        alias: String,
        reason: String,
    },
}

/// The off-quantum twin of [`RealResolver::resolve`]: resolves every declared
/// server on the blocking worker, producing `Send` outcomes (deferring the
/// `CONNECTIONS`-thread-local registration + handle `Value` to the VM-thread
/// decoder). Mirrors the synchronous `resolve_one` state machine step for step;
/// the two are kept in lockstep deliberately (the host path stays byte-identical
/// while the runtime path runs off the quantum).
fn resolve_all_send(
    decls: &[McpDecl],
    workflow: &str,
    run_id: &str,
    cfg: &ResolveConfig,
) -> Vec<SendResolution> {
    decls
        .iter()
        .map(|decl| resolve_one_send(decl, workflow, run_id, cfg))
        .collect()
}

fn resolve_one_send(
    decl: &McpDecl,
    workflow: &str,
    run_id: &str,
    cfg: &ResolveConfig,
) -> SendResolution {
    match (&decl.spec, &decl.auth) {
        (McpSpecDecl::Stdio { .. }, _) | (McpSpecDecl::Http { .. }, None) => {
            connect_plain_send(decl, workflow, run_id, cfg)
        }
        (McpSpecDecl::Http { .. }, Some(auth)) => {
            resolve_authenticated_http_send(decl, auth, workflow, run_id, cfg)
        }
    }
}

/// The VM-captured connect capability gate (`Caps::NETWORK` for `:url`,
/// `Caps::PROCESS` for `:command`), applied on the worker via the pre-resolved
/// booleans — `connect_send` itself never reads `HOST_SANDBOX`. `Some(reason)`
/// means denied (mirrors `connect_from_config`'s gate → `ConnectFailure::Failed`).
fn connect_capability_denied(decl: &McpDecl, cfg: &ResolveConfig) -> Option<String> {
    let (allowed, cap) = match &decl.spec {
        McpSpecDecl::Http { .. } => (cfg.network_allowed, Caps::NETWORK),
        McpSpecDecl::Stdio { .. } => (cfg.process_allowed, Caps::PROCESS),
    };
    if allowed {
        None
    } else {
        Some(format!("mcp/connect: permission denied for {}", cap.name()))
    }
}

/// Drive `connect_send` on the worker (its `io_block_on` is legal here), after
/// the VM-captured capability gate. `bearer` injects an `Authorization` header.
/// The `Send` client comes back for the decoder to register on the VM thread.
fn connect_send_now(
    decl: &McpDecl,
    bearer: Option<&str>,
    cfg: &ResolveConfig,
) -> Result<(Box<ConnectedClient>, ConnectOpts), ConnectFailure> {
    if let Some(reason) = connect_capability_denied(decl, cfg) {
        return Err(ConnectFailure::Failed(reason));
    }
    let cfg_value = spec_config_value(&decl.spec, bearer);
    let opts = ConnectOpts {
        interactive_auth: false,
        allowed_tools: allowed_tools(decl),
    };
    let connected = sema_io::io_block_on(connect_send(&cfg_value, &opts, cfg.browser_allowed))?;
    Ok((Box::new(connected), opts))
}

fn needs_auth_send(decl: &McpDecl, auth: &McpAuthDecl, url: &str) -> SendResolution {
    SendResolution::NeedsAuth {
        alias: decl.alias.clone(),
        url: url.to_string(),
        scopes: auth.scopes.clone(),
        tools: decl.tools.clone(),
        persist: persist_str(decl.persist).to_string(),
    }
}

/// Off-quantum twin of [`connect_plain`].
fn connect_plain_send(
    decl: &McpDecl,
    workflow: &str,
    run_id: &str,
    cfg: &ResolveConfig,
) -> SendResolution {
    match connect_send_now(decl, None, cfg) {
        Ok((connected, opts)) => SendResolution::Connected {
            alias: decl.alias.clone(),
            connected,
            opts,
            auth: None,
        },
        Err(ConnectFailure::NeedsAuth { url }) => {
            let auth = decl.auth.clone().unwrap_or_default();
            needs_auth_or_interactive_send(decl, &auth, &url, workflow, run_id, cfg)
        }
        Err(ConnectFailure::Failed(reason)) => SendResolution::Failed {
            alias: decl.alias.clone(),
            reason,
        },
    }
}

/// Off-quantum twin of [`needs_auth_or_interactive`]. `login_interactive` runs on
/// this plain worker thread — no runtime is being driven here (each connect/
/// refresh enters+exits `io_block_on` separately), so its private current-thread
/// runtime does not nest.
fn needs_auth_or_interactive_send(
    decl: &McpDecl,
    auth: &McpAuthDecl,
    url: &str,
    workflow: &str,
    run_id: &str,
    cfg: &ResolveConfig,
) -> SendResolution {
    if !cfg.interactive_auth || !cfg.browser_allowed {
        return needs_auth_send(decl, auth, url);
    }

    let scoped_store = match store_for(decl.persist, workflow, run_id) {
        Ok(s) => s,
        Err(reason) => {
            eprintln!("{}: authentication failed — {reason}", decl.alias);
            return needs_auth_send(decl, auth, url);
        }
    };

    eprintln!("{}: authentication required — opening browser…", decl.alias);

    match login_interactive(url, auth.client_id.as_deref(), cfg.make_opener()) {
        Ok(creds) => {
            if decl.persist != McpPersist::None {
                let _ = scoped_store.save(&creds);
            }
            // `None` retry_ctx: this connect IS the interactive retry — a still-
            // rejected freshly-consented token gates immediately, never re-prompts.
            connect_with_token_send(decl, &creds.tokens, "consented", None, cfg)
        }
        Err(reason) => {
            eprintln!("{}: authentication failed — {reason}", decl.alias);
            needs_auth_send(decl, auth, url)
        }
    }
}

/// Off-quantum twin of [`resolve_authenticated_http`].
fn resolve_authenticated_http_send(
    decl: &McpDecl,
    auth: &McpAuthDecl,
    workflow: &str,
    run_id: &str,
    cfg: &ResolveConfig,
) -> SendResolution {
    let McpSpecDecl::Http { url, .. } = &decl.spec else {
        unreachable!("declared_mcp rejects :auth on a stdio spec");
    };

    let scoped_store = match store_for(decl.persist, workflow, run_id) {
        Ok(s) => s,
        Err(reason) => {
            return SendResolution::Failed {
                alias: decl.alias.clone(),
                reason,
            }
        }
    };

    let (stored, imported_from_default) = if decl.persist == McpPersist::Keyring {
        (scoped_store.load(url), false)
    } else {
        match scoped_store.load(url) {
            Some(creds) => (Some(creds), false),
            None => match store::default_store().load(url) {
                Some(creds) => (Some(creds), true),
                None => (None, false),
            },
        }
    };

    let Some(mut stored) = stored else {
        return needs_auth_or_interactive_send(decl, auth, url, workflow, run_id, cfg);
    };

    if imported_from_default && decl.persist != McpPersist::None {
        let _ = scoped_store.save(&stored);
    }

    let now = store::now_unix();
    if !stored.tokens.is_expired(now, EXPIRY_SKEW_SECS) {
        return connect_with_token_send(
            decl,
            &stored.tokens,
            "cached",
            Some((workflow, run_id)),
            cfg,
        );
    }

    if stored.tokens.refresh_token.is_some() {
        let login_config = LoginConfig {
            mcp_url: url,
            resource_metadata_url: None,
            requested_scope: stored.tokens.scope.as_deref(),
            preconfigured_client_id: auth.client_id.as_deref(),
        };
        let http = reqwest::Client::new();
        match sema_io::io_block_on(login::refresh(&http, &login_config, &stored)) {
            Ok(tokens) => {
                stored.tokens = tokens;
                let _ = scoped_store.save(&stored);
                return connect_with_token_send(
                    decl,
                    &stored.tokens,
                    "refreshed",
                    Some((workflow, run_id)),
                    cfg,
                );
            }
            Err(_) => {
                return needs_auth_or_interactive_send(decl, auth, url, workflow, run_id, cfg);
            }
        }
    }

    needs_auth_or_interactive_send(decl, auth, url, workflow, run_id, cfg)
}

/// Off-quantum twin of [`connect_with_token`]. `retry_ctx` semantics are
/// identical: `Some((workflow, run_id))` allows one interactive retry on a
/// `NeedsAuth`; `None` gates straight to `needs_auth` (the one-shot guard).
fn connect_with_token_send(
    decl: &McpDecl,
    tokens: &TokenSet,
    source: &str,
    retry_ctx: Option<(&str, &str)>,
    cfg: &ResolveConfig,
) -> SendResolution {
    match connect_send_now(decl, Some(&tokens.access_token), cfg) {
        Ok((connected, opts)) => SendResolution::Connected {
            alias: decl.alias.clone(),
            connected,
            opts,
            auth: Some(AuthGrant {
                scopes: split_scopes(tokens.scope.as_deref()),
                expires_at: tokens.expires_at,
                source: source.to_string(),
            }),
        },
        Err(ConnectFailure::NeedsAuth { url }) => {
            let auth = decl.auth.clone().unwrap_or_default();
            match retry_ctx {
                Some((workflow, run_id)) => {
                    needs_auth_or_interactive_send(decl, &auth, &url, workflow, run_id, cfg)
                }
                None => needs_auth_send(decl, &auth, &url),
            }
        }
        Err(ConnectFailure::Failed(reason)) => SendResolution::Failed {
            alias: decl.alias.clone(),
            reason,
        },
    }
}

/// VM-thread decoder for the resolve offload: registers each `Connected` client
/// into the `CONNECTIONS` table (minting its opaque handle `Value`) and encodes
/// the `Vec<ServerResolution>` into the resumed `Value` — the only place a
/// `Value` is built. Holds no `Value`, so its trace is edge-free.
struct ResolveDecoder;

impl Trace for ResolveDecoder {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}

impl CompletionDecoder for ResolveDecoder {
    fn decode(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        result: Result<SendPayload, ExternalFailure>,
    ) -> DecodedCompletion {
        let payload = result.map_err(|failure| {
            SemaError::eval(format!("workflow/mcp-resolve: {}", failure.message()))
        })?;
        let outs = downcast_send_payload::<Vec<SendResolution>>(payload, "workflow/mcp-resolve")
            .map_err(|failure| SemaError::eval(failure.message().to_string()))?;
        let resolutions: Vec<ServerResolution> =
            outs.into_iter().map(register_send_resolution).collect();
        Ok(workflow_mcp::encode_resolutions(&resolutions))
    }
}

/// Register one `Send` outcome on the VM thread, minting the opaque handle for a
/// `Connected`.
fn register_send_resolution(res: SendResolution) -> ServerResolution {
    match res {
        SendResolution::Connected {
            alias,
            connected,
            opts,
            auth,
        } => ServerResolution::Connected {
            alias,
            handle: register_connected(*connected, &opts),
            auth,
        },
        SendResolution::NeedsAuth {
            alias,
            url,
            scopes,
            tools,
            persist,
        } => ServerResolution::NeedsAuth {
            alias,
            url,
            scopes,
            tools,
            persist,
        },
        SendResolution::Failed { alias, reason } => ServerResolution::Failed { alias, reason },
    }
}

/// Cancel hook for the resolve offload: the blocking job cannot be interrupted
/// mid-flight (best-effort, matching `checkout_external`'s blocking tier), so
/// there is nothing to signal — the runtime discards the completion on cancel,
/// dropping every `ConnectedClient` in the payload (closing its connection).
/// Reports the resource reaped either way.
struct ResolveCancelHook;

impl Trace for ResolveCancelHook {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}

impl CancelHook for ResolveCancelHook {
    fn cancel(&mut self) -> Result<CancelDisposition, CancelHookError> {
        Ok(CancelDisposition::Reaped)
    }
    fn reap(&mut self) -> Result<CancelDisposition, CancelHookError> {
        Ok(CancelDisposition::Reaped)
    }
}

fn resolve_one(decl: &McpDecl, workflow: &str, run_id: &str) -> ServerResolution {
    match (&decl.spec, &decl.auth) {
        // Stdio never speaks OAuth (declared_mcp already forbids :auth there);
        // HTTP without :auth is bring-your-own (:headers) or genuinely open.
        (McpSpecDecl::Stdio { .. }, _) | (McpSpecDecl::Http { .. }, None) => {
            connect_plain(decl, workflow, run_id)
        }
        (McpSpecDecl::Http { .. }, Some(auth)) => {
            resolve_authenticated_http(decl, auth, workflow, run_id)
        }
    }
}

/// `:tools` manifest → `ConnectOpts::allowed_tools` (empty declared list = "all
/// tools", matching `declared_mcp`'s own "omit for all tools" convention).
fn allowed_tools(decl: &McpDecl) -> Option<Vec<String>> {
    if decl.tools.is_empty() {
        None
    } else {
        Some(decl.tools.clone())
    }
}

fn persist_str(persist: McpPersist) -> &'static str {
    match persist {
        McpPersist::Keyring => "keyring",
        McpPersist::Workflow => "workflow",
        McpPersist::Run => "run",
        McpPersist::None => "none",
    }
}

/// Build the config `Value` `mcp/connect` accepts from a declared spec, optionally
/// injecting a bearer token — the same seam `connect_from_config`'s docs describe
/// ("inject it directly via `:headers {"Authorization" "Bearer …"}`").
fn spec_config_value(spec: &McpSpecDecl, bearer: Option<&str>) -> Value {
    let mut m = std::collections::BTreeMap::new();
    match spec {
        McpSpecDecl::Http { url, headers } => {
            m.insert(Value::keyword("url"), Value::string(url));
            let mut hm = std::collections::BTreeMap::new();
            for (k, v) in headers {
                hm.insert(Value::string(k), Value::string(v));
            }
            if let Some(token) = bearer {
                hm.insert(
                    Value::string("Authorization"),
                    Value::string(&format!("Bearer {token}")),
                );
            }
            if !hm.is_empty() {
                m.insert(Value::keyword("headers"), Value::map(hm));
            }
        }
        McpSpecDecl::Stdio {
            command,
            args,
            env,
            cwd,
        } => {
            m.insert(Value::keyword("command"), Value::string(command));
            if !args.is_empty() {
                m.insert(
                    Value::keyword("args"),
                    Value::list(args.iter().map(|a| Value::string(a)).collect()),
                );
            }
            if !env.is_empty() {
                let mut em = std::collections::BTreeMap::new();
                for (k, v) in env {
                    em.insert(Value::string(k), Value::string(v));
                }
                m.insert(Value::keyword("env"), Value::map(em));
            }
            if let Some(c) = cwd {
                m.insert(Value::keyword("cwd"), Value::string(c));
            }
        }
    }
    Value::map(m)
}

/// Stdio, or HTTP without `:auth`: connect directly (never chasing an OAuth
/// challenge — `interactive_auth: false`). An undeclared-auth server that
/// challenges anyway is honestly reported as `NeedsAuth` (scopes empty — none
/// were ever declared, tools/persist taken from the decl) UNLESS interactive
/// auth is enabled, in which case this is the "no-`:auth` HTTP connect path
/// when the server unexpectedly challenges" case §3(3) of the interactive-auth
/// requirements calls out by name: try the same inline login as the declared-
/// `:auth` path before gating.
fn connect_plain(decl: &McpDecl, workflow: &str, run_id: &str) -> ServerResolution {
    let cfg = spec_config_value(&decl.spec, None);
    let opts = ConnectOpts {
        interactive_auth: false,
        allowed_tools: allowed_tools(decl),
    };
    match connect_from_config(&cfg, opts) {
        Ok(handle) => ServerResolution::Connected {
            alias: decl.alias.clone(),
            handle,
            auth: None,
        },
        Err(ConnectFailure::NeedsAuth { url }) => {
            let auth = decl.auth.clone().unwrap_or_default();
            needs_auth_or_interactive(decl, &auth, &url, workflow, run_id)
        }
        Err(ConnectFailure::Failed(reason)) => ServerResolution::Failed {
            alias: decl.alias.clone(),
            reason,
        },
    }
}

fn needs_auth(decl: &McpDecl, auth: &McpAuthDecl, url: &str) -> ServerResolution {
    ServerResolution::NeedsAuth {
        alias: decl.alias.clone(),
        url: url.to_string(),
        scopes: auth.scopes.clone(),
        tools: decl.tools.clone(),
        persist: persist_str(decl.persist).to_string(),
    }
}

/// A `ServerResolution::NeedsAuth` return point — the headless-precursor gate
/// (`needs_auth`, above) — reinterpreted for an interactive run: if
/// [`set_interactive_auth`] enabled inline login for this run AND the sandbox
/// permits opening a browser (`Caps::PROCESS`, checked via
/// `sema_mcp::browser_open_allowed` — the SAME gate the interactive
/// `mcp/connect` path applies; a denial here NEVER attempts a browser, it just
/// falls straight through to the ordinary headless gate), run the SAME
/// browser/loopback OAuth flow `sema mcp login` performs, synchronously, right
/// here. Multiple unsatisfied servers are prompted sequentially — `resolve`
/// (above) calls this once per declared server, in alias order, and each call
/// blocks until that server's login resolves — one browser tab at a time.
///
/// On success: persist to `decl`'s scoped store — unless `:persist :none`,
/// which uses the credential for this connection only, matching the same
/// "use without persisting" semantics `:none` already has elsewhere in this
/// file — then connect with the fresh token, `source: "consented"` (the
/// `AuthGranted.source` value the plan reserves for this case). On any
/// failure (declined consent, timed-out redirect, discovery/DCR error,
/// `store_for` failure): print ONE stderr line naming the reason — never the
/// token — and fall back to the ordinary `NeedsAuth` resolution, so the run
/// gates and exits 2 with guidance exactly as the non-interactive path
/// already does.
fn needs_auth_or_interactive(
    decl: &McpDecl,
    auth: &McpAuthDecl,
    url: &str,
    workflow: &str,
    run_id: &str,
) -> ServerResolution {
    if !interactive_auth_enabled() || !browser_open_allowed() {
        return needs_auth(decl, auth, url);
    }

    let scoped_store = match store_for(decl.persist, workflow, run_id) {
        Ok(s) => s,
        Err(reason) => {
            eprintln!("{}: authentication failed — {reason}", decl.alias);
            return needs_auth(decl, auth, url);
        }
    };

    eprintln!("{}: authentication required — opening browser…", decl.alias);

    match login_interactive(
        url,
        auth.client_id.as_deref(),
        Some(interactive_login_opener()),
    ) {
        Ok(creds) => {
            if decl.persist != McpPersist::None {
                let _ = scoped_store.save(&creds);
            }
            // `None`: this connect IS the interactive retry — a still-rejected
            // freshly-consented token must gate immediately, never prompt again.
            connect_with_token(decl, &creds.tokens, "consented", None)
        }
        Err(reason) => {
            eprintln!("{}: authentication failed — {reason}", decl.alias);
            needs_auth(decl, auth, url)
        }
    }
}

fn split_scopes(scope: Option<&str>) -> Vec<String> {
    scope
        .map(|s| s.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default()
}

/// Resolve the [`TokenStore`] for a declared `:persist` scope. `workflow`/`run_id`
/// name the `:workflow`/`:run` scoped directories; `store_encryption_key`
/// failures (env var unset AND no OS keyring) only matter for those two file
/// scopes — `:keyring` and `:none` never call it.
///
/// `pub(crate)`: the dashboard's `POST …/connect|forget` write endpoints
/// (`crate::workflow_view::connect`) reuse this exact resolution so a session
/// persisted from the panel lands in the SAME store a subsequent `workflow/run`
/// would resolve — one persistence policy, not two.
pub(crate) fn store_for(
    persist: McpPersist,
    workflow: &str,
    run_id: &str,
) -> Result<Box<dyn TokenStore>, String> {
    match persist {
        McpPersist::Keyring => Ok(store::default_store()),
        McpPersist::Workflow => {
            let key = store_encryption_key()?;
            let dir = PathBuf::from(".sema/auth").join(workflow);
            Ok(Box::new(ScopedFileStore::new(dir, key)))
        }
        McpPersist::Run => {
            let key = store_encryption_key()?;
            // Mirrors sema_workflow::context::resolve_runs_root(): the
            // SEMA_WORKFLOW_RUN_DIR env seam if set, else the project-local
            // ".sema/runs" default — reused directly (that fn is pub) so the
            // two never drift.
            let runs_root = sema_workflow::context::resolve_runs_root();
            let dir = PathBuf::from(runs_root).join(run_id).join("auth");
            Ok(Box::new(ScopedFileStore::new(dir, key)))
        }
        McpPersist::None => Ok(Box::new(MemoryStore::new())),
    }
}

/// HTTP + `:auth`: token first (from the scoped store, falling back to the
/// default store, refreshing if expired), then connect. See
/// `docs/plans/2026-06-24-workflow-mcp-auth.md` §3/§4 and the task brief's
/// numbered steps — this function implements all of them in order.
fn resolve_authenticated_http(
    decl: &McpDecl,
    auth: &McpAuthDecl,
    workflow: &str,
    run_id: &str,
) -> ServerResolution {
    let McpSpecDecl::Http { url, .. } = &decl.spec else {
        unreachable!("declared_mcp rejects :auth on a stdio spec");
    };

    let scoped_store = match store_for(decl.persist, workflow, run_id) {
        Ok(s) => s,
        Err(reason) => {
            return ServerResolution::Failed {
                alias: decl.alias.clone(),
                reason,
            }
        }
    };

    // Load by server URL from the scoped store; on miss, fall back to the
    // default (keyring/file) store — a hit there is imported (saved) into the
    // scoped store (except :none, which just uses it for this run), so a prior
    // `sema mcp login <url>` satisfies any workflow (the plan's headless loop).
    // `:keyring` persist skips the fallback: `store_for` already resolved
    // `scoped_store` to `default_store()` for that scope, so re-querying it
    // would just hit the same store, and "importing" a hit into itself is a
    // no-op save worth skipping.
    let (stored, imported_from_default) = if decl.persist == McpPersist::Keyring {
        (scoped_store.load(url), false)
    } else {
        match scoped_store.load(url) {
            Some(creds) => (Some(creds), false),
            None => match store::default_store().load(url) {
                Some(creds) => (Some(creds), true),
                None => (None, false),
            },
        }
    };

    let Some(mut stored) = stored else {
        return needs_auth_or_interactive(decl, auth, url, workflow, run_id);
    };

    if imported_from_default && decl.persist != McpPersist::None {
        let _ = scoped_store.save(&stored);
    }

    let now = store::now_unix();
    if !stored.tokens.is_expired(now, EXPIRY_SKEW_SECS) {
        return connect_with_token(decl, &stored.tokens, "cached", Some((workflow, run_id)));
    }

    if stored.tokens.refresh_token.is_some() {
        let login_config = LoginConfig {
            mcp_url: url,
            resource_metadata_url: None,
            requested_scope: stored.tokens.scope.as_deref(),
            preconfigured_client_id: auth.client_id.as_deref(),
        };
        let http = reqwest::Client::new();
        match sema_io::io_block_on(login::refresh(&http, &login_config, &stored)) {
            Ok(tokens) => {
                stored.tokens = tokens;
                let _ = scoped_store.save(&stored);
                return connect_with_token(
                    decl,
                    &stored.tokens,
                    "refreshed",
                    Some((workflow, run_id)),
                );
            }
            Err(_) => {
                // Refresh failure re-gates rather than deleting the stored
                // creds (plan §10 Q3: "refresh silently; only re-gate if
                // refresh fails" — the stale refresh token might still work
                // later, e.g. a transient network error).
                return needs_auth_or_interactive(decl, auth, url, workflow, run_id);
            }
        }
    }

    needs_auth_or_interactive(decl, auth, url, workflow, run_id)
}

/// Connect with a resolved access token attached as a Bearer header. A connect
/// that itself reports `NeedsAuth` (the server rejects a token this process
/// believed was locally fresh) re-gates WITHOUT deleting the stored
/// credentials — a transient server-side issue shouldn't force a fresh consent
/// when the token might still be valid on the next run.
///
/// `retry_ctx`: `Some((workflow, run_id))` allows ONE interactive retry on that
/// `NeedsAuth` (via [`needs_auth_or_interactive`]) when interactive auth is
/// enabled — `needs_auth_or_interactive` applies its own
/// `interactive_auth_enabled`/`browser_open_allowed` gate, so passing `Some`
/// here is a "may retry", not a "will retry". `None` disables the retry
/// entirely; the ONE existing caller that passes `None` is
/// `needs_auth_or_interactive` itself, feeding back the token it just
/// consented to — that call IS the retry, so a second `NeedsAuth` here must
/// gate straight to `needs_auth` rather than prompting again (a one-shot
/// guard, not recursion).
fn connect_with_token(
    decl: &McpDecl,
    tokens: &TokenSet,
    source: &str,
    retry_ctx: Option<(&str, &str)>,
) -> ServerResolution {
    let cfg = spec_config_value(&decl.spec, Some(&tokens.access_token));
    let opts = ConnectOpts {
        interactive_auth: false,
        allowed_tools: allowed_tools(decl),
    };
    match connect_from_config(&cfg, opts) {
        Ok(handle) => ServerResolution::Connected {
            alias: decl.alias.clone(),
            handle,
            auth: Some(AuthGrant {
                scopes: split_scopes(tokens.scope.as_deref()),
                expires_at: tokens.expires_at,
                source: source.to_string(),
            }),
        },
        Err(ConnectFailure::NeedsAuth { url }) => {
            let auth = decl.auth.clone().unwrap_or_default();
            match retry_ctx {
                Some((workflow, run_id)) => {
                    needs_auth_or_interactive(decl, &auth, &url, workflow, run_id)
                }
                None => needs_auth(decl, &auth, &url),
            }
        }
        Err(ConnectFailure::Failed(reason)) => ServerResolution::Failed {
            alias: decl.alias.clone(),
            reason,
        },
    }
}
