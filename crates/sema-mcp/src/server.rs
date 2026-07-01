use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::notebook::{new_cache, NotebookCache};
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
    // Read raw bytes per line: a single non-UTF-8 byte must NOT tear down the
    // server (it should produce a JSON-RPC parse error and continue), so we
    // cannot use `read_line` which fails the whole read on invalid UTF-8.
    let mut buf: Vec<u8> = Vec::new();

    let notebook_cache: NotebookCache = new_cache();

    eprintln!("Sema MCP server starting stdio loop...");

    loop {
        buf.clear();
        let bytes_read = stdin
            .read_until(b'\n', &mut buf)
            .await
            .map_err(|e| format!("Failed to read stdin: {e}"))?;
        if bytes_read == 0 {
            break; // EOF
        }

        // Decode as UTF-8. A malformed line is a recoverable parse error.
        let line = match std::str::from_utf8(&buf) {
            Ok(s) => s,
            Err(_) => {
                write_parse_error(&mut writer, "request was not valid UTF-8".to_string()).await;
                continue;
            }
        };

        let line_trimmed = line.trim();
        if line_trimmed.is_empty() {
            continue;
        }

        let request: JsonRpcRequest = match serde_json::from_str(line_trimmed) {
            Ok(req) => req,
            Err(e) => {
                write_parse_error(&mut writer, format!("{e}")).await;
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
            // Serialize on the hot path WITHOUT unwrapping: a serialization
            // failure must not panic and tear down the whole server loop. Fall
            // back to a generic -32603 internal-error response that still carries
            // the original request id so the client can correlate it.
            let resp_str = match serde_json::to_string(&resp) {
                Ok(s) => format!("{s}\n"),
                Err(e) => {
                    eprintln!("Failed to serialize response: {e}");
                    let fallback = JsonRpcResponse {
                        jsonrpc: "2.0".to_string(),
                        result: None,
                        error: Some(JsonRpcError::new(
                            -32603,
                            format!("Internal error: failed to serialize response: {e}"),
                        )),
                        id: resp.id.clone(),
                        method: None,
                    };
                    // The fallback is a tiny, statically-shaped struct that should
                    // never fail to serialize; if it somehow does, emit a
                    // hand-written minimal frame rather than panicking.
                    match serde_json::to_string(&fallback) {
                        Ok(s) => format!("{s}\n"),
                        Err(_) => "{\"jsonrpc\":\"2.0\",\"error\":{\"code\":-32603,\"message\":\"Internal error\"},\"id\":null}\n".to_string(),
                    }
                }
            };
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

/// Write a JSON-RPC parse-error (-32700) response with a null id and flush.
/// Used for unrecoverable per-line decode failures (invalid UTF-8 or invalid
/// JSON) that must not terminate the server loop.
async fn write_parse_error<W>(writer: &mut W, message: String)
where
    W: tokio::io::AsyncWrite + Unpin,
{
    let err_resp = JsonRpcResponse {
        jsonrpc: "2.0".to_string(),
        result: None,
        error: Some(JsonRpcError::new(-32700, format!("Parse error: {message}"))),
        id: None,
        method: None,
    };
    if let Ok(s) = serde_json::to_string(&err_resp) {
        writer.write_all(format!("{s}\n").as_bytes()).await.ok();
        writer.flush().await.ok();
    }
}

fn handle_request(
    req: JsonRpcRequest,
    interpreter: &Interpreter,
    notebook_cache: &NotebookCache,
    include_tools: Option<&[String]>,
    exclude_tools: Option<&[String]>,
) -> Option<JsonRpcResponse> {
    // Per JSON-RPC 2.0, a request without an `id` is a notification: the server
    // MUST NOT reply to it. Bailing out with None here keeps us silent for every
    // notification method (e.g. notifications/cancelled, notifications/progress)
    // instead of emitting a spurious `id: null` response.
    req.id.as_ref()?;

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
            method: None,
        }),
        Err(err) => Some(JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            result: None,
            error: Some(err),
            id,
            method: None,
        }),
    }
}
