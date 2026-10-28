//! MCP serving through the public embedding API.
//!
//! Sessions stay on the interpreter thread. Use a worker for async Rust hosts.
pub use sema_mcp::protocol::{CallToolResult, Tool, ToolContent};
pub use sema_mcp::session::{
    CallId, LogEvent, McpCallError, McpCancelHandle, McpError, McpServerOptions, ToolOutput,
};

use crate::Interpreter;

/// An MCP session over a public interpreter. The host owns its transport.
pub struct McpSession(sema_mcp::session::McpSession);

impl McpSession {
    pub fn new(interpreter: &Interpreter, options: McpServerOptions) -> Result<Self, McpError> {
        sema_mcp::session::McpSession::new(
            &interpreter.inner,
            options.notebook_features(
                interpreter.features.0,
                interpreter.features.1,
                interpreter.features.2,
            ),
        )
        .map(Self)
    }
    pub fn list_tools(&self, interpreter: &Interpreter) -> Vec<Tool> {
        self.0.list_tools(&interpreter.inner)
    }
    pub fn call_tool(
        &self,
        interpreter: &Interpreter,
        name: &str,
        args: &serde_json::Value,
    ) -> Result<CallToolResult, McpCallError> {
        self.0.call_tool(&interpreter.inner, name, args)
    }
    pub fn call_tool_with_id(
        &self,
        interpreter: &Interpreter,
        id: CallId,
        name: &str,
        args: &serde_json::Value,
    ) -> Result<CallToolResult, McpCallError> {
        self.0.call_tool_with_id(&interpreter.inner, id, name, args)
    }
    pub fn cancel_handle(&self) -> McpCancelHandle {
        self.0.cancel_handle()
    }
    pub fn handle_message(
        &self,
        interpreter: &Interpreter,
        frame: &[u8],
        writer: &mut dyn std::io::Write,
    ) -> Result<(), McpError> {
        self.0.handle_message(&interpreter.inner, frame, writer)
    }
    pub fn serve_on(
        &self,
        interpreter: &Interpreter,
        reader: impl std::io::BufRead,
        writer: impl std::io::Write,
    ) -> Result<(), McpError> {
        self.0.serve_on(&interpreter.inner, reader, writer)
    }
    pub fn serve_stdio(&self, interpreter: &Interpreter) -> Result<(), McpError> {
        self.0.serve_stdio(&interpreter.inner)
    }
}

use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};
use tokio::sync::{mpsc, oneshot};

enum Request {
    Call {
        id: CallId,
        name: String,
        args: serde_json::Value,
        reply: oneshot::Sender<Result<CallToolResult, McpCallError>>,
    },
    List(oneshot::Sender<Vec<Tool>>),
    Message(Vec<u8>, oneshot::Sender<Result<Vec<u8>, McpError>>),
    Stop,
}

/// Cloneable async access to an interpreter on a dedicated native thread.
#[derive(Clone)]
pub struct McpWorkerHandle {
    sender: mpsc::Sender<Request>,
    cancel: McpCancelHandle,
    stopped: Arc<AtomicBool>,
    next_id: Arc<AtomicU64>,
    max_frame_bytes: usize,
}

impl McpWorkerHandle {
    pub fn next_call_id(&self) -> CallId {
        CallId::from(self.next_id.fetch_add(1, Ordering::Relaxed))
    }
    pub fn cancel(&self, id: CallId) -> bool {
        self.cancel.cancel(id)
    }
    pub async fn call_tool(
        &self,
        name: impl Into<String>,
        args: serde_json::Value,
    ) -> Result<CallToolResult, McpCallError> {
        self.call_tool_with_id(self.next_call_id(), name, args)
            .await
    }
    pub async fn call_tool_with_id(
        &self,
        id: CallId,
        name: impl Into<String>,
        args: serde_json::Value,
    ) -> Result<CallToolResult, McpCallError> {
        if self.stopped.load(Ordering::Acquire) {
            return Err(McpCallError::WorkerClosed);
        }
        let (reply, response) = oneshot::channel();
        self.sender
            .send(Request::Call {
                id,
                name: name.into(),
                args,
                reply,
            })
            .await
            .map_err(|_| McpCallError::WorkerClosed)?;
        response.await.map_err(|_| McpCallError::WorkerClosed)?
    }
    pub async fn list_tools(&self) -> Result<Vec<Tool>, McpError> {
        let (reply, response) = oneshot::channel();
        self.sender
            .send(Request::List(reply))
            .await
            .map_err(|_| McpError::Worker("worker is closed".into()))?;
        response
            .await
            .map_err(|_| McpError::Worker("worker is closed".into()))
    }
    pub async fn handle_message(&self, frame: Vec<u8>) -> Result<Vec<u8>, McpError> {
        if frame.len() <= self.max_frame_bytes {
            if let Ok(message) = serde_json::from_slice::<serde_json::Value>(&frame) {
                if message.get("method").and_then(|v| v.as_str()) == Some("notifications/cancelled")
                {
                    if let Some(id) = message
                        .get("params")
                        .and_then(|p| p.get("requestId"))
                        .and_then(CallId::from_json_id)
                    {
                        self.cancel(id);
                    }
                    return Ok(Vec::new());
                }
            }
        }
        let (reply, response) = oneshot::channel();
        self.sender
            .send(Request::Message(frame, reply))
            .await
            .map_err(|_| McpError::Worker("worker is closed".into()))?;
        response
            .await
            .map_err(|_| McpError::Worker("worker is closed".into()))?
    }
}

