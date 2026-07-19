use std::io::IsTerminal;

#[cfg(unix)]
use std::cell::{Cell, RefCell};
#[cfg(unix)]
use std::collections::VecDeque;
#[cfg(unix)]
use std::rc::Rc;
#[cfg(unix)]
use std::sync::atomic::{AtomicUsize, Ordering};
#[cfg(unix)]
use std::sync::{Mutex, OnceLock};

use sema_core::cycle::GcEdge;
use sema_core::runtime::{
    CancelDisposition, CancelHook, CancelHookError, CompletionDecoder, CompletionKind,
    DecodedCompletion, ExternalFailure, InterruptibleResource, NativeCallContext,
    NativeContinuation, NativeOutcome, NativeResult, NativeSuspend, PreparedExternalOperation,
    ResumeInput, SendPayload, Trace, WaitKind,
};
use sema_core::{check_arity, in_runtime_quantum, Caps, SemaError, Value};

use crate::register_fn;

/// Completion tag for the blocking `sleep` external operation. A tag only needs
/// to be consistent between the issued identity and the prepared op; collisions
/// with other external ops are harmless (it is not a uniqueness key).
const SLEEP_COMPLETION_KIND: u64 = 1;
/// Clamp for a blocking sleep routed to a worker thread (mirrors `async/sleep`):
/// keeps an out-of-range duration from wedging a worker for years.
const MAX_SLEEP_MS: u64 = 86_400_000; // 1 day

/// Cancel hook for the blocking `sleep` worker. A `thread::sleep` cannot be
/// interrupted mid-flight, so cancellation reports the resource reaped: the
/// runtime drops it immediately and the worker's eventual (now unowned)
/// completion is discarded as a late completion.
struct SleepCancelHook;

impl Trace for SleepCancelHook {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}

impl CancelHook for SleepCancelHook {
    fn cancel(&mut self) -> Result<CancelDisposition, CancelHookError> {
        Ok(CancelDisposition::Reaped)
    }
    fn reap(&mut self) -> Result<CancelDisposition, CancelHookError> {
        Ok(CancelDisposition::Reaped)
    }
}

/// Decodes the worker's completion for a blocking `sleep`: success yields nil; a
/// worker failure (e.g. panic) surfaces as an evaluation error.
struct SleepDecoder;

impl Trace for SleepDecoder {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}

impl CompletionDecoder for SleepDecoder {
    fn decode(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        result: Result<SendPayload, ExternalFailure>,
    ) -> DecodedCompletion {
        result
            .map(|_| Value::nil())
            .map_err(|failure| SemaError::eval(format!("sleep failed: {}", failure.message())))
    }
}

/// Resumes the parked `sleep` frame once the worker completes: the decoded nil is
/// injected onto its stack top; a failure or cancellation is raised at the call
/// site (catchable by an enclosing try/catch).
struct SleepContinuation;

impl Trace for SleepContinuation {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}

impl NativeContinuation for SleepContinuation {
    fn resume(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        match input {
            ResumeInput::Returned(value) => Ok(NativeOutcome::Return(value)),
            ResumeInput::Failed(error) => Err(error),
            ResumeInput::Cancelled(reason) => {
                Err(SemaError::eval(format!("sleep was cancelled ({reason:?})")))
            }
            ResumeInput::Runtime(_) => Err(SemaError::eval(
                "sleep continuation received an unexpected runtime response",
            )),
        }
    }
}

/// Build the external-wait `NativeOutcome::Suspend` for a blocking `sleep` under
/// the unified runtime and RETURN it on the runtime native ABI. The runtime
/// submits the job to the thread-pool executor (so it runs off the VM thread and
/// overlaps sibling work) and, when the worker completes, resumes this frame with
/// nil.
fn sleep_via_executor(ms: u64) -> NativeResult {
    let ms = ms.min(MAX_SLEEP_MS);
    let kind = CompletionKind::try_from_raw(SLEEP_COMPLETION_KIND)
        .expect("sleep completion kind is nonzero");
    let prepared = PreparedExternalOperation::interruptible_blocking(
        kind,
        Box::new(SleepDecoder),
        InterruptibleResource::new("sleep", Box::new(SleepCancelHook)),
        move || {
            std::thread::sleep(std::time::Duration::from_millis(ms));
            Ok(Box::new(()) as SendPayload)
        },
    );
    let suspend = NativeSuspend {
        wait: WaitKind::External(Box::new(prepared)),
        continuation: Box::new(SleepContinuation),
    };
    Ok(NativeOutcome::Suspend(suspend))
}

/// Monotonic clock captured the first time it is needed — forced during
/// `register()` so it reflects interpreter/process startup, not the first call
/// to `sys/elapsed`. `sys/elapsed` reports nanoseconds since this instant.
fn process_start() -> std::time::Instant {
    use std::sync::OnceLock;
    static PROCESS_START: OnceLock<std::time::Instant> = OnceLock::new();
    *PROCESS_START.get_or_init(std::time::Instant::now)
}

/// Whether a PATH candidate is actually runnable. On Unix this requires an
/// execute permission bit, matching POSIX `which`; reporting a non-executable
/// file as a found command is wrong. On other platforms runnability is governed
/// by extension rules, so existence (already checked by the caller) suffices.
fn is_executable(path: &std::path::Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        true
    }
}

