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

use std::path::PathBuf;
use std::rc::Rc;

use sema_core::Value;
use sema_mcp::oauth::login::{self, LoginConfig};
use sema_mcp::oauth::scoped::{store_encryption_key, MemoryStore, ScopedFileStore};
use sema_mcp::oauth::store::{self, TokenSet, TokenStore};
use sema_mcp::{close_handle, connect_from_config, ConnectFailure, ConnectOpts};
use sema_stdlib::workflow_mcp::{
    self, AuthGrant, McpAuthDecl, McpDecl, McpPersist, McpSpecDecl, ServerResolution,
    WorkflowMcpResolver,
};

/// Clock skew (seconds) a cached token is treated as expired before its nominal
/// expiry — mirrors `oauth::login`'s own `EXPIRY_SKEW_SECS` (private to that
/// module), reused here via the public [`TokenSet::is_expired`].
const EXPIRY_SKEW_SECS: u64 = 60;

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

    fn close(&self, handles: &[Value]) {
        for handle in handles {
            close_handle(handle);
        }
    }
}

fn resolve_one(decl: &McpDecl, workflow: &str, run_id: &str) -> ServerResolution {
    match (&decl.spec, &decl.auth) {
        // Stdio never speaks OAuth (declared_mcp already forbids :auth there);
        // HTTP without :auth is bring-your-own (:headers) or genuinely open.
        (McpSpecDecl::Stdio { .. }, _) | (McpSpecDecl::Http { .. }, None) => connect_plain(decl),
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
/// challenges anyway is honestly reported as `NeedsAuth`, scopes empty (none were
/// ever declared) and tools/persist taken from the decl.
fn connect_plain(decl: &McpDecl) -> ServerResolution {
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
        Err(ConnectFailure::NeedsAuth { url }) => ServerResolution::NeedsAuth {
            alias: decl.alias.clone(),
            url,
            scopes: Vec::new(),
            tools: decl.tools.clone(),
            persist: persist_str(decl.persist).to_string(),
        },
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

fn split_scopes(scope: Option<&str>) -> Vec<String> {
    scope
        .map(|s| s.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default()
}

/// Resolve the [`TokenStore`] for a declared `:persist` scope. `workflow`/`run_id`
/// name the `:workflow`/`:run` scoped directories; `store_encryption_key`
/// failures (env var unset AND no OS keyring) only matter for those two file
/// scopes — `:keyring` and `:none` never call it.
fn store_for(
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
        return needs_auth(decl, auth, url);
    };

    if imported_from_default && decl.persist != McpPersist::None {
        let _ = scoped_store.save(&stored);
    }

    let now = store::now_unix();
    if !stored.tokens.is_expired(now, EXPIRY_SKEW_SECS) {
        return connect_with_token(decl, &stored.tokens, "cached");
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
                return connect_with_token(decl, &stored.tokens, "refreshed");
            }
            Err(_) => {
                // Refresh failure re-gates rather than deleting the stored
                // creds (plan §10 Q3: "refresh silently; only re-gate if
                // refresh fails" — the stale refresh token might still work
                // later, e.g. a transient network error).
                return needs_auth(decl, auth, url);
            }
        }
    }

    needs_auth(decl, auth, url)
}

/// Connect with a resolved access token attached as a Bearer header. A connect
/// that itself reports `NeedsAuth` (the server rejects the token) re-gates
/// WITHOUT deleting the stored credentials — a transient server-side issue
/// shouldn't force a fresh consent when the token might still be valid on retry.
fn connect_with_token(decl: &McpDecl, tokens: &TokenSet, source: &str) -> ServerResolution {
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
            needs_auth(decl, &auth, &url)
        }
        Err(ConnectFailure::Failed(reason)) => ServerResolution::Failed {
            alias: decl.alias.clone(),
            reason,
        },
    }
}
