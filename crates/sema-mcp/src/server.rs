use serde_json::json;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::notebook::NotebookCache;
use crate::protocol::{JsonRpcError, JsonRpcRequest, JsonRpcResponse};
use crate::tools::{call_mcp_tool, list_mcp_tools};
use sema_eval::Interpreter;

pub async fn run_mcp_server(
    interpreter: Interpreter,
    include_tools: Option<Vec<String>>,
    exclude_tools: Option<Vec<String>>,
) -> Result<(), String> {
    run_mcp_server_on(
        tokio::io::stdin(),
        tokio::io::stdout(),
        interpreter,
        include_tools,
        exclude_tools,
    )
    .await
}

pub async fn run_mcp_server_on<R, W>(
    reader: R,
    mut writer: W,
    interpreter: Interpreter,
    include_tools: Option<Vec<String>>,
    exclude_tools: Option<Vec<String>>,
) -> Result<(), String>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let mut stdin = BufReader::new(reader);
    let mut line = String::new();

    let notebook_cache: NotebookCache = Rc::new(RefCell::new(BTreeMap::new()));

    eprintln!("Sema MCP server starting stdio loop...");

    loop {
        line.clear();
        let bytes_read = stdin
            .read_line(&mut line)
            .await
            .map_err(|e| format!("Failed to read stdin: {e}"))?;
        if bytes_read == 0 {
            break; // EOF
        }

        let line_trimmed = line.trim();
        if line_trimmed.is_empty() {
            continue;
        }

        let request: JsonRpcRequest = match serde_json::from_str(line_trimmed) {
            Ok(req) => req,
            Err(e) => {
                let err_resp = JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    result: None,
                    error: Some(JsonRpcError::new(-32700, format!("Parse error: {e}"))),
                    id: None,
                };
                let resp_str = format!("{}\n", serde_json::to_string(&err_resp).unwrap());
                writer.write_all(resp_str.as_bytes()).await.ok();
                writer.flush().await.ok();
                continue;
            }
        };

        let response = handle_request(
            request,
            &interpreter,
            &notebook_cache,
            include_tools.as_deref(),
            exclude_tools.as_deref(),
        );

        if let Some(resp) = response {
            let resp_str = format!("{}\n", serde_json::to_string(&resp).unwrap());
            if let Err(e) = writer.write_all(resp_str.as_bytes()).await {
                eprintln!("Error writing response to stdout: {e}");
                break;
            }
            if let Err(e) = writer.flush().await {
                eprintln!("Error flushing stdout: {e}");
                break;
            }
        }
    }

    eprintln!("Sema MCP server stdio loop exited.");
    Ok(())
}

fn handle_request(
    req: JsonRpcRequest,
    interpreter: &Interpreter,
    notebook_cache: &NotebookCache,
    include_tools: Option<&[String]>,
    exclude_tools: Option<&[String]>,
) -> Option<JsonRpcResponse> {
    let method = req.method.as_str();
    let id = req.id.clone();

    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {
                "tools": {
                    "listChanged": false
                }
            },
            "serverInfo": {
                "name": "sema-mcp",
                "version": env!("CARGO_PKG_VERSION")
            }
        })),
        "notifications/initialized" => {
            return None;
        }
        "ping" => Ok(json!({})),
        "tools/list" => {
            let tools = list_mcp_tools(interpreter, include_tools, exclude_tools);
            Ok(json!({
                "tools": tools
            }))
        }
        "tools/call" => {
            let params = req.params.clone().unwrap_or(json!({}));
            let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

            let call_res = call_mcp_tool(
                name,
                &arguments,
                interpreter,
                notebook_cache,
                include_tools,
                exclude_tools,
            );
            Ok(json!(call_res))
        }
        _ => Err(JsonRpcError::new(
            -32601,
            format!("Method not found: {method}"),
        )),
    };

    match result {
        Ok(res) => Some(JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            result: Some(res),
            error: None,
            id,
        }),
        Err(err) => Some(JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            result: None,
            error: Some(err),
            id,
        }),
    }
}
