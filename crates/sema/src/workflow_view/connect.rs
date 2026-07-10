//! `POST /api/run/:id/auth/:alias/connect` and `.../forget` — the dashboard's
//! one-click write endpoints (plan `docs/plans/2026-06-24-workflow-mcp-auth.md`
//! §5/§8, item (d)'s write-endpoint half). Both routes require the caller to
//! have already passed the session-token check in `super::route` — nothing in
//! this module re-checks the token, and nothing here ever reads or logs it.
//!
//! `connect` triggers the SAME `login_interactive` browser/loopback OAuth flow
//! `sema mcp login` and the run-start interactive-auth path (`crate::workflow_mcp`)
//! run, on a `spawn_blocking` task (never on the async accept loop — the flow
//! blocks on its own single-threaded tokio runtime, see
//! `sema_mcp::client_auth::login_interactive`'s doc comment on nested
//! runtimes). It answers immediately with `202 {"status":"connecting"}` and
//! records progress in [`ServerState::flows`]; the panel polls the existing
//! `GET …/auth` endpoint (`super::auth`), whose response this module's flow
//! state overrides while a flow is pending/just-finished.
//!
//! `forget` is synchronous (local file deletes only, no network) and best-
//! effort: deleting an absent entry is success, matching every `TokenStore`
//! impl's own `delete` semantics.
//!
//! Never persists (or returns) token material itself — only `StoredCredentials`
//! ever touches a token, and it flows straight from `login_interactive` into a
//! `TokenStore::save`, never through [`FlowState`] or a response body.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use sema_mcp::oauth::loopback::BrowserOpener;
use sema_stdlib::workflow_mcp::McpPersist;

use super::{is_safe_segment, JsonResponse};

/// `(run_id, alias)` — a flow is scoped to one declared server within one run.
pub(crate) type FlowKey = (String, String);

/// The latest outcome of a dashboard-initiated login for one `(run, alias)`
/// pair, as tracked in-memory by [`ServerState::flows`]. Never holds token
/// material — only what the panel needs to render a status.
#[derive(Debug, Clone)]
pub(crate) enum FlowState {
    /// A `login_interactive` task is in flight; a second `connect` for the
    /// same key is a no-op (see [`handle_connect`]).
    Connecting,
    /// Login succeeded and (unless `:persist :none`) landed in the scoped
    /// store.
    Authorized { expires_at: Option<u64> },
    /// Login failed; `reason` is the exact string `login_interactive` (and so
    /// the CLI) would print — never token material.
    Failed { reason: String },
}

/// Test-only browser-opener override — a plain `fn` pointer (not a capturing
/// closure), the same shape `crate::workflow_mcp`'s
/// `set_interactive_login_opener` uses, and for the same reason: it must be
/// `Send + 'static` to cross into the `spawn_blocking` task below, which a
/// `fn` pointer trivially is.
pub type TestOpenerFn = fn(&str) -> Result<(), String>;

/// Shared server-side state for one `sema workflow view` process: the run
/// directory, the minted session token (§8 hardening, see `super`'s module
/// doc), and the in-memory flow map POST handlers write and the GET handler
/// reads. Confined to the server (async/tokio) side of the process — never
/// passed into interpreter-side code, which stays `Rc`/single-threaded.
pub(crate) struct ServerState {
    pub run_dir: std::path::PathBuf,
    pub token: String,
    /// `Arc<Mutex<HashMap<(run_id, alias), FlowState>>>` — shared between
    /// every connection's `connect`/`forget`/`GET …/auth` handler via the
    /// single `Arc<ServerState>` each accepted connection clones.
    flows: Mutex<HashMap<FlowKey, FlowState>>,
    test_opener: Option<TestOpenerFn>,
}

impl ServerState {
    pub fn new(
        run_dir: std::path::PathBuf,
        token: String,
        test_opener: Option<TestOpenerFn>,
    ) -> Self {
        Self {
            run_dir,
            token,
            flows: Mutex::new(HashMap::new()),
            test_opener,
        }
    }

    /// This run's flow overrides, alias -> state (cloned out from behind the
    /// lock), for `GET …/auth`'s status merge. A dashboard's live flow count
    /// is at most a handful, so the linear filter over the whole map is fine.
    pub fn flow_snapshot(&self, run_id: &str) -> HashMap<String, FlowState> {
        self.flows
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|((rid, _), _)| rid == run_id)
            .map(|((_, alias), state)| (alias.clone(), state.clone()))
            .collect()
    }

    fn opener(&self) -> BrowserOpener {
        match self.test_opener {
            Some(f) => Box::new(f),
            None => sema_mcp::gated_browser_opener(),
        }
    }
}

fn not_found() -> JsonResponse {
    ("404 Not Found", "text/plain", b"no such run/alias".to_vec())
}

fn bad_request() -> JsonResponse {
    (
        "400 Bad Request",
        "text/plain",
        b"alias is not an HTTP MCP server".to_vec(),
    )
}

fn connecting_response() -> JsonResponse {
    (
        "202 Accepted",
        "application/json",
        br#"{"status":"connecting"}"#.to_vec(),
    )
}

/// `metadata.json`'s workflow name + `:mcp` manifest — the write-path's own
/// read, separate from `super::auth::read_mcp_manifest` (which the read-only
/// status endpoint uses and which deliberately never needs the workflow
/// name). Degrades to `None` on anything missing/malformed, same discipline
/// as the read side.
fn read_manifest(dir: &Path) -> Option<(String, serde_json::Map<String, serde_json::Value>)> {
    let text = std::fs::read_to_string(dir.join("metadata.json")).ok()?;
    let meta: serde_json::Value = serde_json::from_str(&text).ok()?;
    let workflow = meta.get("workflow")?.as_str()?.to_string();
    let mcp = meta.get("meta")?.get("mcp")?.as_object()?.clone();
    Some((workflow, mcp))
}

