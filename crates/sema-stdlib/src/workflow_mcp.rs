//! `:mcp` declaration parsing + validation for `defworkflow` meta maps.
//!
//! A workflow declares the MCP servers it needs in its meta map (next to
//! `:budget`/`:permissions`/`:args`):
//!
//! ```sema
//! :mcp {asana {:url   "https://mcp.asana.com/mcp"
//!              :auth  {:scopes ["default"]}
//!              :tools ["create_task" "search_tasks"]
//!              :persist :workflow}
//!       fs    {:command "npx" :args ["-y" "@modelcontextprotocol/server-filesystem" "."]}}
//! ```
//!
//! This module is the DATA layer only: [`declared_mcp`] parses + validates that map
//! into owned Rust types. It does NOT connect to anything (no network, no process
//! spawn) — a later task resolves `McpDecl`s into live `mcp/connect` handles inside
//! `workflow/run`, mapping these types to `sema-mcp`'s (that mapping lives in the
//! binary crate, since `sema-stdlib` must not depend on `sema-mcp`). See
//! `docs/plans/2026-06-24-workflow-mcp-auth.md` §2.
//!
//! [`McpDecl`]/[`McpSpecDecl`]/[`McpAuthDecl`]/[`McpPersist`] deliberately MIRROR (do
//! not import) the shapes `mcp/connect` and `sema_mcp::oauth::scoped::PersistScope`
//! use, so the surface syntax stays "one spec shape, not two" (the same discipline
//! `sema-llm`'s `ChatRequest` follows) without creating a cross-crate dependency.
//!
//! `crates/sema-stdlib/src/workflow_check.rs` reuses the small shared predicates
//! below ([`spec_transport`], [`is_valid_persist_keyword`], [`is_string_list`]) for
//! its `E-MCP-SPEC`/`E-MCP-PERSIST`/`W-MCP-TOOLS` diagnostics, so the static checker
//! and this runtime parser never disagree about what shape is valid.

use sema_core::{SemaError, Value};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

// ── public types ─────────────────────────────────────────────────────────────

/// One declared MCP server: a local alias bound in the workflow scope, its
/// connection spec, an optional auth hint, the tool manifest (empty = "all
/// tools"), and where any resulting auth session persists.
#[derive(Debug, Clone, PartialEq)]
pub struct McpDecl {
    pub alias: String,
    pub spec: McpSpecDecl,
    pub auth: Option<McpAuthDecl>,
    pub tools: Vec<String>,
    pub persist: McpPersist,
}

/// The connection spec — exactly one transport, matching `mcp/connect`'s config map.
#[derive(Debug, Clone, PartialEq)]
pub enum McpSpecDecl {
    Http {
        url: String,
        headers: Vec<(String, String)>,
    },
    Stdio {
        command: String,
        args: Vec<String>,
        env: Vec<(String, String)>,
        cwd: Option<String>,
    },
}

/// `:auth {:scopes [...] :client-id "..."}` — only valid on an `:url` (http) spec;
/// stdio servers never need an OAuth flow.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct McpAuthDecl {
    pub scopes: Vec<String>,
    pub client_id: Option<String>,
}

/// Where a declared server's auth session persists (`:persist :keyring|:workflow|:run|:none`).
/// Mirrors — does not import — `sema_mcp::oauth::scoped::PersistScope`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpPersist {
    /// OS keychain — shared across every workflow/run on this machine.
    Keyring,
    /// `.sema/auth/<workflow-name>/` — reused by every run of one workflow.
    Workflow,
    /// `.sema/runs/<run-id>/auth/` — this run only.
    Run,
    /// In-memory only; never touches disk.
    None,
}

impl Default for McpPersist {
    /// `:persist` defaults to `:workflow` when absent (plan §2/§4) — the owner's
    /// "persist to the workflow" ask: re-auth once per workflow, reused by every
    /// run of it, without going as far as the shared OS keyring.
    fn default() -> Self {
        McpPersist::Workflow
    }
}

/// The four valid `:persist` keywords, spelled once and shared with the checker's
/// `E-MCP-PERSIST` diagnostic so the two never drift.
pub(crate) const PERSIST_KEYWORDS: [&str; 4] = ["keyring", "workflow", "run", "none"];

