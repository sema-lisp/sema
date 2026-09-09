//! Command execution (`workspace/executeCommand`, `sema.runTopLevel`) and the
//! custom `sema/evalResult` notification it pushes back to the client.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::watch;
use tower_lsp::lsp_types::notification::Notification;
use tower_lsp::lsp_types::*;
use tower_lsp::Client;

use crate::helpers::*;
use crate::state::BackendState;

const EVAL_TIMEOUT: Duration = Duration::from_secs(30);

// ── Custom notification: sema/evalResult ─────────────────────────

#[derive(Debug, serde::Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvalResultParams {
    pub uri: Url,
    pub range: Range,
    /// Monotonically increasing id for this document's evaluation.
    pub run_id: u64,
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

/// Tracks the one active top-level evaluation per document.
///
/// A later run, a text update, document close, or server shutdown cancels the
/// previous run. The worker owns and reaps its child after it receives that
/// cancellation signal.
#[derive(Clone, Default)]
pub(crate) struct EvalRunManager {
    inner: Arc<Mutex<EvalRuns>>,
}

#[derive(Default)]
struct EvalRuns {
    next_id: u64,
    shutting_down: bool,
    active: HashMap<String, ActiveEvalRun>,
}

struct ActiveEvalRun {
    id: u64,
    cancel: watch::Sender<bool>,
}

pub(crate) struct EvalRunTicket {
    uri: String,
    id: u64,
    cancel: watch::Receiver<bool>,
}

impl EvalRunManager {
    fn begin(&self, uri: &Url) -> Option<EvalRunTicket> {
        let mut runs = self.inner.lock().ok()?;
        if runs.shutting_down {
            return None;
        }

        if let Some(previous) = runs.active.remove(uri.as_str()) {
            previous.cancel.send_replace(true);
        }

        let (cancel_tx, cancel_rx) = watch::channel(false);
        runs.next_id = runs.next_id.wrapping_add(1);
        let id = runs.next_id;
        let uri_key = uri.as_str().to_string();
        runs.active.insert(
            uri_key.clone(),
            ActiveEvalRun {
                id,
                cancel: cancel_tx,
            },
        );
        Some(EvalRunTicket {
            uri: uri_key,
            id,
            cancel: cancel_rx,
        })
    }

    pub(crate) fn cancel_document(&self, uri: &Url) {
        let Ok(mut runs) = self.inner.lock() else {
            return;
        };
        if let Some(run) = runs.active.remove(uri.as_str()) {
            run.cancel.send_replace(true);
        }
    }

    pub(crate) fn cancel_all(&self) {
        let Ok(mut runs) = self.inner.lock() else {
            return;
        };
        runs.shutting_down = true;
        for (_, run) in runs.active.drain() {
            run.cancel.send_replace(true);
        }
    }

    /// Returns true exactly once, and only for the current run for this URI.
    /// A late child result is therefore invisible after cancellation or a newer
    /// document version has replaced it.
    fn finish(&self, ticket: &EvalRunTicket) -> bool {
        let Ok(mut runs) = self.inner.lock() else {
            return false;
        };
        match runs.active.get(&ticket.uri) {
            Some(active) if active.id == ticket.id => {
                runs.active.remove(&ticket.uri);
                true
            }
            _ => false,
        }
    }
}

enum EvalProcessResult {
    Completed {
        stdout: Vec<u8>,
        stderr: Vec<u8>,
        elapsed_ms: u64,
    },
    TimedOut {
        stdout: Vec<u8>,
        stderr: Vec<u8>,
        elapsed_ms: u64,
    },
    Cancelled,
    Failed {
        message: String,
        elapsed_ms: u64,
    },
}

impl BackendState {
    pub(crate) fn cancel_eval_for_document(&self, uri: &Url) {
        self.eval_runs.cancel_document(uri);
    }

    pub(crate) fn cancel_all_evals(&self) {
        self.eval_runs.cancel_all();
    }

