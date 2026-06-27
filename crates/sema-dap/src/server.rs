use std::path::PathBuf;
use std::sync::mpsc as std_mpsc;

use tokio::io::BufReader;
use tokio::sync::mpsc as tokio_mpsc;

use sema_vm::debug::{DapBreakpoint, DebugCommand, DebugEvent, DebugState, SourceBreakpoint};

use crate::protocol::{DapEvent, DapMessage, DapResponse};
use crate::transport;

/// Messages from the async frontend to the backend thread.
/// Only used for operations that require access to the backend thread's state.
enum BackendRequest {
    Launch {
        program: PathBuf,
        stop_on_entry: bool,
        cmd_rx: std_mpsc::Receiver<DebugCommand>,
    },
    SetBreakpoints {
        file: PathBuf,
        breakpoints: Vec<SourceBreakpoint>,
        reply: tokio_mpsc::Sender<Vec<DapBreakpoint>>,
    },
    SetExceptionBreakpoints {
        break_on_uncaught: bool,
    },
    ConfigurationDone,
    Disconnect,
}

struct FrontendState {
    vm_active: bool,
    vm_suspended: bool,
    dbg_cmd_tx: Option<std_mpsc::Sender<DebugCommand>>,
    /// Whether the exception breakpoint filter is enabled. Forwarded to the
    /// backend (pre-launch) or the running VM (via DebugCommand).
    break_on_uncaught: bool,
    /// Message of the most recent uncaught exception we stopped on, surfaced via
    /// the exceptionInfo request.
    last_exception: Option<String>,
}

pub async fn run() {
    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin);
    let mut stdout = tokio::io::stdout();

    let mut seq: u64 = 1;
    let (backend_tx, backend_rx) = tokio_mpsc::channel::<BackendRequest>(32);
    let (event_bridge_tx, mut event_bridge_rx) = tokio_mpsc::channel::<DebugEvent>(32);
    let mut state = FrontendState {
        vm_active: false,
        vm_suspended: false,
        dbg_cmd_tx: None,
        break_on_uncaught: false,
        last_exception: None,
    };

    // Spawn the backend thread
    let event_bridge_tx_clone = event_bridge_tx.clone();
    std::thread::spawn(move || {
        backend_thread(backend_rx, event_bridge_tx_clone);
    });

    loop {
        tokio::select! {
            msg = transport::read_message(&mut reader) => {
                match msg {
                    Ok(Some(text)) => {
                        let parsed: Result<DapMessage, _> = serde_json::from_str(&text);
                        let Ok(msg) = parsed else {
                            eprintln!("DAP: failed to parse message: {text}");
                            continue;
                        };
                        let handled = handle_request(
                            &msg,
                            &mut stdout,
                            &mut seq,
                            &backend_tx,
                            &mut state,
                        ).await;
                        if !handled {
                            break;
                        }
                    }
                    Ok(None) => break, // EOF
                    Err(e) => {
                        eprintln!("DAP: read error: {e}");
                        break;
                    }
                }
            }
            Some(event) = event_bridge_rx.recv() => {
                let dap_event = match event {
                    DebugEvent::Stopped { reason, description } => {
                        state.vm_suspended = true;
                        let reason_str = match reason {
                            sema_vm::debug::StopReason::Breakpoint => "breakpoint",
                            sema_vm::debug::StopReason::Step => "step",
                            sema_vm::debug::StopReason::Pause => "pause",
                            sema_vm::debug::StopReason::Entry => "entry",
                            sema_vm::debug::StopReason::Exception => {
                                // Remember the message so a follow-up exceptionInfo
                                // request can report it.
                                state.last_exception = description.clone();
                                "exception"
                            }
                        };
                        DapEvent::new(seq, "stopped", Some(serde_json::json!({
                            "reason": reason_str,
                            "description": description,
                            "threadId": 1,
                            "allThreadsStopped": true,
                        })))
                    }
                    DebugEvent::Terminated => {
                        state.vm_active = false;
                        state.vm_suspended = false;
                        // Drop the command sender so the backend's post-execution
                        // drain loop sees the channel disconnect and exits, and so
                        // any later request routes to the (pending) backend path
                        // rather than a VM that is no longer polling.
                        state.dbg_cmd_tx = None;
                        DapEvent::new(seq, "terminated", None)
                    }
                    DebugEvent::Output { category, output } => {
                        DapEvent::new(seq, "output", Some(serde_json::json!({
                            "category": category,
                            "output": output,
                        })))
                    }
                };
                seq += 1;
                match serde_json::to_string(&dap_event) {
                    Ok(json) => { let _ = transport::write_message(&mut stdout, &json).await; }
                    Err(e) => eprintln!("DAP: failed to serialize event: {e}"),
                }
            }
        }
    }
}