pub(crate) fn is_valid_persist_keyword(s: &str) -> bool {
    PERSIST_KEYWORDS.contains(&s)
}

/// Which transport an `:mcp` alias's spec map points at: exactly one of `:url`
/// (http) or `:command` (stdio) is valid. Shared between this module's runtime
/// parse and workflow_check's `E-MCP-SPEC` diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SpecTransport {
    Http,
    Stdio,
    Missing,
    Conflict,
}

pub(crate) fn spec_transport(spec_map: &BTreeMap<Value, Value>) -> SpecTransport {
    let has_url = spec_map.contains_key(&Value::keyword("url"));
    let has_command = spec_map.contains_key(&Value::keyword("command"));
    match (has_url, has_command) {
        (true, false) => SpecTransport::Http,
        (false, true) => SpecTransport::Stdio,
        (false, false) => SpecTransport::Missing,
        (true, true) => SpecTransport::Conflict,
    }
}

/// True when `v` is a list/vector whose every element is a string — the shape
/// `:tools`/`:args`/`:scopes` must have. Shared by this module's hard-erroring
/// parse and workflow_check's softer `W-MCP-TOOLS` warning.
pub(crate) fn is_string_list(v: &Value) -> bool {
    v.as_seq()
        .map(|items| items.iter().all(|i| i.as_str().is_some()))
        .unwrap_or(false)
}

// ── parsing + validation ────────────────────────────────────────────────────

/// Parse + validate the `:mcp` key out of a `defworkflow` meta map. Returns an
/// empty vec when `:mcp` is absent (or `meta` itself isn't a map — that shape
/// problem is `E-WF-META`'s to report, not this function's). Every violation is a
/// `SemaError::eval(...)` naming the offending alias, with a `.with_hint(...)`
/// saying what to fix. The returned vec is sorted by alias for a deterministic,
/// replayable order (map iteration order is not itself alias-alphabetical).
pub fn declared_mcp(meta: &Value) -> Result<Vec<McpDecl>, SemaError> {
    let Some(top) = meta.as_map_ref() else {
        return Ok(Vec::new());
    };
    let Some(mcp_val) = top.get(&Value::keyword("mcp")) else {
        return Ok(Vec::new());
    };
    let mcp_map = mcp_val.as_map_ref().ok_or_else(|| {
        SemaError::eval(format!(
            ":mcp must be a map of alias -> server spec, got {}",
            mcp_val.type_name()
        ))
        .with_hint("e.g. :mcp {asana {:url \"https://mcp.asana.com/mcp\"}}")
    })?;

    let mut decls = Vec::with_capacity(mcp_map.len());
    for (alias_val, spec_val) in mcp_map.iter() {
        let alias = alias_val.as_symbol().ok_or_else(|| {
            SemaError::eval(format!(
                ":mcp alias {alias_val} must be a bare symbol, got {}",
                alias_val.type_name()
            ))
            .with_hint(
                "write the alias unquoted, e.g. :mcp {asana {...}} — not a keyword or string key",
            )
        })?;
        decls.push(parse_decl(&alias, spec_val)?);
    }
    decls.sort_by(|a, b| a.alias.cmp(&b.alias));
    Ok(decls)
}

fn decl_error(alias: &str, msg: impl std::fmt::Display, hint: impl Into<String>) -> SemaError {
    SemaError::eval(format!(":mcp {alias}: {msg}")).with_hint(hint)
}

fn parse_decl(alias: &str, spec_val: &Value) -> Result<McpDecl, SemaError> {
    let spec_map = spec_val.as_map_ref().ok_or_else(|| {
        decl_error(
            alias,
            format!("spec must be a map, got {}", spec_val.type_name()),
            "e.g. :mcp {alias {:url \"https://...\"}}",
        )
    })?;

    let spec = parse_spec(alias, spec_map)?;
    let auth = parse_auth(alias, spec_map, &spec)?;
    let tools = parse_string_list(alias, spec_map, "tools")?.unwrap_or_default();
    let persist = parse_persist(alias, spec_map)?;

    Ok(McpDecl {
        alias: alias.to_string(),
        spec,
        auth,
        tools,
        persist,
    })
}

