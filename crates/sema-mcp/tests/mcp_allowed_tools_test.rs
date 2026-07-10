//! `:tools` manifest (least-privilege) enforcement on a connection made via
//! `connect_from_config` (`docs/plans/2026-06-24-workflow-mcp-auth.md` §2). A
//! stdio server exposing two tools (`tool_a`, `tool_b`) is the oracle: a
//! connection restricted to `["tool_a"]` must reject calls to `tool_b`, filter
//! it out of `mcp/tools`, and never wrap it in `mcp/tools->sema` — while a
//! connection with `allowed_tools: None` stays fully unrestricted (today's
//! `mcp/connect` behavior).
//!
//! `connect_from_config` registers the connection in the same thread-local
//! table `mcp/call`/`mcp/tools`/`mcp/tools->sema` read, so the handle it
//! returns is bound into a `sema_eval::Interpreter`'s env and driven from
//! Sema, exactly like a real workflow resolver would do with a declared
//! `:mcp` alias.

use std::collections::BTreeMap;

use sema_core::{Sandbox, Value};
use sema_eval::Interpreter;
use sema_mcp::builtins::{connect_from_config, ConnectOpts};

/// Two tools: `tool_a` and `tool_b`, each echoing which one was called.
const SERVER: &str = r#"
import json, sys

initialized = False

def send(m):
    sys.stdout.write(json.dumps(m) + "\n")
    sys.stdout.flush()

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    r = json.loads(line)
    method = r.get("method")
    rid = r.get("id")
    if rid is None:
        if method == "notifications/initialized":
            initialized = True
        continue
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": rid, "result": {
            "protocolVersion": "2025-11-25", "capabilities": {},
            "serverInfo": {"name": "allow-tools-test", "version": "1"}}})
    elif method == "tools/list":
        send({"jsonrpc": "2.0", "id": rid, "result": {"tools": [
            {"name": "tool_a", "description": "Allowed",
             "inputSchema": {"type": "object", "properties": {}}},
            {"name": "tool_b", "description": "Not allowed",
             "inputSchema": {"type": "object", "properties": {}}},
        ]}})
    elif method == "tools/call":
        name = r.get("params", {}).get("name")
        send({"jsonrpc": "2.0", "id": rid, "result": {
            "content": [{"type": "text", "text": "called-%s" % name}], "isError": False}})
    else:
        send({"jsonrpc": "2.0", "id": rid, "error": {"code": -32601, "message": "no"}})
"#;

fn stdio_config() -> Value {
    let mut map = BTreeMap::new();
    map.insert(Value::keyword("command"), Value::string("python3"));
    map.insert(
        Value::keyword("args"),
        Value::list(vec![Value::string("-c"), Value::string(SERVER)]),
    );
    Value::map(map)
}

fn new_interp() -> Interpreter {
    let interp = Interpreter::new();
    sema_mcp::register_mcp_builtins(&interp.global_env, &Sandbox::allow_all());
    interp
}

/// Connect with the given `:tools` manifest and bind the handle to `server`
/// in the interpreter's env, mirroring how a workflow resolver would bind a
/// declared alias.
fn connect(interp: &Interpreter, allowed_tools: Option<Vec<String>>) {
    let opts = ConnectOpts {
        interactive_auth: true,
        allowed_tools,
    };
    let handle = connect_from_config(&stdio_config(), opts).expect("stdio connect must succeed");
    interp.global_env.set_str("server", handle);
}

fn tool_names(list: &Value) -> Vec<String> {
    list.as_seq()
        .expect("mcp/tools returns a list")
        .iter()
        .map(|item| {
            item.as_map_ref()
                .and_then(|m| m.get(&Value::keyword("name")))
                .and_then(Value::as_str)
                .expect("each tool entry has a :name")
                .to_string()
        })
        .collect()
}

#[test]
fn undeclared_tool_call_errors_with_hint_naming_the_tool_and_manifest() {
    let interp = new_interp();
    connect(&interp, Some(vec!["tool_a".to_string()]));

    let err = interp
        .eval_str(r#"(mcp/call server "tool_b" {})"#)
        .expect_err("tool_b is not in the :tools manifest");
    let msg = err.to_string();
    assert!(msg.contains("tool_b"), "error should name the tool: {msg}");
    assert!(
        msg.contains(":tools manifest"),
        "error should reference the :tools manifest: {msg}"
    );
    assert_eq!(
        err.hint(),
        Some("declared in the workflow's :mcp :tools manifest; add it there to allow it")
    );

    interp.eval_str(r#"(mcp/close server)"#).ok();
}

#[test]
fn declared_tool_call_succeeds() {
    let interp = new_interp();
    connect(&interp, Some(vec!["tool_a".to_string()]));

    let result = interp
        .eval_str(r#"(mcp/call server "tool_a" {})"#)
        .expect("tool_a is declared");
    assert_eq!(result.as_str(), Some("called-tool_a"));

    interp.eval_str(r#"(mcp/close server)"#).ok();
}

#[test]
fn mcp_tools_is_filtered_to_the_allowed_manifest() {
    let interp = new_interp();
    connect(&interp, Some(vec!["tool_a".to_string()]));

    let tools = interp.eval_str("(mcp/tools server)").unwrap();
    assert_eq!(tool_names(&tools), vec!["tool_a".to_string()]);

    interp.eval_str(r#"(mcp/close server)"#).ok();
}

#[test]
fn mcp_tools_to_sema_wraps_only_the_allowed_manifest() {
    let interp = new_interp();
    connect(&interp, Some(vec!["tool_a".to_string()]));

    let defs = interp.eval_str("(mcp/tools->sema server)").unwrap();
    let defs = defs.as_seq().unwrap();
    assert_eq!(defs.len(), 1, "only tool_a should be wrapped");
    let td = defs[0]
        .as_tool_def_rc()
        .expect("mcp/tools->sema returns tool-def values");
    assert_eq!(td.name, "tool_a");

    interp.eval_str(r#"(mcp/close server)"#).ok();
}

#[test]
fn none_allowed_tools_is_unrestricted() {
    let interp = new_interp();
    connect(&interp, None);

    let tools = interp.eval_str("(mcp/tools server)").unwrap();
    let mut names = tool_names(&tools);
    names.sort();
    assert_eq!(names, vec!["tool_a".to_string(), "tool_b".to_string()]);

    let result = interp
        .eval_str(r#"(mcp/call server "tool_b" {})"#)
        .expect("unrestricted connection allows every server tool");
    assert_eq!(result.as_str(), Some("called-tool_b"));

    interp.eval_str(r#"(mcp/close server)"#).ok();
}

#[test]
fn empty_allowed_tools_list_permits_nothing() {
    let interp = new_interp();
    connect(&interp, Some(Vec::new()));

    let tools = interp.eval_str("(mcp/tools server)").unwrap();
    assert!(tool_names(&tools).is_empty());

    let err = interp
        .eval_str(r#"(mcp/call server "tool_a" {})"#)
        .expect_err("an empty :tools manifest allows nothing");
    assert!(err.to_string().contains("tool_a"));

    interp.eval_str(r#"(mcp/close server)"#).ok();
}
