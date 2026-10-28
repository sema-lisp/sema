//! Transport-independent MCP sessions for native hosts.
use std::cell::Cell;
use std::collections::BTreeSet;
use std::io::{BufRead, Read, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sema_core::runtime::RootId;
use sema_core::{SemaError, Value, ValueViewRef};
use sema_eval::Interpreter;
use sema_vm::runtime::{RootHandle, RootOptions, RuntimeCommandHandle};
use serde_json::{json, Value as Json};

use crate::notebook::{new_cache_with_factory, NotebookCache};
use crate::protocol::{CallToolResult, JsonRpcRequest, Tool};

/// Failure to configure or serve a session.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum McpError {
    #[error("invalid MCP options: {0}")]
    InvalidOptions(String),
    #[error("MCP transport: {0}")]
    Transport(#[from] std::io::Error),
    #[error("MCP session belongs to another interpreter")]
    InterpreterMismatch,
    #[error("MCP session is already executing a call")]
    Reentrant,
    #[error("MCP worker: {0}")]
    Worker(String),
}

/// A dispatch failure. Evaluation errors from a tool use `CallToolResult::is_error`.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum McpCallError {
    #[error("unknown tool: {0}")]
    UnknownTool(String),
    #[error("tool is not allowed: {0}")]
    NotAllowed(String),
    #[error("invalid arguments: {0}")]
    InvalidArguments(String),
    #[error("tool exceeded its time budget")]
    Timeout,
    #[error("tool panicked: {0}")]
    Panicked(String),
    #[error("MCP session is already executing a call")]
    Reentrant,
    #[error("MCP session belongs to another interpreter")]
    InterpreterMismatch,
    #[error("MCP worker is closed")]
    WorkerClosed,
    #[error("MCP runtime: {0}")]
    Runtime(String),
}

impl From<McpCallError> for CallToolResult {
    fn from(error: McpCallError) -> Self {
        Self::error(error.to_string())
    }
}

/// A host-chosen call identity, independent of the runtime's private root allocation.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CallId {
    Number(serde_json::Number),
    Text(String),
}
impl From<u64> for CallId {
    fn from(id: u64) -> Self {
        Self::Number(id.into())
    }
}
impl From<String> for CallId {
    fn from(id: String) -> Self {
        Self::Text(id)
    }
}
impl From<&str> for CallId {
    fn from(id: &str) -> Self {
        Self::Text(id.into())
    }
}
impl CallId {
    pub fn from_json_id(id: &Json) -> Option<Self> {
        match id {
            Json::Number(id) => Some(Self::Number(id.clone())),
            Json::String(id) => Some(Self::Text(id.clone())),
            _ => None,
        }
    }
}

/// A cancellation handle that can cross threads without moving Sema values.
#[derive(Clone)]
pub struct McpCancelHandle {
    command: RuntimeCommandHandle,
    running: Arc<Mutex<Option<(CallId, RootId)>>>,
    closed: Arc<std::sync::atomic::AtomicBool>,
}

impl McpCancelHandle {
    /// Close the session and cancel active work. Future calls are rejected.
    pub fn close(&self) {
        self.closed
            .store(true, std::sync::atomic::Ordering::Release);
        self.cancel_running();
    }
    /// Cancel the active call, for worker shutdown.
    pub fn cancel_running(&self) -> bool {
        let running = self.running.lock().unwrap_or_else(|e| e.into_inner());
        running
            .as_ref()
            .is_some_and(|(_, root)| self.command.cancel_root(*root))
    }
    /// Cancel this call only. A settled or unrelated identity is inert.
    pub fn cancel(&self, id: CallId) -> bool {
        let running = self.running.lock().unwrap_or_else(|e| e.into_inner());
        running
            .as_ref()
            .is_some_and(|(current, root)| *current == id && self.command.cancel_root(*root))
    }
}

/// Session lifecycle events. Request contents are never included.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum LogEvent {
    Started,
    Stopped,
    WriteFailed,
    ToolShadowed(String),
    ToolExcluded(String),
}

/// Where program output goes. Captured output remains separate from protocol frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolOutput {
    Capture,
    PassThrough,
    Tee,
}