async fn handle_request(
    msg: &DapMessage,
    stdout: &mut tokio::io::Stdout,
    seq: &mut u64,
    backend_tx: &tokio_mpsc::Sender<BackendRequest>,
    state: &mut FrontendState,
) -> bool {
    let Some(ref command) = msg.command else {
        return true;
    };

    match command.as_str() {
        "initialize" => {
            let body = serde_json::json!({
                "supportsConfigurationDoneRequest": true,
                "supportsFunctionBreakpoints": false,
                "supportsConditionalBreakpoints": true,
                "supportsStepBack": false,
                "supportsSetVariable": true,
                "supportsEvaluateForHovers": true,
                "supportsRestartFrame": false,
                "supportsModulesRequest": false,
                "supportsExceptionInfoRequest": true,
                "exceptionBreakpointFilters": [{
                    "filter": "uncaught",
                    "label": "Uncaught Exceptions",
                    "default": false,
                }],
            });
            send_response(stdout, seq, msg.seq, "initialize", Some(body)).await;
            // Send initialized event
            let event = DapEvent::new(*seq, "initialized", None);
            *seq += 1;
            match serde_json::to_string(&event) {
                Ok(json) => {
                    let _ = transport::write_message(stdout, &json).await;
                }
                Err(e) => eprintln!("DAP: failed to serialize initialized event: {e}"),
            }
        }
        "launch" => {
            let program = msg
                .arguments
                .as_ref()
                .and_then(|a| a.get("program"))
                .and_then(|p| p.as_str())
                .map(clean_path);
            let stop_on_entry = msg
                .arguments
                .as_ref()
                .and_then(|a| a.get("stopOnEntry"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            if let Some(program) = program {
                let (cmd_tx, cmd_rx) = std_mpsc::channel::<DebugCommand>();
                let _ = backend_tx
                    .send(BackendRequest::Launch {
                        program,
                        stop_on_entry,
                        cmd_rx,
                    })
                    .await;
                state.dbg_cmd_tx = Some(cmd_tx);
                send_response(stdout, seq, msg.seq, "launch", None).await;
            } else {
                send_error(stdout, seq, msg.seq, "launch", "missing 'program' argument").await;
            }
        }
        "setBreakpoints" => {
            let file = msg
                .arguments
                .as_ref()
                .and_then(|a| a.get("source"))
                .and_then(|s| s.get("path"))
                .and_then(|p| p.as_str())
                .map(clean_path)
                .unwrap_or_default();
            let breakpoints_req: Vec<SourceBreakpoint> = msg
                .arguments
                .as_ref()
                .and_then(|a| a.get("breakpoints"))
                .and_then(|b| b.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|bp| {
                            let line = bp.get("line").and_then(|l| l.as_u64())? as u32;
                            let condition = bp
                                .get("condition")
                                .and_then(|c| c.as_str())
                                .filter(|c| !c.trim().is_empty())
                                .map(|c| c.to_string());
                            Some(SourceBreakpoint { line, condition })
                        })
                        .collect()
                })
                .unwrap_or_default();

            let resolved_breakpoints = if state.vm_active {
                if let Some(ref tx) = state.dbg_cmd_tx {
                    send_cmd_and_recv(tx, |reply| DebugCommand::SetBreakpoints {
                        file,
                        breakpoints: breakpoints_req,
                        reply,
                    })
                    .await
                } else {
                    Vec::new()
                }
            } else {
                // Pre-launch — send via backend
                let (reply_tx, mut reply_rx) = tokio_mpsc::channel(1);
                let _ = backend_tx
                    .send(BackendRequest::SetBreakpoints {
                        file,
                        breakpoints: breakpoints_req,
                        reply: reply_tx,
                    })
                    .await;
                reply_rx.recv().await.unwrap_or_default()
            };
            let breakpoints: Vec<serde_json::Value> = resolved_breakpoints
                .iter()
                .map(breakpoint_to_json)
                .collect();
            send_response(
                stdout,
                seq,
                msg.seq,
                "setBreakpoints",
                Some(serde_json::json!({ "breakpoints": breakpoints })),
            )
            .await;
        }
        "setExceptionBreakpoints" => {
            // The "uncaught" filter is enabled when present in the filters list.
            let break_on_uncaught = msg
                .arguments
                .as_ref()
                .and_then(|a| a.get("filters"))
                .and_then(|f| f.as_array())
                .map(|arr| arr.iter().any(|v| v.as_str() == Some("uncaught")))
                .unwrap_or(false);
            state.break_on_uncaught = break_on_uncaught;

            if state.vm_active {
                if let Some(ref tx) = state.dbg_cmd_tx {
                    let _ = tx.send(DebugCommand::SetExceptionBreakpoints { break_on_uncaught });
                }
            } else {
                // Pre-launch: stash on the backend so the flag is applied to the
                // DebugState created at launch time.
                let _ = backend_tx
                    .send(BackendRequest::SetExceptionBreakpoints { break_on_uncaught })
                    .await;
            }
            send_response(stdout, seq, msg.seq, "setExceptionBreakpoints", None).await;
        }
        "exceptionInfo" => {
            let body = match &state.last_exception {
                Some(message) => serde_json::json!({
                    "exceptionId": "uncaught",
                    "description": message,
                    "breakMode": "unhandled",
                    "details": { "message": message },
                }),
                None => serde_json::json!({
                    "exceptionId": "uncaught",
                    "description": "No exception information available",
                    "breakMode": "unhandled",
                }),
            };
            send_response(stdout, seq, msg.seq, "exceptionInfo", Some(body)).await;
        }
        "configurationDone" => {
            let _ = backend_tx.send(BackendRequest::ConfigurationDone).await;
            // Only mark the VM active if a launch actually produced a command
            // channel and it hasn't already terminated (e.g. a launch/compile
            // error sends Terminated, which clears dbg_cmd_tx).
            state.vm_active = state.dbg_cmd_tx.is_some();
            state.vm_suspended = false;
            send_response(stdout, seq, msg.seq, "configurationDone", None).await;
        }
        "threads" => {
            send_response(
                stdout,
                seq,
                msg.seq,
                "threads",
                Some(serde_json::json!({
                    "threads": [{ "id": 1, "name": "main" }]
                })),
            )
            .await;
        }
        "stackTrace" => {
            // Only query the VM while it is stopped: it can only answer a stack
            // trace meaningfully then, and gating on `vm_suspended` guarantees
            // the VM is parked in its command loop so the reply wait can't hang.
            let frames = if state.vm_active && state.vm_suspended {
                if let Some(ref tx) = state.dbg_cmd_tx {
                    send_cmd_and_recv(tx, |reply| DebugCommand::GetStackTrace { reply }).await
                } else {
                    Vec::new()
                }
            } else {
                Vec::new()
            };
            let stack_frames: Vec<serde_json::Value> = frames
                .iter()
                .map(|f| {
                    let mut frame = serde_json::json!({
                        "id": f.id,
                        "name": f.name,
                        "line": f.line,
                        "column": f.column,
                    });
                    if let Some(ref path) = f.source_file {
                        frame.as_object_mut().unwrap().insert(
                            "source".to_string(),
                            serde_json::json!({
                                "name": path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
                                "path": path.to_string_lossy(),
                            }),
                        );
                    }
                    frame
                })
                .collect();
            send_response(
                stdout,
                seq,
                msg.seq,
                "stackTrace",
                Some(serde_json::json!({
                    "stackFrames": stack_frames,
                    "totalFrames": stack_frames.len(),
                })),
            )
            .await;
        }
        "scopes" => {
            let frame_id = msg
                .arguments
                .as_ref()
                .and_then(|a| a.get("frameId"))
                .and_then(|f| f.as_u64())
                .unwrap_or(0) as usize;
            let scopes = if state.vm_active && state.vm_suspended {
                if let Some(ref tx) = state.dbg_cmd_tx {
                    send_cmd_and_recv(tx, |reply| DebugCommand::GetScopes { frame_id, reply }).await
                } else {
                    Vec::new()
                }
            } else {
                Vec::new()
            };
            let scope_json: Vec<serde_json::Value> = scopes
                .iter()
                .map(|s| {
                    serde_json::json!({
                        "name": s.name,
                        "variablesReference": s.variables_reference,
                        "expensive": s.expensive,
                    })
                })
                .collect();
            send_response(
                stdout,
                seq,
                msg.seq,
                "scopes",
                Some(serde_json::json!({ "scopes": scope_json })),
            )
            .await;
        }
        "variables" => {
            let reference = msg
                .arguments
                .as_ref()
                .and_then(|a| a.get("variablesReference"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let vars = if state.vm_active && state.vm_suspended {
                if let Some(ref tx) = state.dbg_cmd_tx {
                    send_cmd_and_recv(tx, |reply| DebugCommand::GetVariables { reference, reply })
                        .await
                } else {
                    Vec::new()
                }
            } else {
                Vec::new()
            };
            let var_json: Vec<serde_json::Value> = vars
                .iter()
                .map(|v| {
                    serde_json::json!({
                        "name": v.name,
                        "value": v.value,
                        "type": v.type_name,
                        "variablesReference": v.variables_reference,
                    })
                })
                .collect();
            send_response(
                stdout,
                seq,
                msg.seq,
                "variables",
                Some(serde_json::json!({ "variables": var_json })),
            )
            .await;
        }
        "evaluate" => {
            if !(state.vm_active && state.vm_suspended) {
                send_error(
                    stdout,
                    seq,
                    msg.seq,
                    "evaluate",
                    "evaluate is only available while execution is stopped",
                )
                .await;
                return true;
            }

            let expression = msg
                .arguments
                .as_ref()
                .and_then(|a| a.get("expression"))
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let frame_id = msg
                .arguments
                .as_ref()
                .and_then(|a| a.get("frameId"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize;

            let Some(ref tx) = state.dbg_cmd_tx else {
                send_error(
                    stdout,
                    seq,
                    msg.seq,
                    "evaluate",
                    "debug VM is not available",
                )
                .await;
                return true;
            };

            let result = send_cmd_and_recv_result(tx, "debug VM did not reply", |reply| {
                DebugCommand::Evaluate {
                    frame_id,
                    expression,
                    reply,
                }
            })
            .await;

            match result {
                Ok(var) => {
                    send_response(
                        stdout,
                        seq,
                        msg.seq,
                        "evaluate",
                        Some(serde_json::json!({
                            "result": var.value,
                            "type": var.type_name,
                            "variablesReference": var.variables_reference,
                        })),
                    )
                    .await;
                }
                Err(message) => {
                    send_error(stdout, seq, msg.seq, "evaluate", &message).await;
                }
            }
        }
        "setVariable" => {
            if !(state.vm_active && state.vm_suspended) {
                send_error(
                    stdout,
                    seq,
                    msg.seq,
                    "setVariable",
                    "setVariable is only available while execution is stopped",
                )
                .await;
                return true;
            }

            let variables_reference = msg
                .arguments
                .as_ref()
                .and_then(|a| a.get("variablesReference"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let name = msg
                .arguments
                .as_ref()
                .and_then(|a| a.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let value_expression = msg
                .arguments
                .as_ref()
                .and_then(|a| a.get("value"))
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();

            let Some(ref tx) = state.dbg_cmd_tx else {
                send_error(
                    stdout,
                    seq,
                    msg.seq,
                    "setVariable",
                    "debug VM is not available",
                )
                .await;
                return true;
            };

            let result = send_cmd_and_recv_result(tx, "debug VM did not reply", |reply| {
                DebugCommand::SetVariable {
                    variables_reference,
                    name,
                    value_expression,
                    reply,
                }
            })
            .await;

            match result {
                Ok(var) => {
                    send_response(
                        stdout,
                        seq,
                        msg.seq,
                        "setVariable",
                        Some(serde_json::json!({
                            "value": var.value,
                            "type": var.type_name,
                            "variablesReference": var.variables_reference,
                        })),
                    )
                    .await;
                }
                Err(message) => {
                    send_error(stdout, seq, msg.seq, "setVariable", &message).await;
                }
            }
        }
        "continue" => {
            // Resume commands (Continue/Step) only act while the VM is parked in its
            // command loop (vm_suspended) — there is no paused frame to resume
            // otherwise. The running-mode poll drops exactly Continue/Step (it still
            // honors Pause/Disconnect/queries), so sending one while running would clear
            // vm_suspended without the VM acting on it. When already running we just
            // acknowledge the request without mutating state.
            if state.vm_active && state.vm_suspended {
                if let Some(ref tx) = state.dbg_cmd_tx {
                    let _ = tx.send(DebugCommand::Continue);
                }
                state.vm_suspended = false;
            }
            send_response(
                stdout,
                seq,
                msg.seq,
                "continue",
                Some(serde_json::json!({ "allThreadsContinued": true })),
            )
            .await;
        }
        "next" => {
            if state.vm_active && state.vm_suspended {
                if let Some(ref tx) = state.dbg_cmd_tx {
                    let _ = tx.send(DebugCommand::StepOver);
                }
                state.vm_suspended = false;
            }
            send_response(stdout, seq, msg.seq, "next", None).await;
        }
        "stepIn" => {
            if state.vm_active && state.vm_suspended {
                if let Some(ref tx) = state.dbg_cmd_tx {
                    let _ = tx.send(DebugCommand::StepInto);
                }
                state.vm_suspended = false;
            }
            send_response(stdout, seq, msg.seq, "stepIn", None).await;
        }
        "stepOut" => {
            if state.vm_active && state.vm_suspended {
                if let Some(ref tx) = state.dbg_cmd_tx {
                    let _ = tx.send(DebugCommand::StepOut);
                }
                state.vm_suspended = false;
            }
            send_response(stdout, seq, msg.seq, "stepOut", None).await;
        }
        "pause" => {
            // Only meaningful while RUNNING — Pause asks the running loop to stop at the
            // next instruction. When already suspended the VM has nothing to pause (the
            // command would be a dead no-op the debug-stop loop drops), so skip it.
            if state.vm_active && !state.vm_suspended {
                if let Some(ref tx) = state.dbg_cmd_tx {
                    let _ = tx.send(DebugCommand::Pause);
                }
            }
            send_response(stdout, seq, msg.seq, "pause", None).await;
        }
        "disconnect" => {
            if state.vm_active {
                if let Some(ref tx) = state.dbg_cmd_tx {
                    let _ = tx.send(DebugCommand::Disconnect);
                }
            }
            let _ = backend_tx.send(BackendRequest::Disconnect).await;
            send_response(stdout, seq, msg.seq, "disconnect", None).await;
            return false;
        }
        other => {
            send_error(
                stdout,
                seq,
                msg.seq,
                other,
                &format!("unsupported command: {other}"),
            )
            .await;
        }
    }
    true
}

/// Send a DebugCommand that expects a reply and wait for the response.
/// Encapsulates the sync_channel + spawn_blocking pattern used by all
/// query commands (stackTrace, scopes, variables, setBreakpoints).
async fn send_cmd_and_recv<T: Default + Send + 'static>(
    tx: &std_mpsc::Sender<DebugCommand>,
    make_cmd: impl FnOnce(std_mpsc::SyncSender<T>) -> DebugCommand,
) -> T {
    let (reply_tx, reply_rx) = std_mpsc::sync_channel(1);
    let _ = tx.send(make_cmd(reply_tx));
    tokio::task::spawn_blocking(move || reply_rx.recv().unwrap_or_default())
        .await
        .unwrap_or_default()
}

/// Send a DebugCommand that expects a Result reply (evaluate, setVariable).
async fn send_cmd_and_recv_result<T: Send + 'static>(
    tx: &std_mpsc::Sender<DebugCommand>,
    err_msg: &str,
    make_cmd: impl FnOnce(std_mpsc::SyncSender<Result<T, String>>) -> DebugCommand,
) -> Result<T, String> {
    let (reply_tx, reply_rx) = std_mpsc::sync_channel(1);
    let _ = tx.send(make_cmd(reply_tx));
    let err_msg = err_msg.to_string();
    tokio::task::spawn_blocking(move || reply_rx.recv().unwrap_or_else(|_| Err(err_msg)))
        .await
        .unwrap_or_else(|e| Err(format!("task failed: {e}")))
}

async fn send_response(
    stdout: &mut tokio::io::Stdout,
    seq: &mut u64,
    request_seq: u64,
    command: &str,
    body: Option<serde_json::Value>,
) {
    let resp = DapResponse::success(*seq, request_seq, command, body);
    *seq += 1;
    match serde_json::to_string(&resp) {
        Ok(json) => {
            let _ = transport::write_message(stdout, &json).await;
        }
        Err(e) => eprintln!("DAP: failed to serialize response: {e}"),
    }
}

fn breakpoint_to_json(bp: &DapBreakpoint) -> serde_json::Value {
    let mut value = serde_json::json!({
        "id": bp.id,
        "verified": bp.verified,
        "line": bp.line,
    });
    if let Some(message) = &bp.message {
        value
            .as_object_mut()
            .unwrap()
            .insert("message".to_string(), serde_json::json!(message));
    }
    value
}

async fn send_error(
    stdout: &mut tokio::io::Stdout,
    seq: &mut u64,
    request_seq: u64,
    command: &str,
    message: &str,
) {
    let resp = DapResponse::error(*seq, request_seq, command, message);
    *seq += 1;
    match serde_json::to_string(&resp) {
        Ok(json) => {
            let _ = transport::write_message(stdout, &json).await;
        }
        Err(e) => eprintln!("DAP: failed to serialize error response: {e}"),
    }
}

// --- Backend thread ---

/// Reply to a debug command that arrived after the VM stopped polling, so the
/// frontend's blocking reply wait resolves instead of hanging. Query commands
/// get empty results; evaluate/setVariable get a clear error.
fn reply_session_ended(cmd: DebugCommand) {
    match cmd {
        DebugCommand::SetBreakpoints { reply, .. } => {
            let _ = reply.send(Vec::new());
        }
        DebugCommand::GetStackTrace { reply } => {
            let _ = reply.send(Vec::new());
        }
        DebugCommand::GetScopes { reply, .. } => {
            let _ = reply.send(Vec::new());
        }
        DebugCommand::GetVariables { reply, .. } => {
            let _ = reply.send(Vec::new());
        }
        DebugCommand::Evaluate { reply, .. } => {
            let _ = reply.send(Err("debug session has ended".to_string()));
        }
        DebugCommand::SetVariable { reply, .. } => {
            let _ = reply.send(Err("debug session has ended".to_string()));
        }
        DebugCommand::Continue
        | DebugCommand::StepInto
        | DebugCommand::StepOver
        | DebugCommand::StepOut
        | DebugCommand::Pause
        | DebugCommand::SetExceptionBreakpoints { .. }
        | DebugCommand::Disconnect => {}
    }
}

fn backend_thread(
    mut rx: tokio_mpsc::Receiver<BackendRequest>,
    event_tx: tokio_mpsc::Sender<DebugEvent>,
) {
    let mut vm: Option<sema_vm::VM> = None;
    let mut closure: Option<std::rc::Rc<sema_vm::Closure>> = None;
    let mut debug_state: Option<DebugState> = None;
    let mut interp: Option<sema_eval::Interpreter> = None;
    let mut pending_breakpoints: Vec<(PathBuf, Vec<SourceBreakpoint>)> = Vec::new();
    let mut pending_break_on_uncaught = false;

    loop {
        let req = rx.blocking_recv();
        let Some(req) = req else { break };

        match req {
            BackendRequest::Launch {
                program,
                stop_on_entry,
                cmd_rx,
            } => {
                let source = match std::fs::read_to_string(&program) {
                    Ok(s) => s,
                    Err(e) => {
                        let _ = event_tx.blocking_send(DebugEvent::Output {
                            category: "stderr".to_string(),
                            output: format!("Failed to read {}: {e}\n", program.display()),
                        });
                        let _ = event_tx.blocking_send(DebugEvent::Terminated);
                        continue;
                    }
                };

                let (vals, span_map) = match sema_reader::read_many_with_spans(&source) {
                    Ok(v) => v,
                    Err(e) => {
                        let _ = event_tx.blocking_send(DebugEvent::Output {
                            category: "stderr".to_string(),
                            output: format!("Parse error: {e}\n"),
                        });
                        let _ = event_tx.blocking_send(DebugEvent::Terminated);
                        continue;
                    }
                };

                let prog = match sema_vm::compile_program_with_spans(
                    &vals,
                    &span_map,
                    Some(program.clone()),
                ) {
                    Ok(v) => v,
                    Err(e) => {
                        let _ = event_tx.blocking_send(DebugEvent::Output {
                            category: "stderr".to_string(),
                            output: format!("Compile error: {e}\n"),
                        });
                        let _ = event_tx.blocking_send(DebugEvent::Terminated);
                        continue;
                    }
                };

                // Set up the interpreter environment (provides stdlib, LLM, prelude)
                let interpreter = sema_eval::Interpreter::new();

                // Create the event channel (VM → frontend)
                let (dbg_event_tx, dbg_event_rx) = std_mpsc::channel::<DebugEvent>();

                // Use the command receiver from the frontend
                let mut ds = DebugState::new(dbg_event_tx, cmd_rx);
                ds.set_valid_breakpoint_lines(sema_vm::valid_breakpoint_lines_by_file(
                    &prog.closure,
                    &prog.functions,
                ));

                if stop_on_entry {
                    ds.step_mode = sema_vm::StepMode::StepInto;
                }

                ds.break_on_uncaught = pending_break_on_uncaught;

                // Apply pending breakpoints
                for (file, breakpoints) in pending_breakpoints.drain(..) {
                    ds.set_breakpoints_with_conditions(&file, &breakpoints);
                }

                closure = Some(prog.closure.clone());
                let new_vm = match sema_vm::VM::new(
                    interpreter.global_env.clone(),
                    prog.functions,
                    &[],
                    prog.main_cache_slots,
                ) {
                    Ok(vm) => vm,
                    Err(e) => {
                        let _ = event_tx.blocking_send(DebugEvent::Output {
                            category: "stderr".to_string(),
                            output: format!("VM init error: {e}\n"),
                        });
                        let _ = event_tx.blocking_send(DebugEvent::Terminated);
                        continue;
                    }
                };

                // Forward debug events from std_mpsc to tokio_mpsc in a separate thread
                let event_tx_fwd = event_tx.clone();
                std::thread::spawn(move || {
                    while let Ok(evt) = dbg_event_rx.recv() {
                        if event_tx_fwd.blocking_send(evt).is_err() {
                            break;
                        }
                    }
                });

                // Store state but don't run yet — wait for configurationDone
                debug_state = Some(ds);
                vm = Some(new_vm);
                interp = Some(interpreter);
            }

            BackendRequest::SetBreakpoints {
                file,
                breakpoints,
                reply,
            } => {
                if let Some(ref mut ds) = debug_state {
                    let resolved = ds.set_breakpoints_with_conditions(&file, &breakpoints);
                    let _ = reply.blocking_send(resolved);
                } else {
                    // Store for application at launch time, reply immediately
                    // with pending breakpoints so the frontend doesn't block.
                    let count = breakpoints.len();
                    let pending: Vec<DapBreakpoint> = breakpoints
                        .iter()
                        .enumerate()
                        .map(|(idx, bp)| DapBreakpoint {
                            id: (idx + 1) as u32,
                            verified: false,
                            requested_line: bp.line,
                            line: bp.line,
                            message: Some(
                                "Breakpoint pending until program is compiled".to_string(),
                            ),
                        })
                        .collect();
                    pending_breakpoints.push((file, breakpoints));
                    debug_assert_eq!(pending.len(), count);
                    let _ = reply.blocking_send(pending);
                }
            }

            BackendRequest::SetExceptionBreakpoints { break_on_uncaught } => {
                if let Some(ref mut ds) = debug_state {
                    ds.break_on_uncaught = break_on_uncaught;
                } else {
                    pending_break_on_uncaught = break_on_uncaught;
                }
            }

            BackendRequest::ConfigurationDone => {
                if let (
                    Some(ref mut vm_inst),
                    Some(ref cl),
                    Some(ref mut ds),
                    Some(ref interpreter),
                ) = (&mut vm, &closure, &mut debug_state, &interp)
                {
                    // Redirect program stdout/stderr into DAP Output events so they
                    // don't corrupt the JSON-RPC protocol stream on the server's stdout.
                    let event_tx_stdout = event_tx.clone();
                    sema_core::set_stdout_hook(Some(Box::new(move |s: &str| {
                        let _ = event_tx_stdout.blocking_send(DebugEvent::Output {
                            category: "stdout".to_string(),
                            output: s.to_string(),
                        });
                    })));
                    let event_tx_stderr = event_tx.clone();
                    sema_core::set_stderr_hook(Some(Box::new(move |s: &str| {
                        let _ = event_tx_stderr.blocking_send(DebugEvent::Output {
                            category: "stderr".to_string(),
                            output: s.to_string(),
                        });
                    })));

                    // Mark a debug session active so that tree-walker-evaluated
                    // load/import (which bypass the VM debug loop) can emit a
                    // one-time warning that breakpoints in those files won't
                    // hit. Runs on the same backend thread as execute_debug, so
                    // the thread-local flag is observed by the evaluator. See
                    // §7.4 #4.
                    sema_eval::set_debug_session_active(true);
                    // VM is the active backend under DAP, so (load ...) runs the
                    // loaded file's body on the VM (async/channels work in loaded
                    // files). It runs on a separate, non-debug VM, so breakpoints
                    // inside loaded files still don't hit — the bypass warning
                    // above remains accurate.

                    // Initialize the async scheduler so async/await and channels
                    // work in a debugged program. The program was compiled with
                    // `compile_program_with_spans`, which yields an empty native
                    // table; task VMs resolve natives via the shared global env,
                    // so an empty native table is correct here.
                    sema_vm::init_scheduler(interpreter.global_env.clone(), Vec::new());

                    let result = vm_inst.execute_debug(cl.clone(), &interpreter.ctx, ds);

                    // Clear the hooks immediately after execution so any server-side
                    // prints (e.g. error logging) go back to the real stdout/stderr.
                    sema_eval::set_debug_session_active(false);
                    sema_core::set_stdout_hook(None);
                    sema_core::set_stderr_hook(None);

                    match result {
                        Ok(val) => {
                            if !val.is_nil() {
                                let _ = event_tx.blocking_send(DebugEvent::Output {
                                    category: "stdout".to_string(),
                                    output: format!("{}\n", sema_core::pretty_print(&val, 80)),
                                });
                            }
                        }
                        Err(e) => {
                            let _ = event_tx.blocking_send(DebugEvent::Output {
                                category: "stderr".to_string(),
                                output: format!("Runtime error: {e}\n"),
                            });
                        }
                    }
                    let _ = event_tx.blocking_send(DebugEvent::Terminated);

                    // The VM is no longer polling its command channel. Drain any
                    // commands the frontend sends in the race window before it
                    // processes the Terminated event and replies to each, so a
                    // blocking reply wait on the frontend can never hang on a
                    // never-serviced command. This loop exits once the frontend
                    // drops its command sender (on processing Terminated), which
                    // disconnects the channel.
                    while let Ok(cmd) = ds.command_rx.recv() {
                        reply_session_ended(cmd);
                    }
                }
            }

            BackendRequest::Disconnect => {
                break;
            }
        }
    }
}

fn clean_path(path_str: &str) -> PathBuf {
    let decoded_str = if let Some(rest) = path_str.strip_prefix("file://") {
        decode_percent(rest)
    } else if let Some(rest) = path_str.strip_prefix("file:") {
        decode_percent(rest)
    } else {
        path_str.to_string()
    };

    let clean =
        if cfg!(windows) && decoded_str.starts_with('/') && decoded_str.chars().nth(2) == Some(':')
        {
            &decoded_str[1..]
        } else {
            &decoded_str
        };

    PathBuf::from(clean)
}

/// Decode percent-escapes in a URI path.
///
/// Percent-escapes are decoded into raw bytes (not chars), then the whole byte
/// sequence is interpreted as UTF-8. This is required for correctness with
/// multi-byte UTF-8 characters, which are percent-encoded one byte at a time
/// (e.g. "é" → "%C3%A9"). Decoding each escape directly to a `char` would
/// corrupt such sequences (DAP-9). If the decoded bytes are not valid UTF-8 we
/// fall back to a lossy conversion so a malformed path never aborts the
/// session.
fn decode_percent(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 3 <= bytes.len() {
            if let Some(byte) = hex_to_byte(bytes[i + 1] as char, bytes[i + 2] as char) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

fn hex_to_byte(h1: char, h2: char) -> Option<u8> {
    let b1 = h1.to_digit(16)? as u8;
    let b2 = h2.to_digit(16)? as u8;
    Some((b1 << 4) | b2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_percent_ascii() {
        assert_eq!(decode_percent("a%20b"), "a b");
        assert_eq!(decode_percent("/path/to/file.sema"), "/path/to/file.sema");
    }

    #[test]
    fn decode_percent_multibyte_utf8() {
        // "é" is U+00E9, UTF-8 0xC3 0xA9, percent-encoded one byte at a time.
        assert_eq!(decode_percent("caf%C3%A9.sema"), "café.sema");
        // A 3-byte char: "€" (U+20AC) → 0xE2 0x82 0xAC.
        assert_eq!(decode_percent("%E2%82%AC.sema"), "€.sema");
        // A 4-byte char: "🦀" (U+1F980) → 0xF0 0x9F 0xA6 0x80.
        assert_eq!(decode_percent("%F0%9F%A6%80.sema"), "🦀.sema");
    }

    #[test]
    fn decode_percent_invalid_escape_passthrough() {
        // Non-hex after % is left literal.
        assert_eq!(decode_percent("100%zz"), "100%zz");
        // Trailing percent with no following bytes.
        assert_eq!(decode_percent("abc%"), "abc%");
        // Trailing percent with a single byte.
        assert_eq!(decode_percent("abc%C"), "abc%C");
    }

    #[test]
    fn decode_percent_trailing_escape() {
        // Percent-encoded sequence at end of string must still decode.
        assert_eq!(decode_percent("file%20"), "file ");
        assert_eq!(decode_percent("path%2F"), "path/");
        assert_eq!(decode_percent("caf%C3%A9"), "café");
    }

    #[test]
    fn clean_path_decodes_file_uri_multibyte() {
        let p = clean_path("file:///tmp/caf%C3%A9/main.sema");
        assert_eq!(p, PathBuf::from("/tmp/café/main.sema"));
    }
}