fn parse_spec(alias: &str, spec_map: &BTreeMap<Value, Value>) -> Result<McpSpecDecl, SemaError> {
    match spec_transport(spec_map) {
        SpecTransport::Http => {
            let url = spec_map
                .get(&Value::keyword("url"))
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    decl_error(
                        alias,
                        ":url must be a string",
                        "e.g. :url \"https://mcp.example.com/mcp\"",
                    )
                })?
                .to_string();
            let headers = parse_string_pairs(alias, spec_map, "headers")?;
            Ok(McpSpecDecl::Http { url, headers })
        }
        SpecTransport::Stdio => {
            let command = spec_map
                .get(&Value::keyword("command"))
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    decl_error(alias, ":command must be a string", "e.g. :command \"npx\"")
                })?
                .to_string();
            let args = parse_string_list(alias, spec_map, "args")?.unwrap_or_default();
            let env = parse_string_pairs(alias, spec_map, "env")?;
            let cwd = match spec_map.get(&Value::keyword("cwd")) {
                None => None,
                Some(v) => Some(
                    v.as_str()
                        .ok_or_else(|| {
                            decl_error(alias, ":cwd must be a string", "e.g. :cwd \".\"")
                        })?
                        .to_string(),
                ),
            };
            Ok(McpSpecDecl::Stdio {
                command,
                args,
                env,
                cwd,
            })
        }
        SpecTransport::Missing => Err(decl_error(
            alias,
            "spec is missing :url or :command",
            "http: {:url \"https://...\"}; stdio: {:command \"npx\" :args [...]}",
        )),
        SpecTransport::Conflict => Err(decl_error(
            alias,
            "spec has both :url and :command — pick exactly one transport",
            "use :url for an http server or :command for stdio, never both",
        )),
    }
}

fn parse_auth(
    alias: &str,
    spec_map: &BTreeMap<Value, Value>,
    spec: &McpSpecDecl,
) -> Result<Option<McpAuthDecl>, SemaError> {
    let Some(v) = spec_map.get(&Value::keyword("auth")) else {
        return Ok(None);
    };
    if matches!(spec, McpSpecDecl::Stdio { .. }) {
        return Err(decl_error(
            alias,
            ":auth is not valid on a stdio (:command) spec — stdio servers never need an OAuth flow",
            "drop :auth, or connect over :url if this server needs authentication",
        ));
    }
    let auth_map = v.as_map_ref().ok_or_else(|| {
        decl_error(
            alias,
            format!(":auth must be a map, got {}", v.type_name()),
            "e.g. :auth {:scopes [\"default\"]}",
        )
    })?;
    let scopes = parse_string_list(alias, auth_map, "scopes")?.unwrap_or_default();
    let client_id = match auth_map.get(&Value::keyword("client-id")) {
        None => None,
        Some(v) => Some(
            v.as_str()
                .ok_or_else(|| {
                    decl_error(
                        alias,
                        ":auth :client-id must be a string",
                        "e.g. :client-id \"my-client\"",
                    )
                })?
                .to_string(),
        ),
    };
    Ok(Some(McpAuthDecl { scopes, client_id }))
}

/// A `:key` list-of-strings value: `None` when absent, `Some(vec)` when present and
/// every element is a string. Rejects non-strings loudly (mirrors `mcp/connect`'s
/// `:args` discipline) rather than silently dropping them.
fn parse_string_list(
    alias: &str,
    map: &BTreeMap<Value, Value>,
    key: &str,
) -> Result<Option<Vec<String>>, SemaError> {
    let Some(v) = map.get(&Value::keyword(key)) else {
        return Ok(None);
    };
    let items = v.as_seq().ok_or_else(|| {
        decl_error(
            alias,
            format!(":{key} must be a list of strings"),
            format!("e.g. :{key} [\"a\" \"b\"]"),
        )
    })?;
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let s = item.as_str().ok_or_else(|| {
            decl_error(
                alias,
                format!(
                    "every :{key} element must be a string, got {}",
                    item.type_name()
                ),
                format!(":{key} entries must all be strings"),
            )
        })?;
        out.push(s.to_string());
    }
    Ok(Some(out))
}

