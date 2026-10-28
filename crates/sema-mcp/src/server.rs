//! Compatibility transport adapters over the shared MCP session.
use crate::session::{McpServerOptions, McpSession};
use sema_eval::Interpreter;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

fn options(
    include: Option<Vec<String>>,
    exclude: Option<Vec<String>>,
    timeout: Option<std::time::Duration>,
) -> McpServerOptions {
    let mut options = McpServerOptions::cli().timeout(timeout);
    if let Some(names) = include {
        options = options.only_tools(names);
    }
    if let Some(names) = exclude {
        options = options.exclude_tools(names);
    }
    options
}

/// Serve over process stdio. Use a session directly to configure logging or build.
pub fn run_mcp_server_sync(
    interpreter: Interpreter,
    include: Option<Vec<String>>,
    exclude: Option<Vec<String>>,
    timeout: Option<std::time::Duration>,
) -> Result<(), String> {
    let session = McpSession::new(&interpreter, options(include, exclude, timeout))
        .map_err(|e| e.to_string())?;
    session.serve_stdio(&interpreter).map_err(|e| e.to_string())
}

/// Legacy async transport adapter. Tool execution is synchronous on this thread;
/// async Rust hosts should use `sema::mcp::McpWorker` instead.
pub async fn run_mcp_server(
    interpreter: Interpreter,
    include: Option<Vec<String>>,
    exclude: Option<Vec<String>>,
    timeout: Option<std::time::Duration>,
) -> Result<(), String> {
    run_mcp_server_on(
        tokio::io::stdin(),
        tokio::io::stdout(),
        interpreter,
        include,
        exclude,
        timeout,
    )
    .await
}

/// Legacy async transport adapter using the same bounded frame and session rules.
pub async fn run_mcp_server_on<R, W>(
    reader: R,
    mut writer: W,
    interpreter: Interpreter,
    include: Option<Vec<String>>,
    exclude: Option<Vec<String>>,
    timeout: Option<std::time::Duration>,
) -> Result<(), String>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let options = options(include, exclude, timeout);
    let limit = options.max_frame_bytes;
    let session = McpSession::new(&interpreter, options).map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(reader);
    let mut frame = Vec::new();
    loop {
        frame.clear();
        let read = (&mut reader)
            .take((limit as u64).saturating_add(1))
            .read_until(b'\n', &mut frame)
            .await
            .map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        if frame.len() > limit && frame.last() != Some(&b'\n') {
            loop {
                let bytes = reader.fill_buf().await.map_err(|e| e.to_string())?;
                if bytes.is_empty() {
                    break;
                }
                let end = bytes.iter().position(|b| *b == b'\n');
                let used = end.map_or(bytes.len(), |n| n + 1);
                reader.consume(used);
                if end.is_some() {
                    break;
                }
            }
        }
        if frame.len() <= limit && frame.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let mut response = Vec::new();
        session
            .handle_message(&interpreter, &frame, &mut response)
            .map_err(|e| e.to_string())?;
        writer
            .write_all(&response)
            .await
            .map_err(|e| e.to_string())?;
        writer.flush().await.map_err(|e| e.to_string())?;
    }
    Ok(())
}
