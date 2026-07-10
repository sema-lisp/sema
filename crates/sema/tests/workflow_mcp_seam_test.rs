//! Seam tests for the implicit `:mcp` auth-resolution step in `workflow/run`
//! (docs/plans/2026-06-24-workflow-mcp-auth.md §3), driven against a FAKE
//! `WorkflowMcpResolver` — no network, no real MCP connections. These prove the
//! wiring in `crates/sema-stdlib/src/workflow.rs`: per-resolution event emission
//! order, envelope shapes, the `defworkflow` alias-binding macro, and
//! close-called-exactly-once. The REAL resolver (over `sema-mcp`) is exercised
//! end-to-end in `workflow_mcp_e2e_test.rs`.

mod workflow_common;
use workflow_common as wc;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use sema_core::Value;
use sema_llm::fake::FakeProvider;
use sema_stdlib::workflow_mcp::{
    clear_workflow_mcp_resolver, set_workflow_mcp_resolver, AuthGrant, McpDecl, ServerResolution,
    WorkflowMcpResolver,
};

fn fake_llm() -> FakeProvider {
    FakeProvider::builder("fake").build()
}

/// A scripted resolver: `resolve` returns a fixed `Vec<ServerResolution>`
/// regardless of the declared aliases (each test's workflow source declares the
/// aliases the script expects); `close` counts calls and records the handles it
/// was given, so tests can assert "closed exactly once" and "closed the right
/// handles" without any real MCP connection.
struct FakeResolver {
    resolutions: Vec<ServerResolution>,
    close_calls: Rc<Cell<u32>>,
    closed_handles: Rc<RefCell<Vec<Value>>>,
}

impl WorkflowMcpResolver for FakeResolver {
    fn resolve(&self, _decls: &[McpDecl], _workflow: &str, _run_id: &str) -> Vec<ServerResolution> {
        self.resolutions.clone()
    }

    fn close(&self, handles: &[Value]) {
        self.close_calls.set(self.close_calls.get() + 1);
        self.closed_handles.borrow_mut().extend_from_slice(handles);
    }
}

fn install_fake(resolutions: Vec<ServerResolution>) -> (Rc<Cell<u32>>, Rc<RefCell<Vec<Value>>>) {
    let close_calls = Rc::new(Cell::new(0));
    let closed_handles = Rc::new(RefCell::new(Vec::new()));
    set_workflow_mcp_resolver(Rc::new(FakeResolver {
        resolutions,
        close_calls: close_calls.clone(),
        closed_handles: closed_handles.clone(),
    }));
    (close_calls, closed_handles)
}

// ── all declared servers connect silently / with consent ──────────────────────

#[test]
fn all_satisfied_binds_aliases_emits_granted_in_order_closes_once() {
    let (close_calls, closed_handles) = install_fake(vec![
        ServerResolution::Connected {
            alias: "asana".to_string(),
            handle: Value::string("fake-asana-handle"),
            auth: Some(AuthGrant {
                scopes: vec!["default".to_string()],
                expires_at: Some(1_800_000_000),
                source: "consented".to_string(),
            }),
        },
        ServerResolution::Connected {
            alias: "fs".to_string(),
            handle: Value::string("fake-fs-handle"),
            auth: None,
        },
    ]);

    let src = r#"
        (defworkflow triage
          "test"
          {:budget {:usd 1.0}
           :mcp {asana {:url "https://mcp.asana.com/mcp" :auth {:scopes ["default"]}}
                 fs    {:command "npx" :args ["-y" "srv"]}}}
          (phase "Use")
          {:status :success :asana-handle asana :fs-handle fs})
    "#;

    let out = wc::run_once(src, fake_llm(), "mcp-all-satisfied");

    // Connected + auth: Some(g) -> exactly one auth.granted (for asana); Connected
    // + auth: None (fs) emits nothing.
    let granted = wc::events_of(&out.events, "auth.granted");
    assert_eq!(granted.len(), 1, "fs (auth: None) must not emit an event");
    assert_eq!(granted[0]["server"], "asana");
    assert_eq!(granted[0]["source"], "consented");
    assert_eq!(granted[0]["scopes"][0], "default");
    assert_eq!(granted[0]["expires_at"], 1_800_000_000);
    // Wire shape (field-by-field: the harness parses each journal line through
    // serde_json::Value, which — since this workspace builds serde_json WITHOUT
    // preserve_order — re-sorts object keys alphabetically; event.rs's own unit
    // tests pin the ACTUAL on-the-wire declaration order via raw string
    // comparison. Here we confirm the right DATA reaches the journal end-to-end
    // through workflow/run's real wiring).
    assert_eq!(granted[0]["event"], "auth.granted");

    assert!(wc::events_of(&out.events, "auth.required").is_empty());
    assert!(wc::events_of(&out.events, "auth.failed").is_empty());

    // The body ran and both aliases were bound to their resolved handles.
    assert_eq!(out.result["status"], "success");
    assert_eq!(out.result["asana-handle"], "fake-asana-handle");
    assert_eq!(out.result["fs-handle"], "fake-fs-handle");

    let ended = wc::events_of(&out.events, "run.ended");
    assert_eq!(ended.len(), 1);
    assert_eq!(ended[0]["status"], "success");

    // close() called EXACTLY once, with both handles.
    assert_eq!(close_calls.get(), 1, "close must be called exactly once");
    let handles: Vec<String> = closed_handles
        .borrow()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert_eq!(handles.len(), 2);
    assert!(handles.contains(&"fake-asana-handle".to_string()));
    assert!(handles.contains(&"fake-fs-handle".to_string()));

    clear_workflow_mcp_resolver();
}