// ─── Deferred Unix signal delivery ─────────────────────────────────────────

/// Process-wide event generations. A generation is a broadcast token: each
/// interpreter-owned registry remembers the last generation it observed, so
/// one interpreter checking signals cannot consume another's event. Multiple
/// arrivals before one interpreter checks remain coalesced into one dispatch.
#[cfg(unix)]
static SIGWINCH_EPOCH: AtomicUsize = AtomicUsize::new(0);
#[cfg(unix)]
static SIGINT_EPOCH: AtomicUsize = AtomicUsize::new(0);
#[cfg(unix)]
static SIGTERM_EPOCH: AtomicUsize = AtomicUsize::new(0);

#[cfg(unix)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SignalKind {
    Winch,
    Int,
    Term,
}

#[cfg(unix)]
impl SignalKind {
    const ALL: [Self; 3] = [Self::Winch, Self::Int, Self::Term];

    fn index(self) -> usize {
        match self {
            Self::Winch => 0,
            Self::Int => 1,
            Self::Term => 2,
        }
    }

    fn epoch(self) -> &'static AtomicUsize {
        match self {
            Self::Winch => &SIGWINCH_EPOCH,
            Self::Int => &SIGINT_EPOCH,
            Self::Term => &SIGTERM_EPOCH,
        }
    }

    fn from_value(value: &Value) -> Result<Self, SemaError> {
        let keyword = value
            .as_keyword()
            .ok_or_else(|| SemaError::type_error("keyword", value.type_name()))?;
        match keyword.as_str() {
            "winch" => Ok(Self::Winch),
            "int" => Ok(Self::Int),
            "term" => Ok(Self::Term),
            other => Err(SemaError::eval(format!(
                "sys/on-signal: unknown signal :{other}; use :winch, :int, or :term"
            ))),
        }
    }

    fn signal_number(self) -> libc::c_int {
        match self {
            Self::Winch => libc::SIGWINCH,
            Self::Int => libc::SIGINT,
            Self::Term => libc::SIGTERM,
        }
    }

    fn handler(self) -> libc::sighandler_t {
        // Cast through a pointer to avoid the fn_to_numeric_cast lint. The
        // installed handler touches only one lock-free atomic generation.
        match self {
            Self::Winch => handle_sigwinch as *const () as usize,
            Self::Int => handle_sigint as *const () as usize,
            Self::Term => handle_sigterm as *const () as usize,
        }
    }

    fn install_handler(self) -> Result<libc::sigaction, SemaError> {
        // SAFETY: zero-initialization is valid for `sigaction`; the mask is
        // initialized before installation, and `handler` is an extern-C
        // function with the exact platform signal-handler signature and
        // process lifetime.
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = self.handler();
            action.sa_flags = libc::SA_RESTART;
            if libc::sigemptyset(&mut action.sa_mask) != 0 {
                return Err(signal_install_error());
            }
            let mut previous: libc::sigaction = std::mem::zeroed();
            if libc::sigaction(self.signal_number(), &action, &mut previous) != 0 {
                return Err(signal_install_error());
            }
            Ok(previous)
        }
    }

    fn acquire(self) -> Result<(), SemaError> {
        let mut ownership = process_signal_ownership().lock().map_err(|_| {
            SemaError::eval("sys/on-signal: process signal ownership lock is poisoned")
        })?;
        let slot = &mut ownership[self.index()];
        let next = slot.subscribers.checked_add(1).ok_or_else(|| {
            SemaError::eval("sys/on-signal: process signal subscriber count overflow")
        })?;
        if slot.subscribers == 0 {
            slot.previous = Some(self.install_handler()?);
        }
        slot.subscribers = next;
        Ok(())
    }

    fn release(self) {
        let mut ownership = process_signal_ownership()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let slot = &mut ownership[self.index()];
        assert!(
            slot.subscribers > 0,
            "signal registry released an unowned process handler"
        );
        if slot.subscribers > 1 {
            slot.subscribers -= 1;
            return;
        }

        let previous = slot
            .previous
            .as_ref()
            .expect("owned process signal handler retains its prior sigaction");
        // SAFETY: `previous` was returned by `sigaction` for this exact signal
        // when the first subscriber installed the process handler. The global
        // ownership mutex keeps a new first subscriber from racing this restore.
        let restored =
            unsafe { libc::sigaction(self.signal_number(), previous, std::ptr::null_mut()) };
        assert_eq!(
            restored,
            0,
            "failed to restore prior signal disposition: {}",
            std::io::Error::last_os_error()
        );
        slot.subscribers = 0;
        slot.previous = None;
    }
}

#[cfg(unix)]
fn signal_install_error() -> SemaError {
    SemaError::eval(format!(
        "sys/on-signal: failed to install signal handler: {}",
        std::io::Error::last_os_error()
    ))
}

#[cfg(unix)]
#[derive(Default)]
struct ProcessSignalOwnership {
    subscribers: usize,
    previous: Option<libc::sigaction>,
}

#[cfg(unix)]
fn process_signal_ownership() -> &'static Mutex<[ProcessSignalOwnership; 3]> {
    static OWNERSHIP: OnceLock<Mutex<[ProcessSignalOwnership; 3]>> = OnceLock::new();
    OWNERSHIP.get_or_init(|| Mutex::new(std::array::from_fn(|_| ProcessSignalOwnership::default())))
}