/// A `:key` map-of-string-to-string value (`:headers`/`:env`): empty vec when
/// absent. Both keys and values must be strings.
fn parse_string_pairs(
    alias: &str,
    map: &BTreeMap<Value, Value>,
    key: &str,
) -> Result<Vec<(String, String)>, SemaError> {
    let Some(v) = map.get(&Value::keyword(key)) else {
        return Ok(Vec::new());
    };
    let entries = v.as_map_ref().ok_or_else(|| {
        decl_error(
            alias,
            format!(":{key} must be a map of string to string"),
            format!(":{key} entries look like {{\"Header-Name\" \"value\"}}"),
        )
    })?;
    let mut out = Vec::with_capacity(entries.len());
    for (k, val) in entries.iter() {
        let key_str = k.as_str().ok_or_else(|| {
            decl_error(
                alias,
                format!(":{key} keys must be strings"),
                format!(":{key} entries look like {{\"Header-Name\" \"value\"}}"),
            )
        })?;
        let val_str = val.as_str().ok_or_else(|| {
            decl_error(
                alias,
                format!(":{key} value for {key_str:?} must be a string"),
                format!(":{key} entries look like {{\"Header-Name\" \"value\"}}"),
            )
        })?;
        out.push((key_str.to_string(), val_str.to_string()));
    }
    Ok(out)
}

fn parse_persist(alias: &str, spec_map: &BTreeMap<Value, Value>) -> Result<McpPersist, SemaError> {
    let Some(v) = spec_map.get(&Value::keyword("persist")) else {
        return Ok(McpPersist::default());
    };
    let kw = v.as_keyword().ok_or_else(|| {
        decl_error(
            alias,
            format!(
                ":persist must be one of :keyring, :workflow, :run, :none, got {}",
                v.type_name()
            ),
            "e.g. :persist :workflow",
        )
    })?;
    if !is_valid_persist_keyword(&kw) {
        return Err(decl_error(
            alias,
            format!(":persist value :{kw} is invalid (expected one of :keyring, :workflow, :run, :none)"),
            "e.g. :persist :workflow",
        ));
    }
    match kw.as_str() {
        "keyring" => Ok(McpPersist::Keyring),
        "workflow" => Ok(McpPersist::Workflow),
        "run" => Ok(McpPersist::Run),
        "none" => Ok(McpPersist::None),
        _ => unreachable!("is_valid_persist_keyword just confirmed membership"),
    }
}

// ── runtime resolver seam ────────────────────────────────────────────────────
//
// Crate-dependency law: sema-stdlib must NOT depend on sema-mcp. Everything
// that requires OAuth/connect/close knowledge is behind [`WorkflowMcpResolver`],
// implemented by the binary crate (`sema-lang`, `crates/sema/src/workflow_mcp.rs`)
// over `sema-mcp` and installed via [`set_workflow_mcp_resolver`]. `workflow/run`
// (`crates/sema-stdlib/src/workflow.rs`) is the ONLY caller of this seam — it
// never touches `sema-mcp` types directly, only the [`McpDecl`]s above and the
// opaque `Value` handles below. See `docs/plans/2026-06-24-workflow-mcp-auth.md` §3.

/// One authorized server's auth provenance, for the `auth.granted` journal
/// event. Redaction rule (plan §4): scopes + expiry only, NEVER token material.
#[derive(Debug, Clone, PartialEq)]
pub struct AuthGrant {
    pub scopes: Vec<String>,
    pub expires_at: Option<u64>,
    /// One of `"cached"`, `"refreshed"`, `"consented"`.
    pub source: String,
}