// ── one server needs auth: gate, body never runs ───────────────────────────────

#[test]
fn one_needs_auth_gates_before_body_and_closes_connected_handles() {
    let (close_calls, closed_handles) = install_fake(vec![
        ServerResolution::Connected {
            alias: "asana".to_string(),
            handle: Value::string("fake-asana-handle"),
            auth: Some(AuthGrant {
                scopes: vec![],
                expires_at: None,
                source: "cached".to_string(),
            }),
        },
        ServerResolution::NeedsAuth {
            alias: "linear".to_string(),
            url: "https://mcp.linear.app/mcp".to_string(),
            scopes: vec!["read".to_string()],
            tools: vec!["list_issues".to_string()],
            persist: "workflow".to_string(),
        },
    ]);

    let src = r#"
        (defworkflow triage
          "test"
          {:budget {:usd 1.0}
           :mcp {asana  {:url "https://mcp.asana.com/mcp" :auth {:scopes []}}
                 linear {:url "https://mcp.linear.app/mcp"
                         :auth {:scopes ["read"]}
                         :tools ["list_issues"]
                         :persist :workflow}}}
          (phase "Use")
          (checkpoint :ran #t)
          {:status :success})
    "#;

    let out = wc::run_once(src, fake_llm(), "mcp-needs-auth");

    // auth.granted for asana still fires (per-resolution, regardless of overall
    // outcome), auth.required for linear.
    let granted = wc::events_of(&out.events, "auth.granted");
    assert_eq!(granted.len(), 1);
    assert_eq!(granted[0]["server"], "asana");

    let required = wc::events_of(&out.events, "auth.required");
    assert_eq!(required.len(), 1);
    assert_eq!(required[0]["server"], "linear");
    assert_eq!(required[0]["scopes"][0], "read");
    assert_eq!(required[0]["tools"][0], "list_issues");
    assert_eq!(required[0]["persist"], "workflow");

    // Body NEVER ran: no phase.started / checkpoint events at all.
    assert!(
        wc::events_of(&out.events, "phase.started").is_empty(),
        "body must not run on a needs-auth gate"
    );
    assert!(wc::events_of(&out.events, "checkpoint").is_empty());

    // Envelope: plan-exact :servers + additive :auth detail vector.
    assert_eq!(out.result["status"], "needs-auth");
    assert_eq!(out.result["servers"], serde_json::json!(["linear"]));
    assert_eq!(out.result["auth"][0]["server"], "linear");
    assert_eq!(out.result["auth"][0]["url"], "https://mcp.linear.app/mcp");
    assert_eq!(out.result["auth"][0]["persist"], "workflow");

    let ended = wc::events_of(&out.events, "run.ended");
    assert_eq!(ended.len(), 1);
    assert_eq!(ended[0]["status"], "needs-auth");
    assert_eq!(ended[0]["reason"], "authentication required");

    // The already-Connected asana handle is closed even though the run gates.
    assert_eq!(close_calls.get(), 1, "close must be called exactly once");
    let handles = closed_handles.borrow();
    assert_eq!(handles.len(), 1);
    assert_eq!(handles[0].as_str().unwrap(), "fake-asana-handle");

    clear_workflow_mcp_resolver();
}

// ── one server fails resolution: failed envelope, body never runs ─────────────

#[test]
fn one_failed_fails_run_before_body_and_closes_connected_handles() {
    let (close_calls, closed_handles) = install_fake(vec![
        ServerResolution::Connected {
            alias: "asana".to_string(),
            handle: Value::string("fake-asana-handle"),
            auth: None,
        },
        ServerResolution::Failed {
            alias: "broken".to_string(),
            reason: "connection refused".to_string(),
        },
    ]);

    let src = r#"
        (defworkflow triage
          "test"
          {:budget {:usd 1.0}
           :mcp {asana  {:url "https://mcp.asana.com/mcp"}
                 broken {:command "does-not-exist"}}}
          (phase "Use")
          (checkpoint :ran #t)
          {:status :success})
    "#;

    let out = wc::run_once(src, fake_llm(), "mcp-failed");

    let failed = wc::events_of(&out.events, "auth.failed");
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0]["server"], "broken");
    assert_eq!(failed[0]["reason"], "connection refused");

    assert!(
        wc::events_of(&out.events, "phase.started").is_empty(),
        "body must not run when any server fails resolution"
    );

    assert_eq!(out.result["status"], "failed");
    let msg = out.result["error"].as_str().unwrap();
    assert!(msg.contains("broken"), "{msg}");
    assert!(msg.contains("connection refused"), "{msg}");

    let ended = wc::events_of(&out.events, "run.ended");
    assert_eq!(ended[0]["status"], "failed");
    assert_eq!(ended[0]["reason"], "mcp resolution failed");

    assert_eq!(close_calls.get(), 1, "close must be called exactly once");
    let handles = closed_handles.borrow();
    assert_eq!(handles.len(), 1);
    assert_eq!(handles[0].as_str().unwrap(), "fake-asana-handle");

    clear_workflow_mcp_resolver();
}

