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

use crate::client::{McpClient, McpClientConfig};

thread_local! {
    // Keep MCP connections in a thread-local map so each Sema evaluator can own its own
    // client state without introducing cross-thread sharing.
    static CONNECTIONS: RefCell<HashMap<String, Rc<RefCell<McpConnection>>>> =
        RefCell::new(HashMap::new());
    // Reuse one current-thread Tokio runtime for all MCP builtins in this evaluator context.
    static TOKIO_RT: RefCell<Option<Runtime>> = const { RefCell::new(None) };
}

static HANDLE_COUNTER: AtomicU64 = AtomicU64::new(1);

struct McpConnection {
    client: McpClient,
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

/// Register a builtin that is refused unless `cap` is granted (mirrors
/// `sema_stdlib`'s `register_fn_gated`). Unrestricted sandboxes skip the check.
fn register_fn_gated(
    env: &Env,
    sandbox: &Sandbox,
    cap: Caps,
    name: &str,
    f: impl Fn(&[Value]) -> Result<Value, SemaError> + 'static,
) {
    if sandbox.is_unrestricted() {
        register_fn(env, name, f);
    } else {
        let sandbox = sandbox.clone();
        let fn_name = name.to_string();
        register_fn(env, name, move |args| {
            sandbox.check(cap, &fn_name)?;
            f(args)
        });
    }
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

fn call_tool_via_connection(
    handle: &str,
    tool_name: &str,
    arguments_json: serde_json::Value,
) -> Result<serde_json::Value, SemaError> {
    let connection = lookup_connection(handle)?;
    let mut connection = connection.borrow_mut();
    block_on(connection.client.call_tool(tool_name, arguments_json))
        .map_err(|err| SemaError::eval(format!("mcp/call: {err}")))
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
    register_fn_gated(env, sandbox, Caps::PROCESS, "mcp/connect", |args| {
        let config_json = config_to_json(args)?;
        let command = config_json
            .get("command")
            .and_then(|value| value.as_str())
            .ok_or_else(|| {
                SemaError::eval("mcp/connect requires a :command entry for stdio transport")
                    .with_hint("use {:command \"python3\" :args [\"-c\" \"script\"]}")
            })?;
        let args_vec = config_json
            .get("args")
            .and_then(|value| value.as_array())
            .map(|values| {
                values
                    .iter()
                    .filter_map(|value| value.as_str().map(str::to_string))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let env_map = config_json
            .get("env")
            .and_then(|value| value.as_object())
            .map(|object| {
                object
                    .iter()
                    .filter_map(|(key, value)| {
                        value.as_str().map(|s| (key.to_string(), s.to_string()))
                    })
                    .collect::<HashMap<_, _>>()
            })
            .unwrap_or_default();
        let cwd = config_json
            .get("cwd")
            .and_then(|value| value.as_str())
            .map(std::path::PathBuf::from);

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

        let handle = next_handle();
        let connection = Rc::new(RefCell::new(McpConnection { client }));
        CONNECTIONS.with(|connections| {
            connections.borrow_mut().insert(handle.clone(), connection);
        });
        Ok(Value::string(&handle))
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