/// Owns the worker thread. Dropping it requests cooperative cancellation and shutdown.
pub struct McpWorker {
    handle: McpWorkerHandle,
    thread: Option<std::thread::JoinHandle<()>>,
    done: Option<oneshot::Receiver<()>>,
}

impl McpWorker {
    /// Build and configure the interpreter on its owning thread.
    pub fn spawn(
        builder: crate::InterpreterBuilder,
        options: McpServerOptions,
        setup: impl FnOnce(&Interpreter) -> crate::Result<()> + Send + 'static,
    ) -> Result<Self, McpError> {
        let max_frame_bytes = options.max_frame_bytes_limit();
        let (sender, mut requests) = mpsc::channel(64);
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let (done_tx, done) = oneshot::channel();
        let stopped = Arc::new(AtomicBool::new(false));
        let worker_stopped = stopped.clone();
        let thread = sema_eval::spawn_host_thread("sema-mcp", move || {
            {
                let interp = builder.build();
                if let Err(error) = setup(&interp) {
                    let _ = ready_tx.send(Err(McpError::Worker(error.format_plain())));
                    return;
                }
                let session = match McpSession::new(&interp, options) {
                    Ok(session) => session,
                    Err(error) => {
                        let _ = ready_tx.send(Err(error));
                        return;
                    }
                };
                if ready_tx.send(Ok(session.cancel_handle())).is_err() {
                    return;
                }
                while let Some(request) = requests.blocking_recv() {
                    if worker_stopped.load(Ordering::Acquire) {
                        break;
                    }
                    match request {
                        Request::Call {
                            id,
                            name,
                            args,
                            reply,
                        } => {
                            let _ =
                                reply.send(session.call_tool_with_id(&interp, id, &name, &args));
                        }
                        Request::List(reply) => {
                            let _ = reply.send(session.list_tools(&interp));
                        }
                        Request::Message(frame, reply) => {
                            let mut output = Vec::new();
                            let result = session
                                .handle_message(&interp, &frame, &mut output)
                                .map(|()| output);
                            let _ = reply.send(result);
                        }
                        Request::Stop => break,
                    }
                }
            }
            let _ = done_tx.send(());
        })
        .map_err(McpError::Transport)?;
        let cancel = match ready_rx.recv() {
            Ok(Ok(cancel)) => cancel,
            Ok(Err(error)) => {
                let _ = thread.join();
                return Err(error);
            }
            Err(_) => {
                let _ = thread.join();
                return Err(McpError::Worker("worker stopped during setup".into()));
            }
        };
        Ok(Self {
            handle: McpWorkerHandle {
                sender,
                cancel,
                stopped,
                next_id: Arc::new(AtomicU64::new(1)),
                max_frame_bytes,
            },
            thread: Some(thread),
            done: Some(done),
        })
    }
    pub fn handle(&self) -> McpWorkerHandle {
        self.handle.clone()
    }
    fn request_stop(&self) {
        self.handle.stopped.store(true, Ordering::Release);
        self.handle.cancel.close();
        let _ = self.handle.sender.try_send(Request::Stop);
    }
    pub async fn shutdown(mut self) -> Result<(), McpError> {
        self.request_stop();
        if let Some(done) = self.done.take() {
            let _ = done.await;
        }
        if let Some(thread) = self.thread.take() {
            thread
                .join()
                .map_err(|_| McpError::Worker("worker panicked".into()))?;
        }
        Ok(())
    }
}
impl Drop for McpWorker {
    fn drop(&mut self) {
        self.request_stop();
    }
}