// ── no resolver registered ─────────────────────────────────────────────────────

#[test]
fn decls_with_no_resolver_registered_fails_with_hint() {
    clear_workflow_mcp_resolver();

    let src = r#"
        (defworkflow triage
          "test"
          {:budget {:usd 1.0}
           :mcp {asana {:url "https://mcp.asana.com/mcp"}}}
          (phase "Use")
          (checkpoint :ran #t)
          {:status :success})
    "#;

    let out = wc::run_once(src, fake_llm(), "mcp-no-resolver");

    assert!(
        wc::events_of(&out.events, "phase.started").is_empty(),
        "body must not run without a resolver"
    );
    assert_eq!(out.result["status"], "failed");
    let msg = out.result["error"].as_str().unwrap();
    assert!(
        msg.contains("no MCP resolver"),
        "expected the no-resolver hint, got: {msg}"
    );

    let ended = wc::events_of(&out.events, "run.ended");
    assert_eq!(ended[0]["status"], "failed");
    assert_eq!(ended[0]["reason"], "mcp resolution failed");
}

// ── an :mcp parse error fails the run before the body, too ─────────────────────

#[test]
fn invalid_mcp_declaration_fails_before_body_runs() {
    // The resolver doesn't matter here — parsing fails before resolve() is ever
    // called — but clear it so this test doesn't depend on suite ordering.
    clear_workflow_mcp_resolver();

    let src = r#"
        (defworkflow triage
          "test"
          {:budget {:usd 1.0}
           :mcp {asana {:command "npx" :auth {:scopes ["default"]}}}}
          (phase "Use")
          (checkpoint :ran #t)
          {:status :success})
    "#;

    let out = wc::run_once(src, fake_llm(), "mcp-invalid-decl");

    assert!(wc::events_of(&out.events, "phase.started").is_empty());
    assert_eq!(out.result["status"], "failed");
    let msg = out.result["error"].as_str().unwrap();
    assert!(
        msg.contains("auth is not valid on a stdio") || msg.contains(":auth"),
        "{msg}"
    );

    let ended = wc::events_of(&out.events, "run.ended");
    assert_eq!(ended[0]["status"], "failed");
    assert_eq!(ended[0]["reason"], "mcp declaration invalid");
}

// ── no :mcp declared: today's behavior, resolver is never even consulted ──────

#[test]
fn workflow_without_mcp_never_touches_the_resolver() {
    // A resolver that panics if resolve()/close() is ever called — proves the
    // no-:mcp path takes NONE of the new code (byte-identical requirement).
    struct PanicResolver;
    impl WorkflowMcpResolver for PanicResolver {
        fn resolve(&self, _: &[McpDecl], _: &str, _: &str) -> Vec<ServerResolution> {
            panic!("resolve() must not be called for a workflow with no :mcp");
        }
        fn close(&self, _: &[Value]) {
            panic!("close() must not be called for a workflow with no :mcp");
        }
    }
    set_workflow_mcp_resolver(Rc::new(PanicResolver));

    let src = r#"
        (defworkflow plain
          "no mcp here"
          {:budget {:usd 1.0}}
          (phase "Work")
          (checkpoint :x 1)
          {:status :success :x (checkpoint :x)})
    "#;

    let out = wc::run_once(src, fake_llm(), "mcp-absent");

    assert_eq!(out.result["status"], "success");
    assert_eq!(out.result["x"], 1);
    assert!(wc::events_of(&out.events, "auth.required").is_empty());
    assert!(wc::events_of(&out.events, "auth.granted").is_empty());
    assert!(wc::events_of(&out.events, "auth.failed").is_empty());

    clear_workflow_mcp_resolver();
}