#[cfg(unix)]
#[derive(Default)]
struct SignalSlot {
    callbacks: Vec<Value>,
    seen_epoch: usize,
}

/// One interpreter's signal subscriptions. The two signal builtins in that
/// interpreter's environment share this as a traced native payload; no process
/// or thread-local cell owns callback `Value`s.
#[cfg(unix)]
#[derive(Default)]
struct SignalRegistry {
    slots: RefCell<[SignalSlot; 3]>,
    /// Process handler ownership is independent of callback edges: cycle
    /// collection may sever `SignalSlot::callbacks` before this registry drops,
    /// but teardown must still release every signal it installed.
    installed: Cell<[bool; 3]>,
}

#[cfg(unix)]
impl SignalRegistry {
    fn register(&self, kind: SignalKind, callback: Value) -> Result<(), SemaError> {
        let index = kind.index();
        let mut installed = self.installed.get();
        if !installed[index] {
            kind.acquire()?;
            installed[index] = true;
            self.installed.set(installed);
            // Registration linearizes at this load. Signals observed before it
            // are historical; a signal racing after it advances the generation
            // and is delivered on the next check.
            self.slots.borrow_mut()[index].seen_epoch = kind.epoch().load(Ordering::Relaxed);
        }
        self.slots.borrow_mut()[index].callbacks.push(callback);
        Ok(())
    }

    /// Consume this interpreter's current generation snapshot and clone its
    /// callback batch in stable signal/registration order. The borrow ends
    /// before any callback runs, so callbacks may register more callbacks.
    fn take_pending_callbacks(&self) -> Result<VecDeque<Value>, SemaError> {
        let observed = SignalKind::ALL.map(|kind| kind.epoch().load(Ordering::Relaxed));
        let mut slots = self.slots.try_borrow_mut().map_err(|_| {
            SemaError::eval("sys/check-signals: signal registry is already borrowed")
        })?;
        let mut callbacks = VecDeque::new();
        for kind in SignalKind::ALL {
            let index = kind.index();
            let slot = &mut slots[index];
            if !slot.callbacks.is_empty() && slot.seen_epoch != observed[index] {
                slot.seen_epoch = observed[index];
                callbacks.extend(slot.callbacks.iter().cloned());
            }
        }
        Ok(callbacks)
    }
}

#[cfg(unix)]
impl Drop for SignalRegistry {
    fn drop(&mut self) {
        let installed = self.installed.get();
        for kind in SignalKind::ALL {
            if installed[kind.index()] {
                kind.release();
            }
        }
    }
}

#[cfg(unix)]
struct SignalDispatchContinuation {
    remaining: VecDeque<Value>,
}

#[cfg(unix)]
impl Trace for SignalDispatchContinuation {
    fn trace(&self, sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        for callback in &self.remaining {
            sink(GcEdge::Value(callback));
        }
        true
    }
}

#[cfg(unix)]
impl NativeContinuation for SignalDispatchContinuation {
    fn resume(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        match input {
            ResumeInput::Returned(_) => dispatch_signal_callbacks(self.remaining),
            ResumeInput::Failed(error) => Err(error),
            ResumeInput::Cancelled(reason) => Err(SemaError::eval(format!(
                "sys/check-signals callback was cancelled ({reason:?})"
            ))),
            ResumeInput::Runtime(_) => Err(SemaError::eval(
                "sys/check-signals callback received an unexpected runtime response",
            )),
        }
    }
}

#[cfg(unix)]
fn dispatch_signal_callbacks(mut callbacks: VecDeque<Value>) -> NativeResult {
    let Some(callable) = callbacks.pop_front() else {
        return Ok(NativeOutcome::Return(Value::nil()));
    };
    Ok(NativeOutcome::Call(sema_core::runtime::NativeCall {
        callable,
        args: Vec::new(),
        continuation: Box::new(SignalDispatchContinuation {
            remaining: callbacks,
        }),
    }))
}

#[cfg(unix)]
fn check_signals(
    registry: &SignalRegistry,
    _context: &mut NativeCallContext<'_>,
    args: &[Value],
) -> NativeResult {
    check_arity!(args, "sys/check-signals", 0);
    dispatch_signal_callbacks(registry.take_pending_callbacks()?)
}