type BuildHook = Arc<dyn Fn(&Interpreter, &Json) -> CallToolResult + Send + Sync>;
type OutputCallback = Arc<dyn Fn(&str) + Send + Sync>;
type LogCallback = Arc<dyn Fn(LogEvent) + Send + Sync>;

/// Explicit tool selection and resource limits for one MCP session.
#[derive(Clone)]
pub struct McpServerOptions {
    cli: bool,
    eval: bool,
    also: Vec<String>,
    only: Option<Vec<String>>,
    exclude: Vec<String>,
    pub(crate) timeout: Option<Duration>,
    pub(crate) max_frame_bytes: usize,
    pub(crate) max_source_bytes: usize,
    pub(crate) max_output_bytes: usize,
    server_name: String,
    server_version: String,
    output: ToolOutput,
    on_output: Option<OutputCallback>,
    on_log: Option<LogCallback>,
    build: Option<BuildHook>,
    notebook_features: Option<(bool, bool, bool)>,
}

impl Default for McpServerOptions {
    fn default() -> Self {
        Self::embedded()
    }
}

impl McpServerOptions {
    /// Configure notebook interpreters to match the host's registered feature sets.
    pub fn notebook_features(mut self, stdlib: bool, llm: bool, mcp: bool) -> Self {
        self.notebook_features = Some((stdlib, llm, mcp));
        self
    }
    /// The frame limit enforced by transport adapters and worker handles.
    pub fn max_frame_bytes_limit(&self) -> usize {
        self.max_frame_bytes
    }
    /// Expose docs, docs_search, info, and visible deftools. Eval is opt-in.
    pub fn embedded() -> Self {
        Self {
            cli: false,
            eval: false,
            also: Vec::new(),
            only: None,
            exclude: Vec::new(),
            timeout: Some(Duration::from_secs(300)),
            max_frame_bytes: 16 * 1024 * 1024,
            max_source_bytes: 16 * 1024 * 1024,
            max_output_bytes: 1024 * 1024,
            server_name: "sema-mcp".into(),
            server_version: env!("CARGO_PKG_VERSION").into(),
            output: ToolOutput::Capture,
            on_output: None,
            on_log: None,
            build: None,
            notebook_features: None,
        }
    }
    /// The CLI tool preset. Build is available only with an explicit build hook.
    pub fn cli() -> Self {
        Self {
            cli: true,
            eval: true,
            ..Self::embedded()
        }
    }
    /// Allow clients to evaluate code with the interpreter's full host access.
    pub fn enable_eval(mut self) -> Self {
        self.eval = true;
        self
    }
    pub fn also_tools(mut self, names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.also.extend(names.into_iter().map(Into::into));
        self
    }
    pub fn only_tools(mut self, names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.only = Some(names.into_iter().map(Into::into).collect());
        self
    }
    pub fn exclude_tools(mut self, names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.exclude.extend(names.into_iter().map(Into::into));
        self
    }
    pub fn server_info(mut self, name: impl Into<String>, version: impl Into<String>) -> Self {
        self.server_name = name.into();
        self.server_version = version.into();
        self
    }
    pub fn timeout(mut self, timeout: Option<Duration>) -> Self {
        self.timeout = timeout.filter(|d| !d.is_zero());
        self
    }
    pub fn max_frame_bytes(mut self, bytes: usize) -> Self {
        self.max_frame_bytes = bytes;
        self
    }
    pub fn max_source_bytes(mut self, bytes: usize) -> Self {
        self.max_source_bytes = bytes;
        self
    }
    pub fn max_output_bytes(mut self, bytes: usize) -> Self {
        self.max_output_bytes = bytes;
        self
    }
    pub fn output(mut self, output: ToolOutput) -> Self {
        self.output = output;
        self
    }
    /// The callback runs on the interpreter thread and must not block or evaluate Sema.
    pub fn on_output(mut self, callback: impl Fn(&str) + Send + Sync + 'static) -> Self {
        self.on_output = Some(Arc::new(callback));
        self
    }
    /// The callback runs on the interpreter thread and must not block or evaluate Sema.
    pub fn on_log(mut self, callback: impl Fn(LogEvent) + Send + Sync + 'static) -> Self {
        self.on_log = Some(Arc::new(callback));
        self
    }
    /// Install an in-process build implementation. The caller must preserve the sandbox.
    pub fn with_build_hook(
        mut self,
        hook: impl Fn(&Interpreter, &Json) -> CallToolResult + Send + Sync + 'static,
    ) -> Self {
        self.build = Some(Arc::new(hook));
        self
    }
}