/// One declared server's resolution outcome. [`WorkflowMcpResolver::resolve`]
/// returns a `Vec` of these in the SAME (alias-sorted) order as the `decls` it
/// was given, so `workflow/run` can emit events in a deterministic, replayable
/// order (plan §7).
#[derive(Debug, Clone)]
pub enum ServerResolution {
    /// Connected — silently from a cached/refreshed token, from fresh consent,
    /// or because the server needs no auth at all. `handle` is an OPAQUE
    /// `Value` (an `mcp/connect`-shaped handle); this crate never interprets
    /// it, only stores it in the run's handle registry and hands it back to
    /// [`WorkflowMcpResolver::close`]. `auth` is `Some` only when an OAuth
    /// grant was actually involved (absent for stdio / open / bring-your-own
    /// `:headers` servers).
    Connected {
        alias: String,
        handle: Value,
        auth: Option<AuthGrant>,
    },
    /// No usable session could be found (or refreshed) for this server; the
    /// run gates here — the headless precursor of plan §3. `url` is the MCP
    /// server endpoint (for login guidance); `scopes`/`tools`/`persist` mirror
    /// the declaration, for the consent/manifest surface.
    NeedsAuth {
        alias: String,
        url: String,
        scopes: Vec<String>,
        tools: Vec<String>,
        persist: String,
    },
    /// The server could not be resolved for a reason OTHER than missing auth
    /// (bad config, network/process error, sandbox denial, a connect that
    /// otherwise rejected the handshake, …).
    Failed { alias: String, reason: String },
}

/// Implemented by the binary crate over `sema-mcp`; the sole crossing point
/// from the leaf runtime into MCP/OAuth territory.
pub trait WorkflowMcpResolver {
    /// Resolve every declared server for one run. `workflow` is the
    /// `defworkflow` name and `run_id` the active run's id — both are for the
    /// resolver's own bookkeeping (e.g. naming a `:persist :workflow`/`:run`
    /// directory), not interpreted by this crate.
    fn resolve(&self, decls: &[McpDecl], workflow: &str, run_id: &str) -> Vec<ServerResolution>;

    /// Best-effort close of every handle previously returned as `Connected`.
    /// MUST NOT panic or otherwise abort the caller — `workflow/run` calls
    /// this from its failure/cleanup paths, where a close error must never
    /// mask (or replace) the real outcome being reported.
    fn close(&self, handles: &[Value]);
}

thread_local! {
    /// The process' registered resolver, if any. `None` means "this build has
    /// no MCP resolver" — `workflow/run` turns that into a failed envelope
    /// with a hint, rather than panicking or silently skipping the gate.
    static RESOLVER: RefCell<Option<Rc<dyn WorkflowMcpResolver>>> = const { RefCell::new(None) };
}

/// Install the process' [`WorkflowMcpResolver`]. The binary crate (`sema-lang`)
/// does this alongside `sema_mcp::register_mcp_builtins`, so every path that can
/// run a workflow (REPL, `sema run`, `sema workflow run`) has one. Replaceable —
/// a test installs a fake resolver by calling this again; there is deliberately
/// no "already set" guard, since tests routinely swap it between cases.
pub fn set_workflow_mcp_resolver(r: Rc<dyn WorkflowMcpResolver>) {
    RESOLVER.with(|slot| *slot.borrow_mut() = Some(r));
}

/// Clear the registered resolver (test teardown / isolating one test's fake
/// resolver from the next).
pub fn clear_workflow_mcp_resolver() {
    RESOLVER.with(|slot| *slot.borrow_mut() = None);
}

/// The active resolver on this thread, if one is registered.
pub fn workflow_mcp_resolver() -> Option<Rc<dyn WorkflowMcpResolver>> {
    RESOLVER.with(|slot| slot.borrow().clone())
}

// ── tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn meta_of(src: &str) -> Value {
        sema_reader::read(src).expect("valid sema literal")
    }

    fn err_of(src: &str) -> SemaError {
        declared_mcp(&meta_of(src)).expect_err("expected a validation error")
    }

    // ── absence / defaults ──────────────────────────────────────────────

    #[test]
    fn absent_mcp_key_is_empty() {
        let decls = declared_mcp(&meta_of("{:budget {:usd 1.0}}")).unwrap();
        assert!(decls.is_empty());
    }

    #[test]
    fn non_map_meta_is_empty() {
        // Malformed meta is E-WF-META's problem, not this function's.
        let decls = declared_mcp(&meta_of("[:not :a :map]")).unwrap();
        assert!(decls.is_empty());
    }

    #[test]
    fn persist_defaults_to_workflow_when_absent() {
        let decls = declared_mcp(&meta_of(r#"{:mcp {fs {:command "npx"}}}"#)).unwrap();
        assert_eq!(decls[0].persist, McpPersist::Workflow);
    }

    #[test]
    fn tools_defaults_to_empty_when_absent() {
        let decls = declared_mcp(&meta_of(r#"{:mcp {fs {:command "npx"}}}"#)).unwrap();
        assert!(decls[0].tools.is_empty());
    }

    #[test]
    fn auth_defaults_to_none_when_absent() {
        let decls = declared_mcp(&meta_of(
            r#"{:mcp {asana {:url "https://mcp.asana.com/mcp"}}}"#,
        ))
        .unwrap();
        assert!(decls[0].auth.is_none());
    }

    // ── the plan's example, verbatim ────────────────────────────────────

    #[test]
    fn happy_path_matches_the_plan_example() {
        let src = r#"
            {:mcp {asana {:url "https://mcp.asana.com/mcp"
                          :auth {:scopes ["default"]}
                          :tools ["create_task" "search_tasks"]
                          :persist :workflow}
                   fs    {:command "npx" :args ["-y" "@modelcontextprotocol/server-filesystem" "."]}}}
        "#;
        let decls = declared_mcp(&meta_of(src)).unwrap();
        assert_eq!(decls.len(), 2);

        // Deterministic order: sorted by alias, so "asana" precedes "fs".
        assert_eq!(decls[0].alias, "asana");
        assert_eq!(decls[1].alias, "fs");

        let asana = &decls[0];
        assert_eq!(
            asana.spec,
            McpSpecDecl::Http {
                url: "https://mcp.asana.com/mcp".to_string(),
                headers: Vec::new(),
            }
        );
        assert_eq!(
            asana.auth,
            Some(McpAuthDecl {
                scopes: vec!["default".to_string()],
                client_id: None,
            })
        );
        assert_eq!(
            asana.tools,
            vec!["create_task".to_string(), "search_tasks".to_string()]
        );
        assert_eq!(asana.persist, McpPersist::Workflow);

        let fs = &decls[1];
        assert_eq!(
            fs.spec,
            McpSpecDecl::Stdio {
                command: "npx".to_string(),
                args: vec![
                    "-y".to_string(),
                    "@modelcontextprotocol/server-filesystem".to_string(),
                    ".".to_string(),
                ],
                env: Vec::new(),
                cwd: None,
            }
        );
        assert!(fs.auth.is_none());
        assert!(fs.tools.is_empty());
        assert_eq!(fs.persist, McpPersist::Workflow);
    }

    // ── top-level shape ──────────────────────────────────────────────────

    #[test]
    fn mcp_not_a_map_errors() {
        let e = err_of(r#"{:mcp [1 2 3]}"#);
        assert!(e.to_string().contains(":mcp must be a map"), "{e}");
    }

    #[test]
    fn keyword_alias_errors() {
        let e = err_of(r#"{:mcp {:asana {:url "https://x"}}}"#);
        assert!(e.to_string().contains("must be a bare symbol"), "{e}");
    }

    #[test]
    fn string_alias_errors() {
        let e = err_of(r#"{:mcp {"asana" {:url "https://x"}}}"#);
        assert!(e.to_string().contains("must be a bare symbol"), "{e}");
    }

    #[test]
    fn spec_not_a_map_errors() {
        let e = err_of(r#"{:mcp {asana "https://x"}}"#);
        assert!(
            e.to_string().contains(":mcp asana: spec must be a map"),
            "{e}"
        );
    }

    // ── transport ─────────────────────────────────────────────────────────

    #[test]
    fn missing_url_and_command_errors() {
        let e = err_of(r#"{:mcp {asana {:persist :run}}}"#);
        assert!(e.to_string().contains("missing :url or :command"), "{e}");
    }

    #[test]
    fn both_url_and_command_errors() {
        let e = err_of(r#"{:mcp {asana {:url "https://x" :command "npx"}}}"#);
        assert!(e.to_string().contains("both :url and :command"), "{e}");
    }

    #[test]
    fn non_string_url_errors() {
        let e = err_of(r#"{:mcp {asana {:url 42}}}"#);
        assert!(e.to_string().contains(":url must be a string"), "{e}");
    }

    #[test]
    fn non_string_command_errors() {
        let e = err_of(r#"{:mcp {fs {:command 42}}}"#);
        assert!(e.to_string().contains(":command must be a string"), "{e}");
    }

    // ── :args / :headers / :env / :cwd ──────────────────────────────────

    #[test]
    fn non_string_args_element_errors() {
        let e = err_of(r#"{:mcp {fs {:command "npx" :args ["-y" 42]}}}"#);
        assert!(
            e.to_string()
                .contains("every :args element must be a string"),
            "{e}"
        );
    }

    #[test]
    fn args_not_a_list_errors() {
        let e = err_of(r#"{:mcp {fs {:command "npx" :args "-y"}}}"#);
        assert!(e.to_string().contains(":args must be a list"), "{e}");
    }

    #[test]
    fn headers_value_must_be_string() {
        let e = err_of(r#"{:mcp {asana {:url "https://x" :headers {"Authorization" 42}}}}"#);
        assert!(e.to_string().contains(":headers value"), "{e}");
    }

    #[test]
    fn headers_key_must_be_string() {
        let e = err_of(r#"{:mcp {asana {:url "https://x" :headers {42 "Bearer t"}}}}"#);
        assert!(
            e.to_string().contains(":headers keys must be strings"),
            "{e}"
        );
    }

    #[test]
    fn headers_parses_string_pairs() {
        let decls = declared_mcp(&meta_of(
            r#"{:mcp {asana {:url "https://x" :headers {"Authorization" "Bearer t"}}}}"#,
        ))
        .unwrap();
        assert_eq!(
            decls[0].spec,
            McpSpecDecl::Http {
                url: "https://x".to_string(),
                headers: vec![("Authorization".to_string(), "Bearer t".to_string())],
            }
        );
    }

    #[test]
    fn env_value_must_be_string() {
        let e = err_of(r#"{:mcp {fs {:command "npx" :env {"TOKEN" 1}}}}"#);
        assert!(e.to_string().contains(":env value"), "{e}");
    }

    #[test]
    fn env_key_must_be_string() {
        let e = err_of(r#"{:mcp {fs {:command "npx" :env {42 "value"}}}}"#);
        assert!(e.to_string().contains(":env keys must be strings"), "{e}");
    }

    #[test]
    fn cwd_must_be_a_string() {
        let e = err_of(r#"{:mcp {fs {:command "npx" :cwd 1}}}"#);
        assert!(e.to_string().contains(":cwd must be a string"), "{e}");
    }

    // ── :auth ─────────────────────────────────────────────────────────────

    #[test]
    fn auth_on_stdio_spec_errors() {
        let e = err_of(r#"{:mcp {fs {:command "npx" :auth {:scopes ["default"]}}}}"#);
        assert!(
            e.to_string().contains(":auth is not valid on a stdio"),
            "{e}"
        );
    }

    #[test]
    fn auth_not_a_map_errors() {
        let e = err_of(r#"{:mcp {asana {:url "https://x" :auth "oauth"}}}"#);
        assert!(e.to_string().contains(":auth must be a map"), "{e}");
    }

    #[test]
    fn auth_scopes_must_be_strings() {
        let e = err_of(r#"{:mcp {asana {:url "https://x" :auth {:scopes [1]}}}}"#);
        assert!(
            e.to_string()
                .contains("every :scopes element must be a string"),
            "{e}"
        );
    }

    #[test]
    fn auth_client_id_must_be_a_string() {
        let e = err_of(r#"{:mcp {asana {:url "https://x" :auth {:client-id 1}}}}"#);
        assert!(e.to_string().contains(":client-id must be a string"), "{e}");
    }

    // ── :tools ────────────────────────────────────────────────────────────

    #[test]
    fn tools_not_a_list_errors() {
        let e = err_of(r#"{:mcp {asana {:url "https://x" :tools "create_task"}}}"#);
        assert!(e.to_string().contains(":tools must be a list"), "{e}");
    }

    #[test]
    fn tools_non_string_element_errors() {
        let e = err_of(r#"{:mcp {asana {:url "https://x" :tools [1]}}}"#);
        assert!(
            e.to_string()
                .contains("every :tools element must be a string"),
            "{e}"
        );
    }

    // ── :persist ──────────────────────────────────────────────────────────

    #[test]
    fn persist_non_keyword_errors_listing_options() {
        let e = err_of(r#"{:mcp {asana {:url "https://x" :persist "workflow"}}}"#);
        let msg = e.to_string();
        assert!(
            msg.contains(":keyring")
                && msg.contains(":workflow")
                && msg.contains(":run")
                && msg.contains(":none"),
            "{msg}"
        );
    }

    #[test]
    fn persist_invalid_keyword_errors_listing_options() {
        let e = err_of(r#"{:mcp {asana {:url "https://x" :persist :bogus}}}"#);
        let msg = e.to_string();
        assert!(
            msg.contains(":keyring")
                && msg.contains(":workflow")
                && msg.contains(":run")
                && msg.contains(":none"),
            "{msg}"
        );
    }

    #[test]
    fn persist_accepts_all_four_keywords() {
        for (kw, expected) in [
            (":keyring", McpPersist::Keyring),
            (":workflow", McpPersist::Workflow),
            (":run", McpPersist::Run),
            (":none", McpPersist::None),
        ] {
            let src = format!(r#"{{:mcp {{asana {{:url "https://x" :persist {kw}}}}}}}"#);
            let decls = declared_mcp(&meta_of(&src)).unwrap();
            assert_eq!(decls[0].persist, expected, "persist {kw}");
        }
    }

    // ── ordering ──────────────────────────────────────────────────────────

    #[test]
    fn declarations_are_sorted_by_alias() {
        let src = r#"{:mcp {zebra {:command "z"} apple {:command "a"} mango {:command "m"}}}"#;
        let decls = declared_mcp(&meta_of(src)).unwrap();
        let aliases: Vec<&str> = decls.iter().map(|d| d.alias.as_str()).collect();
        assert_eq!(aliases, vec!["apple", "mango", "zebra"]);
    }

    // ── error hints name the alias ───────────────────────────────────────

    #[test]
    fn error_hint_is_present() {
        let e = err_of(r#"{:mcp {asana {:url 42}}}"#);
        assert!(e.hint().is_some(), "expected a hint on the error");
        assert!(
            e.to_string().contains("asana"),
            "error should name the alias"
        );
    }

    // ── resolver seam ────────────────────────────────────────────────────

    struct StubResolver;
    impl WorkflowMcpResolver for StubResolver {
        fn resolve(
            &self,
            _decls: &[McpDecl],
            _workflow: &str,
            _run_id: &str,
        ) -> Vec<ServerResolution> {
            Vec::new()
        }
        fn close(&self, _handles: &[Value]) {}
    }

    // These three tests share the process-global thread-local RESOLVER slot;
    // each leaves it cleared on exit so it can't leak into another test in
    // this binary (cargo runs unit tests within one binary on multiple
    // threads by default, but the thread-local means each test thread has its
    // own slot anyway — clearing is just good hygiene, not a race guard).

    #[test]
    fn resolver_slot_starts_empty() {
        clear_workflow_mcp_resolver();
        assert!(workflow_mcp_resolver().is_none());
    }

    #[test]
    fn set_workflow_mcp_resolver_installs_it() {
        set_workflow_mcp_resolver(Rc::new(StubResolver));
        assert!(workflow_mcp_resolver().is_some());
        clear_workflow_mcp_resolver();
    }

    #[test]
    fn set_workflow_mcp_resolver_is_replaceable() {
        set_workflow_mcp_resolver(Rc::new(StubResolver));
        let first = workflow_mcp_resolver().unwrap();
        set_workflow_mcp_resolver(Rc::new(StubResolver));
        let second = workflow_mcp_resolver().unwrap();
        // Replacing installs a genuinely different Rc (not a no-op / same pointer).
        assert!(!Rc::ptr_eq(&first, &second));
        clear_workflow_mcp_resolver();
    }
}