/// `:persist` off the manifest spec, defaulting to `:workflow` — mirrors
/// `sema_stdlib::workflow_mcp`'s own parse default (`McpPersist::default()`).
fn parse_persist(spec: &serde_json::Value) -> McpPersist {
    match spec.get("persist").and_then(|v| v.as_str()) {
        Some("keyring") => McpPersist::Keyring,
        Some("run") => McpPersist::Run,
        Some("none") => McpPersist::None,
        _ => McpPersist::Workflow,
    }
}

/// Validate `id`/`alias` against the run's manifest, returning the declared
/// HTTP spec's `(workflow, url, client_id, persist)` — the shared prefix of
/// `connect` and `forget`. `Err` is already the full 404/400 response.
fn resolve_declared_http_server(
    run_dir: &Path,
    id: &str,
    alias: &str,
) -> Result<(String, String, Option<String>, McpPersist), JsonResponse> {
    if !is_safe_segment(id) || !is_safe_segment(alias) {
        return Err(not_found());
    }
    let dir = run_dir.join(id);
    let Some((workflow, mcp)) = read_manifest(&dir) else {
        return Err(not_found());
    };
    let Some(spec) = mcp.get(alias) else {
        return Err(not_found());
    };
    let Some(url) = spec.get("url").and_then(|v| v.as_str()) else {
        return Err(bad_request());
    };
    let client_id = spec
        .get("auth")
        .and_then(|a| a.get("client-id"))
        .and_then(|v| v.as_str())
        .map(String::from);
    let persist = parse_persist(spec);
    Ok((workflow, url.to_string(), client_id, persist))
}

/// `POST …/connect`. Validates, then either reports an already-pending flow
/// (idempotent — never starts a second one for the same `(run, alias)`) or
/// records `Connecting` and hands the login off to a `spawn_blocking` task.
/// Always answers `202 {"status":"connecting"}` on the happy/pending path —
/// the panel polls `GET …/auth` for the terminal state.
pub(crate) fn handle_connect(state: &Arc<ServerState>, id: &str, alias: &str) -> JsonResponse {
    let (workflow, url, client_id, persist) =
        match resolve_declared_http_server(&state.run_dir, id, alias) {
            Ok(v) => v,
            Err(resp) => return resp,
        };

    let key: FlowKey = (id.to_string(), alias.to_string());
    {
        let mut flows = state.flows.lock().unwrap_or_else(|e| e.into_inner());
        if matches!(flows.get(&key), Some(FlowState::Connecting)) {
            return connecting_response();
        }
        flows.insert(key.clone(), FlowState::Connecting);
    }

    let state = Arc::clone(state);
    let opener = state.opener();
    let run_id = id.to_string();
    tokio::task::spawn_blocking(move || {
        run_connect_flow(
            &state, key, workflow, run_id, persist, url, client_id, opener,
        );
    });

    connecting_response()
}

/// The actual OAuth flow, run on a `spawn_blocking` thread (see the module
/// doc's nested-runtime note). Persists to the decl's `:persist` scoped store
/// on success (skipped for `:none` — the credential is used for this
/// process's status only, never written to disk) and records the terminal
/// [`FlowState`] either way.
#[allow(clippy::too_many_arguments)]
fn run_connect_flow(
    state: &ServerState,
    key: FlowKey,
    workflow: String,
    run_id: String,
    persist: McpPersist,
    url: String,
    client_id: Option<String>,
    opener: BrowserOpener,
) {
    let scoped_store = match crate::workflow_mcp::store_for(persist, &workflow, &run_id) {
        Ok(s) => s,
        Err(reason) => {
            set_flow(state, key, FlowState::Failed { reason });
            return;
        }
    };

    match sema_mcp::login_interactive(&url, client_id.as_deref(), Some(opener)) {
        Ok(creds) => {
            if persist != McpPersist::None {
                let _ = scoped_store.save(&creds);
            }
            let expires_at = creds.tokens.expires_at;
            set_flow(state, key, FlowState::Authorized { expires_at });
        }
        Err(reason) => set_flow(state, key, FlowState::Failed { reason }),
    }
}

fn set_flow(state: &ServerState, key: FlowKey, flow: FlowState) {
    state
        .flows
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key, flow);
}

/// `POST …/forget`. Deletes the stored session for the declared server's URL
/// from BOTH the decl's scoped store and the default store (an imported
/// session — see `crate::workflow_mcp::resolve_authenticated_http`'s
/// default-store fallback — must not silently resurrect on the next run),
/// clears any in-memory flow state, and always answers `200
/// {"status":"forgotten"}` — every `TokenStore::delete` already treats a
/// missing entry as success, and a `store_for` failure (e.g. no encryption
/// key configured) just means there was never anything to delete there.
pub(crate) fn handle_forget(state: &Arc<ServerState>, id: &str, alias: &str) -> JsonResponse {
    let (workflow, url, _client_id, persist) =
        match resolve_declared_http_server(&state.run_dir, id, alias) {
            Ok(v) => v,
            Err(resp) => return resp,
        };

    if let Ok(scoped_store) = crate::workflow_mcp::store_for(persist, &workflow, id) {
        let _ = scoped_store.delete(&url);
    }
    let _ = sema_mcp::oauth::store::default_store().delete(&url);

    state
        .flows
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&(id.to_string(), alias.to_string()));

    (
        "200 OK",
        "application/json",
        br#"{"status":"forgotten"}"#.to_vec(),
    )
}