    pub(crate) fn handle_execute_command(
        &self,
        command: &str,
        arguments: &[serde_json::Value],
        client: &Client,
        handle: &tokio::runtime::Handle,
    ) {
        let Some(uri) = command_uri(arguments) else {
            return;
        };

        if command == "sema.cancelTopLevel" {
            self.cancel_eval_for_document(&uri);
            return;
        }
        if command != "sema.runTopLevel" {
            return;
        }

        let form_index = arguments
            .first()
            .and_then(|arg| arg.get("formIndex"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as usize;

        let (exprs, span_map) = if let Some(cached) = self.cached_parses.get(uri.as_str()) {
            (cached.ast.clone(), cached.span_map.clone())
        } else {
            let Some(text) = self.documents.get(uri.as_str()) else {
                return;
            };
            let Ok(parsed) = sema_reader::read_many_with_spans(text) else {
                return;
            };
            parsed
        };
        if form_index >= exprs.len() {
            return;
        }

        let lines: Vec<&str> = self
            .documents
            .get(uri.as_str())
            .map(|text| text.lines().collect())
            .unwrap_or_default();
        let form_range = top_level_ranges(&exprs, &span_map, &lines)
            .iter()
            .find(|(index, _)| *index == form_index)
            .map(|(_, range)| *range)
            .unwrap_or_default();
        let program = exprs[..=form_index]
            .iter()
            .map(|value| sema_core::pretty_print(value, 80))
            .collect::<Vec<_>>()
            .join("\n");
        let Some(mut ticket) = self.eval_runs.begin(&uri) else {
            return;
        };

        let sema_binary = self.sema_binary.clone();
        let runs = self.eval_runs.clone();
        let client = client.clone();
        handle.spawn(async move {
            let result = run_eval_process(&sema_binary, &uri, program, &mut ticket.cancel).await;
            if !runs.finish(&ticket) || matches!(result, EvalProcessResult::Cancelled) {
                return;
            }
            let params = eval_result_params(uri, form_range, ticket.id, result);
            client
                .send_notification::<EvalResultNotification>(params)
                .await;
        });
    }
}

fn command_uri(arguments: &[serde_json::Value]) -> Option<Url> {
    Url::parse(arguments.first()?.get("uri")?.as_str()?).ok()
}

async fn run_eval_process(
    sema_binary: &str,
    uri: &Url,
    program: String,
    cancel: &mut watch::Receiver<bool>,
) -> EvalProcessResult {
    let mut args = vec![
        "eval".to_string(),
        "--stdin".to_string(),
        "--json".to_string(),
    ];
    if let Ok(path) = uri.to_file_path() {
        args.push("--path".to_string());
        args.push(path.display().to_string());
    }

    let started = std::time::Instant::now();
    let mut child = match tokio::process::Command::new(PathBuf::from(sema_binary))
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            return EvalProcessResult::Failed {
                message: format!("Failed to spawn sema: {error}"),
                elapsed_ms: started.elapsed().as_millis() as u64,
            };
        }
    };

    if let Some(mut stdin) = child.stdin.take() {
        if let Err(error) = stdin.write_all(program.as_bytes()).await {
            let _ = child.start_kill();
            let _ = child.wait().await;
            return EvalProcessResult::Failed {
                message: format!("Failed to write evaluation input: {error}"),
                elapsed_ms: started.elapsed().as_millis() as u64,
            };
        }
    }

    wait_for_eval_child(child, cancel, EVAL_TIMEOUT, started).await
}

async fn wait_for_eval_child(
    mut child: tokio::process::Child,
    cancel: &mut watch::Receiver<bool>,
    timeout: Duration,
    started: std::time::Instant,
) -> EvalProcessResult {
    let stdout = child.stdout.take().map(read_pipe);
    let stderr = child.stderr.take().map(read_pipe);
    let completion = if *cancel.borrow() {
        EvalCompletion::Cancelled
    } else {
        tokio::select! {
            status = child.wait() => EvalCompletion::Exited(status.map_err(|error| error.to_string())),
            _ = cancel.changed() => EvalCompletion::Cancelled,
            _ = tokio::time::sleep(timeout) => EvalCompletion::TimedOut,
        }
    };

    match completion {
        EvalCompletion::Cancelled => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            await_pipes(stdout, stderr).await;
            EvalProcessResult::Cancelled
        }
        EvalCompletion::TimedOut => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            let (stdout, stderr) = await_pipes(stdout, stderr).await;
            EvalProcessResult::TimedOut {
                stdout,
                stderr,
                elapsed_ms: started.elapsed().as_millis() as u64,
            }
        }
        EvalCompletion::Exited(Err(error)) => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            await_pipes(stdout, stderr).await;
            EvalProcessResult::Failed {
                message: format!("Failed while waiting for sema: {error}"),
                elapsed_ms: started.elapsed().as_millis() as u64,
            }
        }
        EvalCompletion::Exited(Ok(_)) => {
            let (stdout, stderr) = await_pipes(stdout, stderr).await;
            EvalProcessResult::Completed {
                stdout,
                stderr,
                elapsed_ms: started.elapsed().as_millis() as u64,
            }
        }
    }
}

enum EvalCompletion {
    Exited(Result<std::process::ExitStatus, String>),
    Cancelled,
    TimedOut,
}

fn read_pipe<R>(mut pipe: R) -> tokio::task::JoinHandle<std::io::Result<Vec<u8>>>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut bytes = Vec::new();
        pipe.read_to_end(&mut bytes).await?;
        Ok(bytes)
    })
}

async fn await_pipes(
    stdout: Option<tokio::task::JoinHandle<std::io::Result<Vec<u8>>>>,
    stderr: Option<tokio::task::JoinHandle<std::io::Result<Vec<u8>>>>,
) -> (Vec<u8>, Vec<u8>) {
    let stdout = async move {
        match stdout {
            Some(task) => task.await.ok().and_then(Result::ok).unwrap_or_default(),
            None => Vec::new(),
        }
    };
    let stderr = async move {
        match stderr {
            Some(task) => task.await.ok().and_then(Result::ok).unwrap_or_default(),
            None => Vec::new(),
        }
    };
    tokio::join!(stdout, stderr)
}