/// Reports the one strong payload edge owned by the NativeFn currently being
/// traced. Both signal builtins point at the same allocation, so two NativeFns
/// report two edges and the opaque node's `strong_count` is two.
#[cfg(unix)]
fn signal_registry_payload_tracer(
    payload: &Rc<dyn std::any::Any>,
    sink: &mut dyn FnMut(GcEdge<'_>),
) -> bool {
    sink(GcEdge::Opaque {
        ptr: sema_core::NodePtr::of_rc(payload),
        strong_count: Rc::strong_count(payload),
        trace: trace_signal_registry,
        sever: sever_signal_registry,
    });
    true
}

#[cfg(unix)]
fn trace_signal_registry(ptr: sema_core::NodePtr, sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
    // SAFETY: payload tracing supplied the data pointer of a live
    // `Rc<SignalRegistry>`; the collector retains traced allocations through
    // the complete pass.
    let registry = unsafe { &*(ptr.raw() as *const SignalRegistry) };
    let Ok(slots) = registry.slots.try_borrow() else {
        return false;
    };
    for callback in slots.iter().flat_map(|slot| &slot.callbacks) {
        sink(GcEdge::Value(callback));
    }
    true
}

#[cfg(unix)]
fn sever_signal_registry(ptr: sema_core::NodePtr) -> Option<Value> {
    // SAFETY: see `trace_signal_registry`; severing runs while the same opaque
    // allocation remains retained by the collector.
    let registry = unsafe { &*(ptr.raw() as *const SignalRegistry) };
    let mut slots = registry.slots.try_borrow_mut().ok()?;
    let callbacks = slots
        .iter_mut()
        .flat_map(|slot| std::mem::take(&mut slot.callbacks))
        .collect();
    Some(Value::list(callbacks))
}

// ─── Signal handlers: only allowed to use async-signal-safe operations ───────
#[cfg(unix)]
extern "C" fn handle_sigwinch(_: libc::c_int) {
    SIGWINCH_EPOCH.fetch_add(1, Ordering::Relaxed);
}

#[cfg(unix)]
extern "C" fn handle_sigint(_: libc::c_int) {
    SIGINT_EPOCH.fetch_add(1, Ordering::Relaxed);
}

#[cfg(unix)]
extern "C" fn handle_sigterm(_: libc::c_int) {
    SIGTERM_EPOCH.fetch_add(1, Ordering::Relaxed);
}

/// Deterministic delivery hook for runtime integration tests. It exercises the
/// same epoch transition as the async OS handler without sending a process
/// signal that could interfere with a parallel test.
#[cfg(unix)]
#[doc(hidden)]
pub fn mark_sigwinch_pending_for_test() {
    SIGWINCH_EPOCH.fetch_add(1, Ordering::Relaxed);
}

/// Resolve the (program, argv) for a `shell` invocation. A lone command string
/// runs through the system shell (`sh -c "<cmd>"` / `cmd /C "<cmd>"`); explicit
/// args run the program directly. Owned `String`s so the spawned async path can
/// move them across the thread boundary. Shared by both paths so they launch
/// byte-identical commands.
fn shell_program_args(cmd: &str, cmd_args: &[&str]) -> (String, Vec<String>) {
    if cmd_args.is_empty() {
        // Single string: run through the system shell for command parsing
        let shell = if cfg!(windows) { "cmd" } else { "sh" };
        let flag = if cfg!(windows) { "/C" } else { "-c" };
        (shell.to_string(), vec![flag.to_string(), cmd.to_string()])
    } else {
        // Explicit args: run the command directly
        (
            cmd.to_string(),
            cmd_args.iter().map(|s| s.to_string()).collect(),
        )
    }
}

/// Extract a `{:cwd "path" :env {"KEY" "val" ...}}` options map into owned,
/// `Send` data: an optional working directory and a list of env overrides.
/// Shared by `shell` and `proc/spawn` so both interpret the options map
/// identically; owning `String`s (not borrows) lets the async `shell` path move
/// them across the I/O-pool thread boundary.
pub(crate) fn command_opts(
    opts: &std::collections::BTreeMap<Value, Value>,
) -> (Option<String>, Vec<(String, String)>) {
    let cwd = opts
        .get(&Value::keyword("cwd"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let mut env = Vec::new();
    if let Some(em) = opts
        .get(&Value::keyword("env"))
        .and_then(|v| v.as_map_ref())
    {
        for (k, val) in em.iter() {
            if let (Some(k), Some(val)) = (k.as_str(), val.as_str()) {
                env.push((k.to_string(), val.to_string()));
            }
        }
    }
    (cwd, env)
}

/// POSIX single-quote a string so it survives `sh -c` as one literal word. Wrap
/// in single quotes; each embedded `'` becomes `'\''` (close-quote, escaped
/// literal quote, reopen). The empty string becomes `''`. Robust for arbitrary
/// input — no shell metacharacter is special inside single quotes.
fn posix_shell_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

/// Decode a finished child's status/stdout/stderr into the Sema `shell` result
/// map. Identical shape for the sync and async paths: `:stdout`/`:stderr` are
/// lossy-UTF-8 strings and `:exit-code` is the exit code (or `-1` when the
/// process was terminated by a signal / has no code).
fn shell_output_value(status_code: Option<i32>, stdout: &[u8], stderr: &[u8]) -> Value {
    let stdout = String::from_utf8_lossy(stdout).to_string();
    let stderr = String::from_utf8_lossy(stderr).to_string();

    let mut result = std::collections::BTreeMap::new();
    result.insert(Value::keyword("stdout"), Value::string(&stdout));
    result.insert(Value::keyword("stderr"), Value::string(&stderr));
    result.insert(
        Value::keyword("exit-code"),
        Value::int(status_code.unwrap_or(-1) as i64),
    );
    Value::map(result)
}

/// The subprocess facts that cross the thread boundary back from the I/O pool
/// to the VM thread. Only plain `Send` data — never a `Value`/`Rc`.
/// Decoded into the same `Value` shape as the sync path via [`shell_output_value`].
struct RawShellOutput {
    status_code: Option<i32>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

/// Completion tag for an offloaded `shell` subprocess. Consistent between the
/// issued identity and the prepared op (not a uniqueness key).
const SHELL_COMPLETION_KIND: u64 = 0x7368_656c; // "shel"

/// The `Send` future that runs the shell subprocess off the VM thread on the
/// executor's blocking worker (via `io_block_on` inside `runtime_offload`). It
/// publishes the child's OS pid into `pid_slot` (its own process group, so the
/// abort hook can `SIGKILL` the whole group), then clears it once the child is
/// reaped so a late cancel never signals a reused pid. `kill_on_drop` kills the
/// direct child if the future is dropped on cancel.
#[cfg(not(target_arch = "wasm32"))]
async fn shell_run_future(
    program: String,
    child_args: Vec<String>,
    cwd: Option<String>,
    env_vars: Vec<(String, String)>,
    pid_slot: std::sync::Arc<std::sync::atomic::AtomicU32>,
) -> Result<RawShellOutput, String> {
    use std::sync::atomic::Ordering;
    let mut cmd = tokio::process::Command::new(&program);
    cmd.args(&child_args)
        .kill_on_drop(true)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if let Some(dir) = &cwd {
        cmd.current_dir(dir);
    }
    for (k, v) in &env_vars {
        cmd.env(k, v);
    }
    // Own process group so a compound/pipelined command (`sh -c "a; b"`) can be
    // torn down as a GROUP on abort, not just the `sh` leader.
    #[cfg(unix)]
    cmd.process_group(0);
    let child = cmd.spawn().map_err(|e| format!("shell: {e}"))?;
    if let Some(id) = child.id() {
        pid_slot.store(id, Ordering::SeqCst);
    }
    let output = child.wait_with_output().await;
    // Child reaped (or the wait errored): clear the pid so a cancel that races
    // completion never `SIGKILL`s a reaped (possibly reused) pid.
    pid_slot.store(0, Ordering::SeqCst);
    let output = output.map_err(|e| format!("shell: {e}"))?;
    Ok(RawShellOutput {
        status_code: output.status.code(),
        stdout: output.stdout,
        stderr: output.stderr,
    })
}

/// Cancel hook for the runtime `shell` op. On `async/cancel`/`async/timeout` it
/// (a) issues a SYNCHRONOUS `SIGKILL` to the child's PROCESS GROUP — reliable
/// even when the program exits immediately after the timeout (a one-shot
/// `sema -e`), where the worker may be gone before it can drop the future, and
/// killing the GROUP reaps a compound command's grandchildren — and (b) fires the
/// select signal so the job drops the future (`kill_on_drop` the direct child).
/// Mirrors `shell_async`'s abort hook exactly.
#[cfg(not(target_arch = "wasm32"))]
struct ShellCancelHook {
    signal: Option<crate::runtime_offload::CancelSignal>,
    pid_slot: std::sync::Arc<std::sync::atomic::AtomicU32>,
}

#[cfg(not(target_arch = "wasm32"))]
impl Trace for ShellCancelHook {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl CancelHook for ShellCancelHook {
    fn cancel(&mut self) -> Result<CancelDisposition, CancelHookError> {
        #[cfg(unix)]
        {
            let pid = self.pid_slot.load(std::sync::atomic::Ordering::SeqCst);
            if pid != 0 {
                // SAFETY: killpg of the child's own process group (process_group(0)
                // set pgid == pid). The negative pid targets the GROUP. The pid is
                // reset to 0 by the worker once the child is reaped, so a
                // reaped/reused pid is never targeted.
                unsafe {
                    libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
                }
            }
        }
        if let Some(signal) = self.signal.take() {
            let _ = signal.send(());
        }
        Ok(CancelDisposition::Reaped)
    }
    fn reap(&mut self) -> Result<CancelDisposition, CancelHookError> {
        Ok(CancelDisposition::Reaped)
    }
}

/// The unified-runtime `shell` path: SUSPEND on an interruptible External wait
/// whose job runs the subprocess off the VM thread; on resume the decoder builds
/// the identical `shell_output_value`. Cancellation class: interruptible with a
/// synchronous process-group `SIGKILL` (see [`ShellCancelHook`]).
#[cfg(not(target_arch = "wasm32"))]
fn shell_runtime(
    program: String,
    child_args: Vec<String>,
    cwd: Option<String>,
    env_vars: Vec<(String, String)>,
) -> NativeResult {
    let (cancel_tx, cancel_rx) = crate::runtime_offload::cancel_channel();
    let pid_slot = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let resource = InterruptibleResource::new(
        "shell",
        Box::new(ShellCancelHook {
            signal: Some(cancel_tx),
            pid_slot: pid_slot.clone(),
        }),
    );
    let kind = CompletionKind::try_from_raw(SHELL_COMPLETION_KIND)
        .expect("shell completion kind is nonzero");
    crate::runtime_offload::suspend_external_interruptible_try(
        "shell",
        kind,
        resource,
        cancel_rx,
        move |raw: RawShellOutput| -> Result<Value, SemaError> {
            Ok(shell_output_value(
                raw.status_code,
                &raw.stdout,
                &raw.stderr,
            ))
        },
        move || shell_run_future(program, child_args, cwd, env_vars, pid_slot),
    )
}

pub fn register(env: &sema_core::Env, sandbox: &sema_core::Sandbox) {
    // Anchor the sys/elapsed clock at startup rather than at its first call.
    process_start();

    crate::register_fn_gated(env, sandbox, Caps::ENV_READ, "env", |args| {
        check_arity!(args, "env", 1);
        let name = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        match std::env::var(name) {
            Ok(val) => Ok(Value::string(&val)),
            Err(_) => Ok(Value::nil()),
        }
    });

    // shell requires both SHELL (to launch a shell) AND PROCESS (it spawns a child
    // process). Gate on SHELL via the helper, and check PROCESS inline so either
    // denial blocks the call.
    let shell_sandbox = sandbox.clone();
    crate::register_runtime_fn_path_gated(env, sandbox, Caps::SHELL, "shell", &[], move |args| {
        shell_sandbox.check(Caps::PROCESS, "shell")?;
        check_arity!(args, "shell", 1..);
        let cmd = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;

        // A trailing map is an options map (`{:cwd "path" :env {...}}`), never an
        // argv element — so the positional string-argv form is unchanged. Only
        // the LAST arg qualifies, and only when it is actually a map; a string in
        // that slot still means an argv element.
        let (argv, opts) = match args[1..].split_last() {
            Some((last, rest)) if last.as_map_ref().is_some() => (rest, last.as_map_ref()),
            _ => (&args[1..], None),
        };
        let cmd_args: Vec<&str> = argv
            .iter()
            .map(|a| {
                a.as_str()
                    .ok_or_else(|| SemaError::type_error("string", a.type_name()))
            })
            .collect::<Result<_, _>>()?;
        let (cwd, env_vars) = opts.map(command_opts).unwrap_or_default();

        // Resolve the program + argv exactly once, shared by both paths so they
        // launch byte-identical commands.
        let (program, child_args) = shell_program_args(cmd, &cmd_args);

        // Inside a unified-runtime VM quantum: SUSPEND on a structural External
        // wait so the subprocess runs off the VM thread while sibling tasks run.
        if in_runtime_quantum() {
            return shell_runtime(program, child_args, cwd, env_vars);
        }

        // Top-level (not in any task): run the subprocess synchronously.
        let mut command = std::process::Command::new(&program);
        command.args(&child_args);
        if let Some(dir) = &cwd {
            command.current_dir(dir);
        }
        for (k, v) in &env_vars {
            command.env(k, v);
        }
        let output = command
            .output()
            .map_err(|e| SemaError::Io(format!("shell: {e}")))?;

        Ok(NativeOutcome::Return(shell_output_value(
            output.status.code(),
            &output.stdout,
            &output.stderr,
        )))
    });

    // shell/quote — POSIX-quote a string for safe interpolation into a POSIX `sh -c`
    // command line (Unix). Note: the single-string form of `shell` uses `cmd /C` on Windows.
    // Pure (string→string), so ungated like other string helpers.
    register_fn(env, "shell/quote", |args| {
        check_arity!(args, "shell/quote", 1);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        Ok(Value::string(&posix_shell_quote(s)))
    });

    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "exit", |args| {
        let code = if args.is_empty() {
            0
        } else {
            // Reject non-numeric status rather than silently exiting 0 — a script
            // that means to fail must not succeed. Floats truncate toward zero.
            match args[0]
                .as_int()
                .or_else(|| args[0].as_float().map(|f| f as i64))
            {
                Some(n) => n as i32,
                None => return Err(SemaError::type_error("integer", args[0].type_name())),
            }
        };
        std::process::exit(code);
    });

    fn time_ms_impl(args: &[Value]) -> Result<Value, SemaError> {
        check_arity!(args, "time-ms", 0);
        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        Ok(Value::int(ms))
    }
    register_fn(env, "time-ms", time_ms_impl);
    // Canonical slash-namespaced alias (Decision #24)
    register_fn(env, "time/now-ms", time_ms_impl);

    // `sleep` is dual-ABI. Under the unified runtime it is a genuinely-blocking
    // operation: the runtime callback submits it to the thread-pool executor and
    // SUSPENDS (returning `NativeOutcome::Suspend`) so the worker runs it off the
    // VM thread — two `async/spawn`ed sleeps then overlap instead of serializing
    // on the VM thread (unlike `async/sleep`, which is a virtual timer). Outside
    // a runtime quantum the plain value callback sleeps synchronously.
    env.set(
        sema_core::intern("sleep"),
        Value::native_fn(sema_core::NativeFn::simple_with_runtime(
            "sleep",
            |args| {
                check_arity!(args, "sleep", 1);
                let ms = args[0]
                    .as_int()
                    .ok_or_else(|| SemaError::type_error("int", args[0].type_name()))?;
                // The plain value ABI is synchronous (REPL, scripts, and nested
                // host callbacks); the runtime ABI below performs the offload.
                std::thread::sleep(std::time::Duration::from_millis(ms as u64));
                Ok(Value::nil())
            },
            |_ctx: &mut NativeCallContext<'_>, args: &[Value]| {
                check_arity!(args, "sleep", 1);
                let ms = args[0]
                    .as_int()
                    .ok_or_else(|| SemaError::type_error("int", args[0].type_name()))?;
                sleep_via_executor(ms.max(0) as u64)
            },
        )),
    );

    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "sys/args", |args| {
        check_arity!(args, "sys/args", 0);
        let args_list: Vec<Value> = std::env::args().map(|a| Value::string(&a)).collect();
        Ok(Value::list(args_list))
    });

    crate::register_fn_gated(env, sandbox, Caps::ENV_READ, "sys/cwd", |args| {
        check_arity!(args, "sys/cwd", 0);
        let cwd = std::env::current_dir().map_err(|e| SemaError::Io(format!("sys/cwd: {e}")))?;
        Ok(Value::string(&cwd.to_string_lossy()))
    });

    register_fn(env, "sys/platform", |args| {
        check_arity!(args, "sys/platform", 0);
        let platform = if cfg!(target_os = "macos") {
            "macos"
        } else if cfg!(target_os = "linux") {
            "linux"
        } else if cfg!(target_os = "windows") {
            "windows"
        } else {
            "unknown"
        };
        Ok(Value::string(platform))
    });

    crate::register_fn_gated(env, sandbox, Caps::ENV_WRITE, "sys/set-env", |args| {
        check_arity!(args, "sys/set-env", 2);
        let name = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let value = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        // set_var mutates process-global state and is UB under concurrent getenv
        // (tokio workers, C libs) on glibc. Serialize all sets behind a lock
        // (STD-12). NOTE: still unsafe if other code reads env concurrently —
        // sys/set-env should not be called while a server is handling requests.
        static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::set_var(name, value);
        }
        Ok(Value::nil())
    });

    crate::register_fn_gated(env, sandbox, Caps::ENV_READ, "sys/env-all", |args| {
        check_arity!(args, "sys/env-all", 0);
        let mut map = std::collections::BTreeMap::new();
        for (key, val) in std::env::vars() {
            map.insert(Value::keyword(&key), Value::string(&val));
        }
        Ok(Value::map(map))
    });

    crate::register_fn_gated(env, sandbox, Caps::ENV_READ, "sys/home-dir", |args| {
        check_arity!(args, "sys/home-dir", 0);
        match std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")) {
            Ok(home) => Ok(Value::string(&home)),
            Err(_) => Ok(Value::nil()),
        }
    });

    register_fn(env, "sys/sema-home", |args| {
        check_arity!(args, "sys/sema-home", 0);
        Ok(Value::string(&sema_core::sema_home().to_string_lossy()))
    });

    // sys/config-dir — platform-appropriate user config base directory, so apps
    // (e.g. Sema Coder) can store config without branching on OS in Sema:
    //   macOS:   ~/Library/Application Support
    //   Windows: %APPDATA%
    //   else:    $XDG_CONFIG_HOME or ~/.config
    register_fn(env, "sys/config-dir", |args| {
        check_arity!(args, "sys/config-dir", 0);
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_default();
        let dir = if cfg!(target_os = "macos") {
            std::path::PathBuf::from(&home)
                .join("Library")
                .join("Application Support")
        } else if cfg!(windows) {
            std::env::var("APPDATA")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| {
                    std::path::PathBuf::from(&home)
                        .join("AppData")
                        .join("Roaming")
                })
        } else {
            std::env::var("XDG_CONFIG_HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| std::path::PathBuf::from(&home).join(".config"))
        };
        Ok(Value::string(&dir.to_string_lossy()))
    });

    crate::register_fn_gated(env, sandbox, Caps::ENV_READ, "sys/temp-dir", |args| {
        check_arity!(args, "sys/temp-dir", 0);
        Ok(Value::string(&std::env::temp_dir().to_string_lossy()))
    });

    register_fn(env, "sys/hostname", |args| {
        check_arity!(args, "sys/hostname", 0);
        match hostname::get() {
            Ok(name) => Ok(Value::string(&name.to_string_lossy())),
            Err(_) => Ok(Value::nil()),
        }
    });

    crate::register_fn_gated(env, sandbox, Caps::ENV_READ, "sys/user", |args| {
        check_arity!(args, "sys/user", 0);
        match std::env::var("USER").or_else(|_| std::env::var("USERNAME")) {
            Ok(user) => Ok(Value::string(&user)),
            Err(_) => Ok(Value::nil()),
        }
    });

    register_fn(env, "sys/interactive?", |args| {
        check_arity!(args, "sys/interactive?", 0);
        Ok(Value::bool(std::io::stdin().is_terminal()))
    });

    register_fn(env, "sys/tty", |args| {
        check_arity!(args, "sys/tty", 0);
        if !std::io::stdin().is_terminal() {
            return Ok(Value::nil());
        }
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            let fd = std::io::stdin().as_raw_fd();
            unsafe {
                let name = libc::ttyname(fd);
                if name.is_null() {
                    Ok(Value::nil())
                } else {
                    let s = std::ffi::CStr::from_ptr(name).to_string_lossy().to_string();
                    Ok(Value::string(&s))
                }
            }
        }
        #[cfg(not(unix))]
        {
            Ok(Value::nil())
        }
    });

    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "sys/pid", |args| {
        check_arity!(args, "sys/pid", 0);
        Ok(Value::int(std::process::id() as i64))
    });

    register_fn(env, "sys/arch", |args| {
        check_arity!(args, "sys/arch", 0);
        Ok(Value::string(std::env::consts::ARCH))
    });

    register_fn(env, "sys/os", |args| {
        check_arity!(args, "sys/os", 0);
        Ok(Value::string(std::env::consts::OS))
    });

    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "sys/which", |args| {
        check_arity!(args, "sys/which", 1);
        let name = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let path_var = std::env::var("PATH").unwrap_or_default();
        let sep = if cfg!(windows) { ';' } else { ':' };
        // On Windows a bare name resolves by trying each PATHEXT suffix (unless
        // it already has an extension); on Unix the name is used verbatim.
        let candidates: Vec<String> =
            if cfg!(windows) && std::path::Path::new(name).extension().is_none() {
                let exts =
                    std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string());
                let mut names = vec![name.to_string()];
                names.extend(
                    exts.split(';')
                        .map(str::trim)
                        .filter(|e| !e.is_empty())
                        .map(|e| format!("{name}{e}")),
                );
                names
            } else {
                vec![name.to_string()]
            };
        for dir in path_var.split(sep) {
            for cand in &candidates {
                let candidate = std::path::Path::new(dir).join(cand);
                if candidate.is_file() && is_executable(&candidate) {
                    return Ok(Value::string(&candidate.to_string_lossy()));
                }
            }
        }
        Ok(Value::nil())
    });

    register_fn(env, "sys/interner-stats", |args| {
        check_arity!(args, "sys/interner-stats", 0);
        let (count, bytes) = sema_core::interner_stats();
        let mut result = std::collections::BTreeMap::new();
        result.insert(Value::keyword("count"), Value::int(count as i64));
        result.insert(Value::keyword("bytes"), Value::int(bytes as i64));
        Ok(Value::map(result))
    });

    register_fn(env, "sys/elapsed", |args| {
        check_arity!(args, "sys/elapsed", 0);
        let nanos = process_start().elapsed().as_nanos() as i64;
        Ok(Value::int(nanos))
    });

    // sys/term-size — returns {:rows N :cols M} or nil when not a TTY
    register_fn(env, "sys/term-size", |args| {
        check_arity!(args, "sys/term-size", 0);
        #[cfg(unix)]
        {
            let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
            // Try each standard fd in order until one succeeds; stdout is most reliable
            // for terminal size since stderr is used for status lines on many setups.
            for fd in [libc::STDOUT_FILENO, libc::STDERR_FILENO, libc::STDIN_FILENO] {
                let ret = unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut ws) };
                if ret == 0 && ws.ws_row > 0 && ws.ws_col > 0 {
                    let mut m = std::collections::BTreeMap::new();
                    m.insert(Value::keyword("rows"), Value::int(ws.ws_row as i64));
                    m.insert(Value::keyword("cols"), Value::int(ws.ws_col as i64));
                    return Ok(Value::map(m));
                }
            }
            Ok(Value::nil())
        }
        #[cfg(not(unix))]
        Ok(Value::nil())
    });

    // ─── Signal hooks ────────────────────────────────────────────────────────
    // sys/on-signal — register a Sema callback for a signal.
    // Supported signals: :winch (SIGWINCH), :int (SIGINT), :term (SIGTERM).
    // An interpreter's first callback for a signal installs the OS handler.
    // Call (sys/check-signals) from your event loop to dispatch pending callbacks.
    #[cfg(unix)]
    {
        use sema_core::NativeFn;

        let registry = Rc::new(SignalRegistry::default());
        sema_core::register_payload_tracer(
            std::any::TypeId::of::<SignalRegistry>(),
            signal_registry_payload_tracer,
        );

        let on_signal_registry = Rc::downgrade(&registry);
        let on_signal_payload: Rc<dyn std::any::Any> = registry.clone();

        env.set(
            sema_core::intern("sys/on-signal"),
            Value::native_fn(
                NativeFn::with_payload("sys/on-signal", on_signal_payload, move |_ctx, args| {
                    check_arity!(args, "sys/on-signal", 2);
                    let kind = SignalKind::from_value(&args[0])?;
                    let registry = on_signal_registry.upgrade().ok_or_else(|| {
                        SemaError::eval("internal error: sys/on-signal registry is unavailable")
                    })?;
                    registry.register(kind, args[1].clone())?;
                    Ok(Value::nil())
                })
                .with_escaping_args(&[1]),
            ),
        );

        // sys/check-signals — call all pending signal callbacks. Callback
        // dispatch is runtime-only because a callback may suspend; ordinary
        // eval entry points already execute through that runtime ABI.
        env.set(
            sema_core::intern("sys/check-signals"),
            Value::native_fn(NativeFn::with_payload_result(
                "sys/check-signals",
                registry,
                check_signals,
            )),
        );
    }

    // The documented non-Unix ABI is a no-op, not an unbound symbol. Keep the
    // same arity/keyword validation while retaining no callback values.
    #[cfg(not(unix))]
    {
        register_fn(env, "sys/on-signal", |args| {
            check_arity!(args, "sys/on-signal", 2);
            let keyword = args[0]
                .as_keyword()
                .ok_or_else(|| SemaError::type_error("keyword", args[0].type_name()))?;
            match keyword.as_str() {
                "winch" | "int" | "term" => Ok(Value::nil()),
                other => Err(SemaError::eval(format!(
                    "sys/on-signal: unknown signal :{other}; use :winch, :int, or :term"
                ))),
            }
        });
        register_fn(env, "sys/check-signals", |args| {
            check_arity!(args, "sys/check-signals", 0);
            Ok(Value::nil())
        });
    }
}