/// Session state. Methods verify interpreter identity before using cached resources.
pub struct McpSession {
    options: McpServerOptions,
    identity: sema_core::runtime::RuntimeId,
    cancel: McpCancelHandle,
    notebooks: NotebookCache,
    in_call: Cell<bool>,
    next_id: Cell<u64>,
}

struct CallGuard<'a>(&'a McpSession, &'a sema_core::EvalContext, Option<Instant>);
impl Drop for CallGuard<'_> {
    fn drop(&mut self) {
        self.1.set_eval_deadline(self.2);
        self.0.in_call.set(false);
        *self
            .0
            .cancel
            .running
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
    }
}

impl McpSession {
    pub(crate) fn new_with_cache(
        interpreter: &Interpreter,
        options: McpServerOptions,
        cache: NotebookCache,
    ) -> Result<Self, McpError> {
        let mut session = Self::new(interpreter, options)?;
        crate::notebook::configure_cache(&cache, &session.notebooks);
        session.notebooks = cache;
        Ok(session)
    }

    pub fn new(interpreter: &Interpreter, options: McpServerOptions) -> Result<Self, McpError> {
        if options.max_frame_bytes == 0
            || options.max_source_bytes == 0
            || options.max_output_bytes == 0
        {
            return Err(McpError::InvalidOptions(
                "byte limits must be positive".into(),
            ));
        }
        let mut known: BTreeSet<String> = crate::tools::builtin_tools()
            .iter()
            .map(|t| t.name.clone())
            .collect();
        interpreter.global_env.iter_bindings(|_, value| {
            if let ValueViewRef::ToolDef(td) = value.view_ref() {
                known.insert(td.name.clone());
            }
        });
        if !options.eval
            && options
                .also
                .iter()
                .chain(options.only.iter().flatten())
                .any(|name| name == "eval")
        {
            return Err(McpError::InvalidOptions(
                "enable_eval() is required to expose eval".into(),
            ));
        }
        for name in options
            .also
            .iter()
            .chain(options.only.iter().flatten())
            .chain(&options.exclude)
        {
            if !known.contains(name.as_str()) {
                return Err(McpError::InvalidOptions(format!("unknown tool: {name}")));
            }
            if name == "build" && options.build.is_none() {
                return Err(McpError::InvalidOptions(
                    "build requires a build hook".into(),
                ));
            }
            if matches!(name.as_str(), "notebook/eval_cell" | "notebook/eval_all")
                && !interpreter.ctx.sandbox.is_unrestricted()
            {
                return Err(McpError::InvalidOptions(
                    "notebook evaluation requires an unrestricted session".into(),
                ));
            }
        }
        // Runtime identity is stable without retaining the interpreter's environment.
        let identity = interpreter.runtime().runtime_id();
        let sandbox = interpreter.ctx.sandbox.clone();
        let stdlib = interpreter
            .global_env
            .get(sema_core::intern("print"))
            .is_some();
        let llm = interpreter
            .global_env
            .get(sema_core::intern("llm/configure"))
            .is_some();
        let mcp = interpreter
            .global_env
            .get(sema_core::intern("mcp/connect"))
            .is_some();
        let (stdlib, llm, mcp) = options.notebook_features.unwrap_or((stdlib, llm, mcp));
        let timeout = options.timeout;
        let notebooks = new_cache_with_factory(move |notebook| {
            let inner = Interpreter::new_host(&sandbox, stdlib, llm);
            if mcp {
                crate::register_mcp_builtins(&inner.global_env, &sandbox);
            }
            let mut engine = sema_notebook::Engine::from_interpreter(notebook, inner);
            engine.set_cell_timeout(timeout);
            engine
        });
        let session = Self {
            options,
            identity,
            cancel: McpCancelHandle {
                command: interpreter.command_handle(),
                running: Arc::new(Mutex::new(None)),
                closed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            },
            notebooks,
            in_call: Cell::new(false),
            next_id: Cell::new(1),
        };
        Ok(session)
    }