fn eval_result_params(
    uri: Url,
    range: Range,
    run_id: u64,
    result: EvalProcessResult,
) -> EvalResultParams {
    match result {
        EvalProcessResult::Completed {
            stdout,
            stderr,
            elapsed_ms,
        } => eval_result_from_output(uri, range, run_id, stdout, stderr, elapsed_ms),
        EvalProcessResult::TimedOut {
            stdout,
            stderr,
            elapsed_ms,
        } => EvalResultParams {
            uri,
            range,
            run_id,
            kind: "run".to_string(),
            value: None,
            stdout: String::from_utf8_lossy(&stdout).to_string(),
            stderr: String::from_utf8_lossy(&stderr).to_string(),
            ok: false,
            error: Some("Evaluation timed out after 30 seconds".to_string()),
            elapsed_ms,
        },
        EvalProcessResult::Cancelled => {
            unreachable!("cancelled runs are suppressed before encoding")
        }
        EvalProcessResult::Failed {
            message,
            elapsed_ms,
        } => EvalResultParams {
            uri,
            range,
            run_id,
            kind: "run".to_string(),
            value: None,
            stdout: String::new(),
            stderr: String::new(),
            ok: false,
            error: Some(message),
            elapsed_ms,
        },
    }
}

fn eval_result_from_output(
    uri: Url,
    range: Range,
    run_id: u64,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    elapsed_ms: u64,
) -> EvalResultParams {
    let stdout = String::from_utf8_lossy(&stdout).to_string();
    let stderr = String::from_utf8_lossy(&stderr).to_string();
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&stdout) else {
        return EvalResultParams {
            uri,
            range,
            run_id,
            kind: "run".to_string(),
            value: None,
            stdout,
            stderr,
            ok: false,
            error: Some("Failed to parse eval output".to_string()),
            elapsed_ms,
        };
    };

    EvalResultParams {
        uri,
        range,
        run_id,
        kind: "run".to_string(),
        value: json
            .get("value")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        stdout: json
            .get("stdout")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string(),
        stderr: json
            .get("stderr")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string(),
        ok: json
            .get("ok")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        error: json.get("error").and_then(|error| {
            error
                .get("message")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        }),
        elapsed_ms: json
            .get("elapsedMs")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(elapsed_ms),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_run_cancels_and_suppresses_the_previous_result() {
        let manager = EvalRunManager::default();
        let uri = Url::parse("file:///runs.sema").unwrap();
        let old = manager.begin(&uri).expect("first run");
        let new = manager.begin(&uri).expect("second run");

        assert!(*old.cancel.borrow());
        assert!(!manager.finish(&old));
        assert!(manager.finish(&new));
    }

    #[test]
    fn document_change_cancels_and_suppresses_its_run() {
        let manager = EvalRunManager::default();
        let uri = Url::parse("file:///changed.sema").unwrap();
        let run = manager.begin(&uri).expect("run");

        manager.cancel_document(&uri);

        assert!(*run.cancel.borrow());
        assert!(!manager.finish(&run));
    }

    #[test]
    fn shutdown_cancels_active_runs_and_rejects_new_ones() {
        let manager = EvalRunManager::default();
        let uri = Url::parse("file:///shutdown.sema").unwrap();
        let run = manager.begin(&uri).expect("run");

        manager.cancel_all();

        assert!(*run.cancel.borrow());
        assert!(!manager.finish(&run));
        assert!(manager.begin(&uri).is_none());
    }

    #[test]
    fn eval_result_preserves_the_server_run_id() {
        let uri = Url::parse("file:///result.sema").unwrap();
        let result = eval_result_from_output(
            uri,
            Range::default(),
            17,
            br#"{"ok":true,"value":"42","stdout":"","stderr":"","elapsedMs":3}"#.to_vec(),
            Vec::new(),
            1,
        );

        assert_eq!(result.run_id, 17);
        assert!(result.ok);
        assert_eq!(result.value.as_deref(), Some("42"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancellation_kills_and_reaps_the_owned_child() {
        let child = tokio::process::Command::new("/bin/sleep")
            .arg("30")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("start child");
        let (cancel_tx, mut cancel_rx) = watch::channel(false);
        let task = tokio::spawn(async move {
            wait_for_eval_child(
                child,
                &mut cancel_rx,
                Duration::from_secs(5),
                std::time::Instant::now(),
            )
            .await
        });

        cancel_tx.send_replace(true);
        assert!(matches!(
            task.await.expect("task"),
            EvalProcessResult::Cancelled
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn timeout_kills_and_reaps_the_owned_child() {
        let child = tokio::process::Command::new("/bin/sleep")
            .arg("30")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("start child");
        let (_cancel_tx, mut cancel_rx) = watch::channel(false);

        let result = wait_for_eval_child(
            child,
            &mut cancel_rx,
            Duration::from_millis(10),
            std::time::Instant::now(),
        )
        .await;

        assert!(matches!(result, EvalProcessResult::TimedOut { .. }));
    }
}
