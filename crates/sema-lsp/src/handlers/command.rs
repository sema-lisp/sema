//! Command execution (`workspace/executeCommand`, `sema.runTopLevel`) and the
//! custom `sema/evalResult` notification it pushes back to the client.

use std::path::PathBuf;

use serde::Serialize;
use tower_lsp::lsp_types::notification::Notification;
use tower_lsp::lsp_types::*;
use tower_lsp::Client;

use crate::helpers::*;
use crate::state::BackendState;

// ── Custom notification: sema/evalResult ─────────────────────────

#[derive(Debug, serde::Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvalResultParams {
    pub uri: Url,
    pub range: Range,
    pub kind: String,
    pub value: Option<String>,
    pub stdout: String,
    pub stderr: String,
    pub ok: bool,
    pub error: Option<String>,
    pub elapsed_ms: u64,
}

pub enum EvalResultNotification {}

impl Notification for EvalResultNotification {
    type Params = EvalResultParams;
    const METHOD: &'static str = "sema/evalResult";
}

impl BackendState {
    pub(crate) fn handle_execute_command(
        &self,
        command: &str,
        arguments: &[serde_json::Value],
        client: &Client,
        handle: &tokio::runtime::Handle,
    ) {
        if command != "sema.runTopLevel" {
            return;
        }

        let arg = match arguments.first() {
            Some(a) => a,
            None => return,
        };

        let uri_str = arg.get("uri").and_then(|v| v.as_str()).unwrap_or("");
        let form_index = arg.get("formIndex").and_then(|v| v.as_u64()).unwrap_or(0) as usize;

        let uri = match Url::parse(uri_str) {
            Ok(u) => u,
            Err(_) => return,
        };

        // Prefer the cached parse populated by didChange; only re-parse if
        // the cache misses (should be rare for open documents).
        let (exprs, span_map) = if let Some(cached) = self.cached_parses.get(uri_str) {
            (cached.ast.clone(), cached.span_map.clone())
        } else {
            let text = match self.documents.get(uri_str) {
                Some(t) => t.clone(),
                None => return,
            };
            match sema_reader::read_many_with_spans(&text) {
                Ok(r) => r,
                Err(_) => return,
            }
        };

        if form_index >= exprs.len() {
            return;
        }

        let lines: Vec<&str> = self
            .documents
            .get(uri_str)
            .map(|t| t.lines().collect())
            .unwrap_or_default();
        let indexed_ranges = top_level_ranges(&exprs, &span_map, &lines);
        let form_range = indexed_ranges
            .iter()
            .find(|(i, _)| *i == form_index)
            .map(|(_, r)| *r)
            .unwrap_or_default();

        // Build program text: pretty-print forms [0..=form_index]
        let program: String = exprs[..=form_index]
            .iter()
            .map(|v| sema_core::pretty_print(v, 80))
            .collect::<Vec<_>>()
            .join("\n");

        // Use configured sema binary path
        let sema_bin = PathBuf::from(&self.sema_binary);

        // Build args
        let mut args = vec![
            "eval".to_string(),
            "--stdin".to_string(),
            "--json".to_string(),
        ];

        let is_strict = self.run_sandbox_mode == "strict";
        if is_strict {
            args.push("--sandbox".to_string());
            args.push("strict".to_string());
            args.push("--no-llm".to_string());
        }

        // Add --path if the URI is a file
        if let Ok(path) = uri.to_file_path() {
            args.push("--path".to_string());
            args.push(path.display().to_string());
        }

        let start = std::time::Instant::now();

        let result = std::process::Command::new(&sema_bin)
            .args(&args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                use std::io::Write;
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = stdin.write_all(program.as_bytes());
                    // stdin is dropped here, closing the pipe
                }
                child.wait_with_output()
            });

        let elapsed_ms = start.elapsed().as_millis() as u64;

        let params = match result {
            Ok(output) => {
                let stdout_str = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr_str = String::from_utf8_lossy(&output.stderr).to_string();

                // Parse the JSON envelope from stdout
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&stdout_str) {
                    let ok = json.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
                    let value = json
                        .get("value")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    let captured_stdout = json
                        .get("stdout")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let captured_stderr = json
                        .get("stderr")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let error = json.get("error").and_then(|v| {
                        v.get("message")
                            .and_then(|m| m.as_str())
                            .map(|s| {
                                let mut msg = s.to_string();
                                if is_strict && (msg.contains("sandbox") || msg.contains("LLM") || msg.contains("effect")) {
                                    msg.push_str(" (Run lens sandbox is 'strict'. Change 'sema.run.sandbox' setting to 'off' to allow this.)");
                                }
                                msg
                            })
                    });
                    let eval_elapsed = json
                        .get("elapsedMs")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(elapsed_ms);

                    EvalResultParams {
                        uri: uri.clone(),
                        range: form_range,
                        kind: "run".to_string(),
                        value,
                        stdout: captured_stdout,
                        stderr: captured_stderr,
                        ok,
                        error,
                        elapsed_ms: eval_elapsed,
                    }
                } else {
                    EvalResultParams {
                        uri: uri.clone(),
                        range: form_range,
                        kind: "run".to_string(),
                        value: None,
                        stdout: stdout_str,
                        stderr: stderr_str,
                        ok: false,
                        error: Some("Failed to parse eval output".to_string()),
                        elapsed_ms,
                    }
                }
            }
            Err(e) => EvalResultParams {
                uri: uri.clone(),
                range: form_range,
                kind: "run".to_string(),
                value: None,
                stdout: String::new(),
                stderr: String::new(),
                ok: false,
                error: Some(format!("Failed to spawn sema: {e}")),
                elapsed_ms,
            },
        };

        let client = client.clone();
        handle.block_on(async {
            client
                .send_notification::<EvalResultNotification>(params)
                .await;
        });
    }
}