    fn same_interpreter(&self, interp: &Interpreter) -> bool {
        self.identity == interp.runtime().runtime_id()
    }
    pub fn cancel_handle(&self) -> McpCancelHandle {
        self.cancel.clone()
    }
    fn log(&self, event: LogEvent) {
        if let Some(callback) = &self.options.on_log {
            callback(event);
        }
    }

    pub fn list_tools(&self, interpreter: &Interpreter) -> Vec<Tool> {
        if !self.same_interpreter(interpreter) {
            return Vec::new();
        }
        interpreter.global_env.iter_bindings(|_, value| {
            if let ValueViewRef::ToolDef(td) = value.view_ref() {
                if crate::tools::is_builtin_name(&td.name) || td.name.starts_with('_') {
                    self.log(LogEvent::ToolShadowed(td.name.clone()));
                }
            }
        });
        crate::tools::list_mcp_tools(interpreter, None, None)
            .into_iter()
            .filter(|tool| self.allowed(&tool.name, interpreter))
            .collect()
    }

    fn allowed(&self, name: &str, interpreter: &Interpreter) -> bool {
        if name.starts_with('_') || self.options.exclude.iter().any(|n| n == name) {
            return false;
        }
        if name == "eval" && !self.options.eval {
            return false;
        }
        if name == "build" && self.options.build.is_none() {
            return false;
        }
        if let Some(only) = &self.options.only {
            if !only.iter().any(|n| n == name) {
                return false;
            }
        }
        if !self.options.cli
            && crate::tools::is_builtin_name(name)
            && !matches!(name, "docs" | "docs_search" | "info")
            && !(name == "eval" && self.options.eval)
            && !self.options.also.iter().any(|n| n == name)
        {
            return false;
        }
        if matches!(name, "notebook/eval_cell" | "notebook/eval_all")
            && !interpreter.ctx.sandbox.is_unrestricted()
        {
            return false;
        }
        true
    }

    pub fn call_tool(
        &self,
        interp: &Interpreter,
        name: &str,
        args: &Json,
    ) -> Result<CallToolResult, McpCallError> {
        let raw = self.next_id.get();
        let id = CallId::from(raw);
        self.next_id.set(raw.wrapping_add(1));
        self.call_tool_with_id(interp, id, name, args)
    }

