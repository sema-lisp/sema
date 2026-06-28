use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use sema_core::{check_arity, Env, NativeFn, SemaError, ToolDefinition, Value};
use tokio::runtime::Runtime;

use crate::client::{McpClient, McpClientConfig};

thread_local! {
    static CONNECTIONS: RefCell<HashMap<String, Rc<RefCell<McpConnection>>>> =
        RefCell::new(HashMap::new());
    static TOKIO_RT: RefCell<Option<Runtime>> = RefCell::new(None);
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

fn extract_config_map(args: &[Value]) -> Result<serde_json::Value, SemaError> {
    check_arity!(args, "mcp/connect", 1);
    let config = &args[0];
    let map = config.as_map_ref().ok_or_else(|| {
        SemaError::type_error("map", config.type_name())
            .with_hint("mcp/connect expects a single config map; use {:command ...} or {:url ...}")
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
    let response = block_on(connection.client.call_tool(tool_name, arguments_json))
        .map_err(|err| SemaError::eval(format!("mcp/call: {err}")))?;
    serde_json::to_value(response).map_err(|err| {
        SemaError::eval(format!(
            "mcp/call: failed to serialize tool response: {err}"
        ))
    })
}

pub fn register_mcp_builtins(env: &Env) {
    register_fn(env, "mcp/connect", |args| {
        let config_json = extract_config_map(args)?;
        let command = config_json
            .get("command")
            .and_then(|value| value.as_str())
            .ok_or_else(|| {
                SemaError::eval("mcp/connect requires a :command entry for stdio transport")
                    .with_hint("use {:command \"python3\" :args [\"-c\" \"script\"]}")
            })?;
        let empty_args = Vec::new();
        let args_json = config_json
            .get("args")
            .and_then(|value| value.as_array())
            .map(Vec::as_slice)
            .unwrap_or_else(|| empty_args.as_slice());
        let args_vec = args_json
            .iter()
            .filter_map(|value| value.as_str().map(|s| s.to_string()))
            .collect::<Vec<_>>();
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
            });
        let cwd = config_json
            .get("cwd")
            .and_then(|value| value.as_str())
            .map(std::path::PathBuf::from);
        let mut mcp_client = block_on(McpClient::connect(McpClientConfig {
            command: command.to_string(),
            args: args_vec,
            env: env_map,
            cwd,
        }))
        .map_err(|err| SemaError::eval(format!("mcp/connect: {err}")))?;
        if let Err(err) = block_on(mcp_client.initialize()) {
            let _ = block_on(mcp_client.close());
            return Err(SemaError::eval(format!("mcp/connect: {err}")));
        }
        let handle = next_handle();
        let connection = Rc::new(RefCell::new(McpConnection { client: mcp_client }));
        CONNECTIONS.with(|connections| {
            connections.borrow_mut().insert(handle.clone(), connection);
        });
        Ok(Value::string(&handle))
    });

    register_fn(env, "mcp/tools", |args| {
        check_arity!(args, "mcp/tools", 1);
        let handle = args[0].as_str().ok_or_else(|| {
            SemaError::type_error("string", args[0].type_name())
                .with_hint("mcp/tools expects the opaque handle returned by mcp/connect")
        })?;
        let connection = lookup_connection(handle)?;
        let mut connection = connection.borrow_mut();
        let tools = block_on(connection.client.list_tools())
            .map_err(|err| SemaError::eval(format!("mcp/tools: {err}")))?;
        let mut items = Vec::new();
        for tool in tools {
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
            items.push(Value::map(entry));
        }
        Ok(Value::list(items))
    });

    register_fn(env, "mcp/->tools", |args| {
        check_arity!(args, "mcp/->tools", 1);
        let handle = args[0].as_str().ok_or_else(|| {
            SemaError::type_error("string", args[0].type_name())
                .with_hint("mcp/->tools expects the opaque handle returned by mcp/connect")
        })?;
        let connection = lookup_connection(handle)?;
        let mut connection = connection.borrow_mut();
        let tools = block_on(connection.client.list_tools())
            .map_err(|err| SemaError::eval(format!("mcp/->tools: {err}")))?;
        let mut items = Vec::new();
        for tool in tools {
            let parameters = sema_core::json_to_value(&tool.input_schema);
            let tool_name = tool.name.clone();
            let connection_handle = handle.to_string();
            let handler_name = format!("mcp/{tool_name}");
            let handler = Value::native_fn(NativeFn::simple(&handler_name, move |args| {
                let arguments_json = if args.is_empty() {
                    serde_json::Value::Object(Default::default())
                } else {
                    sema_core::value_to_json_lossy(&args[0])
                };
                let response_json =
                    call_tool_via_connection(&connection_handle, &tool_name, arguments_json)?;
                Ok(sema_core::json_to_value(&response_json))
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
        let handle = args[0].as_str().ok_or_else(|| {
            SemaError::type_error("string", args[0].type_name())
                .with_hint("mcp/call expects the opaque handle returned by mcp/connect")
        })?;
        let tool_name = args[1].as_str().ok_or_else(|| {
            SemaError::type_error("string", args[1].type_name())
                .with_hint("mcp/call expects the tool name as a string")
        })?;
        let arguments_json = sema_core::value_to_json_lossy(&args[2]);
        let response_json = call_tool_via_connection(handle, tool_name, arguments_json)?;
        Ok(sema_core::json_to_value(&response_json))
    });

    register_fn(env, "mcp/close", |args| {
        check_arity!(args, "mcp/close", 1);
        let handle = args[0].as_str().ok_or_else(|| {
            SemaError::type_error("string", args[0].type_name())
                .with_hint("mcp/close expects the opaque handle returned by mcp/connect")
        })?;
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