    pub fn call_tool_with_id(
        &self,
        interp: &Interpreter,
        id: CallId,
        name: &str,
        args: &Json,
    ) -> Result<CallToolResult, McpCallError> {
        if self
            .cancel
            .closed
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return Err(McpCallError::WorkerClosed);
        }
        if !self.same_interpreter(interp) {
            return Err(McpCallError::InterpreterMismatch);
        }
        if self.in_call.get()
            || sema_core::in_runtime_quantum()
            || interp.ctx.runtime_quantum_active()
        {
            return Err(McpCallError::Reentrant);
        }
        if !args.is_object() {
            return Err(McpCallError::InvalidArguments(
                "arguments must be an object".into(),
            ));
        }
        if !self.allowed(name, interp) {
            return Err(McpCallError::NotAllowed(name.into()));
        }
        if !self.list_tools(interp).iter().any(|t| t.name == name) {
            return Err(McpCallError::UnknownTool(name.into()));
        }
        let budget = crate::tools::resolve_call_timeout(args, self.options.timeout)
            .map_err(|message| McpCallError::InvalidArguments(message.into()))?;
        let previous = interp.ctx.eval_deadline.get();
        let deadline = budget.and_then(|d| Instant::now().checked_add(d));
        let deadline = match (previous, deadline) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        self.in_call.set(true);
        let _guard = CallGuard(self, &interp.ctx, previous);
        interp.ctx.set_eval_deadline(deadline);
        let callback = self.options.on_output.clone();
        let output = self.options.output;
        let stdout = Box::new(move |text: &str| {
            if let Some(callback) = &callback {
                callback(text);
            } else if output != ToolOutput::Capture {
                print!("{text}");
            }
        });
        let callback = self.options.on_output.clone();
        let stderr = Box::new(move |text: &str| {
            if let Some(callback) = &callback {
                callback(text);
            } else if output != ToolOutput::Capture {
                eprint!("{text}");
            }
        });
        let _hooks = sema_core::output_hook::HostHookScope::new(Some(stdout), Some(stderr));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.dispatch(interp, id, name, args)
        }));
        let timed_out = interp.ctx.deadline_exceeded();
        let result = result.unwrap_or_else(|p| {
            self.cancel.cancel_running();
            Err(McpCallError::Panicked(
                p.downcast_ref::<String>()
                    .cloned()
                    .or_else(|| p.downcast_ref::<&str>().map(|s| (*s).into()))
                    .unwrap_or_else(|| "unknown panic".into()),
            ))
        });
        let result = if timed_out {
            Err(McpCallError::Timeout)
        } else {
            result
        };
        interp.ctx.set_eval_deadline(previous);
        if result.is_err() {
            // A failed call must not leave its captured output for the next caller.
            for event in interp.take_output() {
                let (text, is_stderr) = match event {
                    sema_vm::runtime::OutputEvent::Stdout { text, .. } => (text, false),
                    sema_vm::runtime::OutputEvent::Stderr { text, .. } => (text, true),
                };
                self.emit_output(&text, is_stderr);
            }
        }
        // Detached work gets a bounded turn outside the next call's deadline.
        match interp.drive_turn() {
            Err(error) => return Err(McpCallError::Runtime(error.format_plain())),
            Ok(_) => {
                for event in interp.take_output() {
                    let (text, is_stderr) = match event {
                        sema_vm::runtime::OutputEvent::Stdout { text, .. } => (text, false),
                        sema_vm::runtime::OutputEvent::Stderr { text, .. } => (text, true),
                    };
                    self.emit_output(&text, is_stderr);
                }
            }
        }
        result.map(|mut result| {
            for content in &mut result.content {
                let crate::protocol::ToolContent::Text { text } = content;
                if text.len() > self.options.max_output_bytes {
                    *text = BoundedText::new(std::mem::take(text), self.options.max_output_bytes)
                        .finish();
                }
            }
            result
        })
    }

    fn emit_output(&self, text: &str, is_stderr: bool) {
        if let Some(callback) = &self.options.on_output {
            callback(text);
        } else if self.options.output != ToolOutput::Capture {
            if is_stderr {
                eprint!("{text}");
            } else {
                print!("{text}");
            }
        }
    }

    fn run_root(
        &self,
        interp: &Interpreter,
        id: CallId,
        root: RootHandle,
    ) -> Result<Value, SemaError> {
        sema_core::output_hook::limit_root_output(root.id(), self.options.max_output_bytes);
        *self
            .cancel
            .running
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some((id.clone(), root.id()));
        if self
            .cancel
            .closed
            .load(std::sync::atomic::Ordering::Acquire)
        {
            self.cancel.cancel(id);
        }
        interp.drive_until_settled_isolated(&root)
    }

    fn dispatch(
        &self,
        interp: &Interpreter,
        id: CallId,
        name: &str,
        args: &Json,
    ) -> Result<CallToolResult, McpCallError> {
        if let Err(result) = crate::tools::check_tool_access(name, args, interp) {
            return Ok(result);
        }
        let source_key = match name {
            "compile" => Some("source_path"),
            "fmt" | "disasm" => Some("file_path"),
            n if n.starts_with("notebook/") && n != "notebook/new" => Some("path"),
            _ => None,
        };
        if let Some(path) = source_key
            .and_then(|key| args.get(key))
            .and_then(Json::as_str)
        {
            let metadata = std::fs::metadata(path)
                .map_err(|e| McpCallError::InvalidArguments(e.to_string()))?;
            if !metadata.is_file() || metadata.len() > self.options.max_source_bytes as u64 {
                return Ok(CallToolResult::error(
                    "source must be a regular file within max_source_bytes",
                ));
            }
        }
        let opts = RootOptions {
            capture_output: true,
            ..Default::default()
        };
        let result = match name {
            "eval" => {
                let code = args
                    .get("code")
                    .and_then(Json::as_str)
                    .ok_or_else(|| McpCallError::InvalidArguments("missing code".into()))?;
                if code.len() > self.options.max_source_bytes {
                    return Err(McpCallError::InvalidArguments(
                        "code exceeds max_source_bytes".into(),
                    ));
                }
                // __vm-eval performs macro expansion inside the runtime root.
                let forms = sema_reader::read_many(code);
                forms.and_then(|mut forms| {
                    forms.insert(0, Value::symbol("begin"));
                    let expr = Value::list(vec![
                        Value::symbol("eval"),
                        Value::list(vec![Value::symbol("quote"), Value::list(forms)]),
                    ]);
                    interp
                        .submit_value(expr, opts)
                        .and_then(|root| self.run_root(interp, id, root))
                })
            }
            "run_file" => {
                let path = args
                    .get("file_path")
                    .and_then(Json::as_str)
                    .ok_or_else(|| McpCallError::InvalidArguments("missing file_path".into()))?;
                let path = std::path::Path::new(path);
                let bytes =
                    match crate::tools::read_source_bytes(path, self.options.max_source_bytes) {
                        Ok(bytes) => bytes,
                        Err(error) => return Ok(CallToolResult::error(error)),
                    };
                let _args = match crate::tools::SysArgsGuard::install(interp, args) {
                    Ok(guard) => guard,
                    Err(error) => return Ok(error),
                };
                struct FileScope<'a>(&'a sema_core::EvalContext);
                impl Drop for FileScope<'_> {
                    fn drop(&mut self) {
                        self.0.pop_file_path();
                    }
                }
                let _file = path.canonicalize().ok().map(|path| {
                    interp.ctx.push_file_path(path);
                    FileScope(&interp.ctx)
                });
                let root = if sema_vm::is_bytecode_file(&bytes) {
                    sema_vm::deserialize_from_bytes(&bytes)
                        .and_then(|compiled| interp.submit_compile_result(compiled, opts))
                } else {
                    std::str::from_utf8(&bytes)
                        .map_err(|e| SemaError::eval(e.to_string()))
                        .and_then(|source| {
                            let (mut forms, spans) = sema_reader::read_many_with_spans(source)?;
                            interp.ctx.merge_span_table(spans);
                            forms.insert(0, Value::symbol("begin"));
                            let expr = Value::list(vec![
                                Value::symbol("eval"),
                                Value::list(vec![Value::symbol("quote"), Value::list(forms)]),
                            ]);
                            interp.submit_value(expr, opts)
                        })
                };
                root.and_then(|root| self.run_root(interp, id, root))
            }
            "build" => {
                return Ok(self
                    .options
                    .build
                    .as_ref()
                    .expect("build checked before dispatch")(
                    interp, args
                ))
            }
            _ if crate::tools::is_builtin_name(name) => {
                if name == "docs" {
                    if let Some(symbol) = args.get("symbol").and_then(Json::as_str) {
                        if let Some(value) = interp.global_env.get(sema_core::intern(symbol)) {
                            if let ValueViewRef::ToolDef(td) = value.view_ref() {
                                if !crate::tools::is_exposed(td) {
                                    return Err(McpCallError::NotAllowed(symbol.into()));
                                }
                            }
                        }
                    }
                }
                return Ok(crate::tools::call_host_tool(
                    name,
                    args,
                    interp,
                    &self.notebooks,
                    Some(&[name.into()]),
                    None,
                ));
            }
            _ => {
                let value = interp
                    .global_env
                    .get(sema_core::intern(name))
                    .ok_or_else(|| McpCallError::UnknownTool(name.into()))?;
                let ValueViewRef::ToolDef(td) = value.view_ref() else {
                    return Err(McpCallError::UnknownTool(name.into()));
                };
                if !crate::tools::is_exposed(td) {
                    return Err(McpCallError::NotAllowed(name.into()));
                }
                let arguments = crate::tools::json_args_to_sema(&td.parameters, args, &td.handler)
                    .map_err(McpCallError::InvalidArguments)?;
                let quote = |v| Value::list(vec![Value::symbol("quote"), v]);
                let mut call = vec![quote(td.handler.clone())];
                call.extend(arguments.into_iter().map(quote));
                interp
                    .submit_value(Value::list(call), opts)
                    .and_then(|root| self.run_root(interp, id, root))
            }
        };
        let mut text = String::new();
        for event in interp.take_output() {
            let (chunk, is_stderr) = match event {
                sema_vm::runtime::OutputEvent::Stdout { text, .. } => (text, false),
                sema_vm::runtime::OutputEvent::Stderr { text, .. } => (text, true),
            };
            if self.options.output != ToolOutput::PassThrough {
                text.push_str(&chunk);
            }
            if self.options.output != ToolOutput::Capture {
                self.emit_output(&chunk, is_stderr);
            }
        }
        match result {
            Ok(value) => {
                use std::fmt::Write as _;
                let mut bounded = BoundedText::new(text, self.options.max_output_bytes);
                if matches!(name, "eval" | "run_file") && !value.is_nil() {
                    let _ =
                        std::fmt::Write::write_fmt(&mut bounded, format_args!("\nResult: {value}"));
                } else if let Some(s) = value.as_str() {
                    let _ = bounded.write_str(s);
                } else if value.as_seq().is_some() || value.is_map() {
                    let _ = serde_json::to_writer(&mut bounded, &JsonText(&value, 0));
                } else if !value.is_nil() {
                    let _ = std::fmt::Write::write_fmt(&mut bounded, format_args!("{value}"));
                }
                Ok(CallToolResult::text(bounded.finish()))
            }
            Err(error) => {
                text.push_str("\nError: ");
                text.push_str(&error.format_plain());
                Ok(CallToolResult::error(text))
            }
        }
    }

    /// Handle one complete frame without taking ownership of the host's transport.
    pub fn handle_message(
        &self,
        interp: &Interpreter,
        frame: &[u8],
        writer: &mut dyn Write,
    ) -> Result<(), McpError> {
        if !self.same_interpreter(interp) {
            return Err(McpError::InterpreterMismatch);
        }
        if self.in_call.get()
            || sema_core::in_runtime_quantum()
            || interp.ctx.runtime_quantum_active()
        {
            return Err(McpError::Reentrant);
        }
        let response = if frame.len() > self.options.max_frame_bytes {
            Some(
                json!({"jsonrpc":"2.0","id":null,"error":{"code":-32600,"message":"request exceeds max_frame_bytes"}}),
            )
        } else {
            match parse_request(frame) {
                Err(error) => Some(error),
                Ok(req) => {
                    let Some(id) = req.id else {
                        return Ok(());
                    };
                    let result = match req.method.as_str() {
                        "initialize" => Ok(
                            json!({"protocolVersion":"2024-11-05","capabilities":{"tools":{"listChanged":false}},"serverInfo":{"name":self.options.server_name,"version":self.options.server_version}}),
                        ),
                        "ping" => Ok(json!({})),
                        "tools/list" => Ok(json!({"tools":self.list_tools(interp)})),
                        "tools/call" => {
                            let params = req.params.as_ref();
                            let name = params
                                .and_then(|p| p.get("name"))
                                .and_then(Json::as_str)
                                .unwrap_or("");
                            let empty = json!({});
                            let args = params.and_then(|p| p.get("arguments")).unwrap_or(&empty);
                            let result = self
                                .call_tool_with_id(
                                    interp,
                                    CallId::from_json_id(&id).unwrap_or_else(|| CallId::from(0)),
                                    name,
                                    args,
                                )
                                .unwrap_or_else(Into::into);
                            Ok(json!(result))
                        }
                        _ => Err(
                            json!({"code":-32601,"message":format!("Method not found: {}",req.method)}),
                        ),
                    };
                    Some(match result {
                        Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
                        Err(error) => json!({"jsonrpc":"2.0","id":id,"error":error}),
                    })
                }
            }
        };
        if let Some(response) = response {
            let write = (|| -> std::io::Result<()> {
                serde_json::to_writer(&mut *writer, &response).map_err(std::io::Error::other)?;
                writer.write_all(b"\n")?;
                writer.flush()
            })();
            if write.is_err() {
                self.log(LogEvent::WriteFailed);
            }
            write?;
        }
        Ok(())
    }

    pub fn serve_on(
        &self,
        interp: &Interpreter,
        mut reader: impl BufRead,
        mut writer: impl Write,
    ) -> Result<(), McpError> {
        if !self.same_interpreter(interp) {
            return Err(McpError::InterpreterMismatch);
        }
        if self.in_call.get()
            || sema_core::in_runtime_quantum()
            || interp.ctx.runtime_quantum_active()
        {
            return Err(McpError::Reentrant);
        }
        self.log(LogEvent::Started);
        let mut frame = Vec::new();
        loop {
            frame.clear();
            let size = reader
                .by_ref()
                .take((self.options.max_frame_bytes as u64).saturating_add(1))
                .read_until(b'\n', &mut frame)?;
            if size == 0 {
                break;
            }
            if frame.len() > self.options.max_frame_bytes && frame.last() != Some(&b'\n') {
                loop {
                    let bytes = reader.fill_buf()?;
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
            if frame.len() <= self.options.max_frame_bytes
                && frame.iter().all(u8::is_ascii_whitespace)
            {
                continue;
            }
            self.handle_message(interp, &frame, &mut writer)?;
        }
        self.log(LogEvent::Stopped);
        Ok(())
    }

    pub fn serve_stdio(&self, interp: &Interpreter) -> Result<(), McpError> {
        if self.options.output != ToolOutput::Capture {
            return Err(McpError::InvalidOptions(
                "stdio serving requires captured output".into(),
            ));
        }
        self.serve_on(interp, std::io::stdin().lock(), std::io::stdout().lock())
    }
}

fn parse_request(frame: &[u8]) -> Result<JsonRpcRequest, Json> {
    let invalid =
        || json!({"jsonrpc":"2.0","id":null,"error":{"code":-32600,"message":"Invalid Request"}});
    let value:Json=serde_json::from_slice(frame).map_err(|error|json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":format!("Parse error: {error}")}}))?;
    if value.get("jsonrpc").and_then(Json::as_str) != Some("2.0")
        || value
            .get("id")
            .is_some_and(|id| !id.is_null() && !id.is_number() && !id.is_string())
    {
        return Err(invalid());
    }
    serde_json::from_value(value).map_err(|_| invalid())
}

/// Stream collection results without allocating a second copy of the entire value.
struct JsonText<'a>(&'a Value, usize);
impl serde::Serialize for JsonText<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::{SerializeMap, SerializeSeq};
        if self.1 >= 128 {
            return serializer.serialize_str("[maximum depth]");
        }
        match self.0.view_ref() {
            ValueViewRef::String(text) => serializer.serialize_str(text),
            ValueViewRef::List(values) | ValueViewRef::Vector(values) => {
                let mut seq = serializer.serialize_seq(Some(values.len()))?;
                for value in values {
                    seq.serialize_element(&JsonText(value, self.1 + 1))?;
                }
                seq.end()
            }
            ValueViewRef::Map(values) => {
                let mut map = serializer.serialize_map(Some(values.len()))?;
                for (key, value) in values {
                    map.serialize_entry(
                        &sema_core::key_to_string(key),
                        &JsonText(value, self.1 + 1),
                    )?;
                }
                map.end()
            }
            ValueViewRef::HashMap(values) => {
                let mut map = serializer.serialize_map(Some(values.len()))?;
                for (key, value) in values {
                    map.serialize_entry(
                        &sema_core::key_to_string(key),
                        &JsonText(value, self.1 + 1),
                    )?;
                }
                map.end()
            }
            _ => sema_core::value_to_json_lossy(self.0).serialize(serializer),
        }
    }
}

struct BoundedText {
    text: String,
    limit: usize,
    truncated: bool,
}
impl BoundedText {
    fn new(text: String, limit: usize) -> Self {
        let mut out = Self {
            text: String::new(),
            limit,
            truncated: false,
        };
        let _ = std::fmt::Write::write_str(&mut out, &text);
        out
    }
    fn finish(mut self) -> String {
        if self.truncated {
            self.text.push_str("\n[output truncated]\n");
        }
        self.text
    }
}
impl std::fmt::Write for BoundedText {
    fn write_str(&mut self, text: &str) -> std::fmt::Result {
        if self.truncated {
            return Err(std::fmt::Error);
        }
        let mut end = text.len().min(self.limit.saturating_sub(self.text.len()));
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        self.text.push_str(&text[..end]);
        self.truncated = end < text.len();
        if self.truncated {
            Err(std::fmt::Error)
        } else {
            Ok(())
        }
    }
}

impl std::io::Write for BoundedText {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let text = std::str::from_utf8(bytes).map_err(std::io::Error::other)?;
        std::fmt::Write::write_str(self, text).map_err(std::io::Error::other)?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
