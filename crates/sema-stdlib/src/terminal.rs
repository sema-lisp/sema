use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::io::Write;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use sema_core::cycle::GcEdge;
use sema_core::runtime::{
    CancelDisposition, CancelHook, CancelHookError, CompletionDecoder, CompletionKind,
    DecodedCompletion, ExternalFailure, InterruptibleResource, NativeCallContext,
    NativeContinuation, NativeOutcome, NativeResult, NativeSuspend, PreparedExternalOperation,
    ResumeInput, SendPayload, Trace, WaitKind,
};
use sema_core::{check_arity, NativeFn, SemaError, Value, ValueView};

use crate::register_fn;

fn wrap_sgr(text: &str, code: &str) -> String {
    format!("\x1b[{code}m{text}\x1b[0m")
}

fn make_style_fn(env: &sema_core::Env, name: &str, code: &str) {
    let code = code.to_string();
    let fn_name = name.to_string();
    register_fn(env, name, move |args| {
        check_arity!(args, &fn_name, 1);
        let text = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        Ok(Value::string(&wrap_sgr(text, &code)))
    });
}

const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const SPINNER_INTERVAL_MS: u64 = 80;
/// One spinner frame interval. The render thread parks on its condvar for at
/// most this long between frames, so a `stop` that fires mid-park wakes it now
/// and a `join` after `stop` is bounded by (at most) one interval.
const SPINNER_INTERVAL: Duration = Duration::from_millis(SPINNER_INTERVAL_MS);

/// Live count of spinner render threads (test observation hook). A thread bumps
/// this when it starts and drops it just before it exits, letting a test assert
/// interpreter teardown leaves no live spinner thread. Process-global; nextest's
/// process-per-test isolation keeps the count local to one test.
static SPINNER_LIVE_THREADS: AtomicUsize = AtomicUsize::new(0);

/// Spinner render threads currently alive.
pub fn spinner_live_thread_count() -> usize {
    SPINNER_LIVE_THREADS.load(Ordering::SeqCst)
}

/// Cross-thread stop signal for one spinner render thread: a flag plus a condvar
/// so `stop` wakes a parked thread immediately instead of waiting out its frame
/// interval. Holds only POD state (no `Value`/`Env`), preserving CORE-2 I2.
struct SpinnerStop {
    stopped: Mutex<bool>,
    wake: Condvar,
}

impl SpinnerStop {
    fn new() -> Self {
        Self {
            stopped: Mutex::new(false),
            wake: Condvar::new(),
        }
    }

    /// Signal the render thread to stop and wake it out of its frame park now.
    fn stop(&self) {
        let mut stopped = self.stopped.lock().unwrap_or_else(|e| e.into_inner());
        *stopped = true;
        self.wake.notify_all();
    }

    fn is_stopped(&self) -> bool {
        *self.stopped.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Park up to one frame interval, returning the moment `stop` fires. Returns
    /// `true` once stopped so the render loop exits promptly. This replaces the
    /// bare `thread::sleep` frame loop, whose sleeping thread could only be
    /// stopped after its full interval elapsed (and never on teardown).
    fn park_frame(&self) -> bool {
        let stopped = self.stopped.lock().unwrap_or_else(|e| e.into_inner());
        let (stopped, _timed_out) = self
            .wake
            .wait_timeout_while(stopped, SPINNER_INTERVAL, |stopped| !*stopped)
            .unwrap_or_else(|e| e.into_inner());
        *stopped
    }
}

/// A RESOURCE-OWNED spinner: its stop signal, live message cell, and render
/// thread. Held POD-only (no `Value`/`Env`), so the owning registry stays
/// `Value`-free (CORE-2 I2).
struct SpinnerHandle {
    stop: Arc<SpinnerStop>,
    message: Arc<Mutex<String>>,
    thread: Option<JoinHandle<()>>,
}

impl SpinnerHandle {
    /// Signal stop, then join the render thread. The condvar wake makes the
    /// thread exit within one frame interval, so this join is bounded.
    fn stop_and_join(mut self) {
        self.stop.stop();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// INTERPRETER-SHARED registry of live spinners, owned via an `Rc` held by the
/// `term/spinner-*` natives (the `fs_watch` / B5 `TtyRegistry` model). A weak
/// interpreter-teardown hook stops and joins every spinner still parked here, so
/// teardown leaves no live render thread.
struct SpinnerRegistry {
    spinners: RefCell<HashMap<i64, SpinnerHandle>>,
    next_id: Cell<i64>,
    teardown_hook_registered: Cell<bool>,
}

impl SpinnerRegistry {
    fn new() -> Self {
        Self {
            spinners: RefCell::new(HashMap::new()),
            next_id: Cell::new(0),
            teardown_hook_registered: Cell::new(false),
        }
    }

    fn insert(&self, handle: SpinnerHandle) -> i64 {
        let id = self.next_id.get();
        self.next_id.set(id.wrapping_add(1));
        self.spinners.borrow_mut().insert(id, handle);
        id
    }

    fn take(&self, id: i64) -> Option<SpinnerHandle> {
        self.spinners.borrow_mut().remove(&id)
    }

    fn update(&self, id: i64, msg: String) {
        if let Some(handle) = self.spinners.borrow().get(&id) {
            *handle.message.lock().unwrap_or_else(|e| e.into_inner()) = msg;
        }
    }

    fn ensure_teardown_hook(self: &Rc<Self>, ctx: &sema_core::EvalContext) {
        if !self.teardown_hook_registered.replace(true) {
            let registry = Rc::downgrade(self);
            ctx.register_interpreter_teardown_hook(move || {
                if let Some(registry) = registry.upgrade() {
                    registry.stop_all();
                }
            });
        }
    }

    fn stop_all(&self) {
        for (_, handle) in std::mem::take(&mut *self.spinners.borrow_mut()) {
            handle.stop_and_join();
        }
        self.teardown_hook_registered.set(false);
    }
}

/// Spawn the render thread: park on the condvar between frames so `stop` wakes it
/// immediately. Bumps the live-thread gauge for its lifetime.
fn spawn_spinner(
    stop: Arc<SpinnerStop>,
    message: Arc<Mutex<String>>,
) -> Result<JoinHandle<()>, SemaError> {
    std::thread::Builder::new()
        .name("sema-spinner".to_string())
        .spawn(move || {
            SPINNER_LIVE_THREADS.fetch_add(1, Ordering::SeqCst);
            let mut frame_idx = 0usize;
            loop {
                if stop.is_stopped() {
                    break;
                }
                let msg = message.lock().unwrap_or_else(|e| e.into_inner()).clone();
                let frame = SPINNER_FRAMES[frame_idx % SPINNER_FRAMES.len()];
                {
                    let mut stderr = std::io::stderr().lock();
                    let _ = write!(stderr, "\r\x1b[K{frame} {msg}");
                    let _ = stderr.flush();
                }
                frame_idx += 1;
                if stop.park_frame() {
                    break;
                }
            }
            SPINNER_LIVE_THREADS.fetch_sub(1, Ordering::SeqCst);
        })
        .map_err(|e| {
            SemaError::eval(format!(
                "term/spinner-start: failed to spawn render thread: {e}"
            ))
        })
}

/// Clear the spinner's line and, if a non-empty symbol/text was given, print
/// the final status. Shared by `term/spinner-stop`'s sync path and its runtime
/// offload's `decode` step (both run this on the VM thread — it's the same
/// stderr write either way, just reached after a join that may have been
/// offloaded to a blocking-tier worker).
fn spinner_finish(symbol: &str, text: &str) {
    let mut stderr = std::io::stderr().lock();
    let _ = write!(stderr, "\r\x1b[K");
    if !symbol.is_empty() || !text.is_empty() {
        let _ = writeln!(stderr, "{symbol} {text}");
    }
    let _ = stderr.flush();
}

/// Parse `term/spinner-stop`'s id plus the optional `{:symbol … :text …}` final
/// status. The options map is decoded to plain `String`s here, on the VM thread,
/// so nothing Sema-valued crosses into an offloaded worker.
fn parse_spinner_stop_args(args: &[Value]) -> Result<(i64, String, String), SemaError> {
    check_arity!(args, "term/spinner-stop", 1..=2);
    let id = args[0]
        .as_int()
        .ok_or_else(|| SemaError::type_error("integer", args[0].type_name()))?;
    let (symbol, text) = if args.len() == 2 {
        if let ValueView::Map(opts) = args[1].view() {
            let symbol = opts
                .get(&Value::keyword("symbol"))
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .unwrap_or_default();
            let text = opts
                .get(&Value::keyword("text"))
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .unwrap_or_default();
            (symbol, text)
        } else {
            (String::new(), String::new())
        }
    } else {
        (String::new(), String::new())
    };
    Ok((id, symbol, text))
}

/// Host / non-quantum `term/spinner-stop`: remove the spinner, signal stop, and
/// join the render thread on the calling thread. The condvar wake bounds this
/// join by one frame interval. An unknown id is a nil no-op (never a stray line
/// clear), matching the legacy behavior.
fn spinner_stop_sync(registry: &SpinnerRegistry, args: &[Value]) -> Result<Value, SemaError> {
    let (id, symbol, text) = parse_spinner_stop_args(args)?;
    let Some(handle) = registry.take(id) else {
        return Ok(Value::nil());
    };
    handle.stop_and_join();
    spinner_finish(&symbol, &text);
    Ok(Value::nil())
}

/// Runtime (quantum) `term/spinner-stop`: signal stop + wake on the VM thread
/// (fast, non-blocking), then offload only the bounded `join` via an External
/// wait so a sibling task runs while the render thread winds down. The final
/// status renders on the VM thread in the decoder.
fn spinner_stop_offload(registry: &SpinnerRegistry, args: &[Value]) -> NativeResult {
    let (id, symbol, text) = parse_spinner_stop_args(args)?;
    let Some(mut handle) = registry.take(id) else {
        return Ok(NativeOutcome::Return(Value::nil()));
    };
    handle.stop.stop();
    let thread = handle.thread.take();
    Ok(NativeOutcome::Suspend(build_spinner_join_suspend(
        thread, symbol, text,
    )))
}

/// Completion tag for the offloaded spinner join (`"spnj"`).
const SPINNER_JOIN_COMPLETION_KIND: u64 = 0x7370_6e6a;

/// Build the External spinner-join suspension: a blocking-tier job joins the
/// (already-stopped) render thread; the decoder then renders the final status on
/// the VM thread and returns nil. Modeled on `workflow/run`'s flush-ack barrier.
fn build_spinner_join_suspend(
    thread: Option<JoinHandle<()>>,
    symbol: String,
    text: String,
) -> NativeSuspend {
    let kind = CompletionKind::try_from_raw(SPINNER_JOIN_COMPLETION_KIND)
        .expect("spinner join completion kind is nonzero");
    let resource = InterruptibleResource::new("term/spinner-join", Box::new(SpinnerJoinCancelHook));
    let prepared = PreparedExternalOperation::interruptible_blocking(
        kind,
        Box::new(SpinnerJoinDecoder { symbol, text }),
        resource,
        move || {
            // Join on a blocking-tier worker (NOT the VM thread). The stop flag +
            // condvar wake fired before dispatch, so the render thread exits
            // within one frame interval and this join is bounded.
            if let Some(thread) = thread {
                let _ = thread.join();
            }
            Ok(Box::new(()) as SendPayload)
        },
    );
    NativeSuspend {
        wait: WaitKind::External(Box::new(prepared)),
        continuation: Box::new(SpinnerJoinContinuation),
    }
}

/// Decoder for the offloaded spinner join: ignores the (unit) job payload and
/// renders the final status on the VM thread. Holds only POD `String`s — no
/// `Value` edges (CORE-2 I2).
struct SpinnerJoinDecoder {
    symbol: String,
    text: String,
}

impl Trace for SpinnerJoinDecoder {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}

impl CompletionDecoder for SpinnerJoinDecoder {
    fn decode(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        result: Result<SendPayload, ExternalFailure>,
    ) -> DecodedCompletion {
        result.map_err(|failure| {
            SemaError::eval(format!("term/spinner-stop join: {}", failure.message()))
        })?;
        spinner_finish(&self.symbol, &self.text);
        Ok(Value::nil())
    }
}

/// Resumes the parked `term/spinner-stop` with nil once the join completes. A
/// cancellation propagates (the render thread was already signaled and winds
/// down on its own), settling the task promptly.
struct SpinnerJoinContinuation;

impl Trace for SpinnerJoinContinuation {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}

impl NativeContinuation for SpinnerJoinContinuation {
    fn resume(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        match input {
            ResumeInput::Returned(value) => Ok(NativeOutcome::Return(value)),
            ResumeInput::Failed(error) => Err(error),
            ResumeInput::Cancelled(reason) => Err(SemaError::eval(format!(
                "term/spinner-stop was cancelled ({reason:?})"
            ))),
            ResumeInput::Runtime(_) => Err(SemaError::eval(
                "term/spinner-stop: unexpected runtime response awaiting spinner join",
            )),
        }
    }
}

/// No-op cancel hook: the stop flag + condvar wake already fired before dispatch,
/// so a cancelled join has nothing to abort — the render thread winds down on its
/// own and the worker reaps it.
struct SpinnerJoinCancelHook;

impl Trace for SpinnerJoinCancelHook {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}

impl CancelHook for SpinnerJoinCancelHook {
    fn cancel(&mut self) -> Result<CancelDisposition, CancelHookError> {
        Ok(CancelDisposition::Reaped)
    }
    fn reap(&mut self) -> Result<CancelDisposition, CancelHookError> {
        Ok(CancelDisposition::Reaped)
    }
}

pub fn register(env: &sema_core::Env) {
    // Modifiers
    make_style_fn(env, "term/bold", "1");
    make_style_fn(env, "term/dim", "2");
    make_style_fn(env, "term/italic", "3");
    make_style_fn(env, "term/underline", "4");
    make_style_fn(env, "term/inverse", "7");
    make_style_fn(env, "term/strikethrough", "9");

    // Foreground colors
    make_style_fn(env, "term/black", "30");
    make_style_fn(env, "term/red", "31");
    make_style_fn(env, "term/green", "32");
    make_style_fn(env, "term/yellow", "33");
    make_style_fn(env, "term/blue", "34");
    make_style_fn(env, "term/magenta", "35");
    make_style_fn(env, "term/cyan", "36");
    make_style_fn(env, "term/white", "37");
    make_style_fn(env, "term/gray", "90");

    // (term/style "text" :bold :red ...)
    register_fn(env, "term/style", |args| {
        check_arity!(args, "term/style", 1..);
        let text = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;

        let mut codes: Vec<&str> = Vec::new();
        for arg in &args[1..] {
            let kw = arg
                .as_keyword()
                .ok_or_else(|| SemaError::type_error("keyword", arg.type_name()))?;
            let code = match kw.as_str() {
                // Modifiers
                "bold" => "1",
                "dim" => "2",
                "italic" => "3",
                "underline" => "4",
                "inverse" => "7",
                "strikethrough" => "9",
                // Colors
                "black" => "30",
                "red" => "31",
                "green" => "32",
                "yellow" => "33",
                "blue" => "34",
                "magenta" => "35",
                "cyan" => "36",
                "white" => "37",
                "gray" => "90",
                other => {
                    return Err(SemaError::eval(format!(
                        "term/style: unknown style keyword :{other}"
                    )))
                }
            };
            codes.push(code);
        }
        if codes.is_empty() {
            return Ok(Value::string(text));
        }
        let combined = codes.join(";");
        Ok(Value::string(&wrap_sgr(text, &combined)))
    });

    // (term/strip "ansi-string") -> plain string
    register_fn(env, "term/strip", |args| {
        check_arity!(args, "term/strip", 1);
        let text = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        // Strip ANSI escape sequences (full CSI + OSC, not just SGR). Shared with
        // string/width & string/wrap via crate::strip_ansi.
        Ok(Value::string(&crate::strip_ansi(text)))
    });

    // (term/rgb "text" r g b) -> 24-bit color
    register_fn(env, "term/rgb", |args| {
        check_arity!(args, "term/rgb", 4);
        let text = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let r = args[1]
            .as_int()
            .ok_or_else(|| SemaError::type_error("integer", args[1].type_name()))?;
        let g = args[2]
            .as_int()
            .ok_or_else(|| SemaError::type_error("integer", args[2].type_name()))?;
        let b = args[3]
            .as_int()
            .ok_or_else(|| SemaError::type_error("integer", args[3].type_name()))?;
        Ok(Value::string(&format!(
            "\x1b[38;2;{r};{g};{b}m{text}\x1b[0m"
        )))
    });

    // The spinner registry is INTERPRETER-SHARED: created once here, owned by the
    // `term/spinner-*` natives via `Rc`, and torn down by a weak interpreter hook
    // (the `fs_watch` / B5 `TtyRegistry` model).
    let spinner_registry = Rc::new(SpinnerRegistry::new());

    // (term/spinner-start "message") -> spinner-id
    // Spawns a RESOURCE-OWNED render thread parked in the registry. It is stopped
    // and joined by an explicit `term/spinner-stop` or, if it outlives the
    // interpreter, by the registry's teardown hook — never left running.
    let start_registry = Rc::clone(&spinner_registry);
    env.set(
        sema_core::intern("term/spinner-start"),
        Value::native_fn(NativeFn::with_ctx(
            "term/spinner-start",
            move |ctx, args| {
                check_arity!(args, "term/spinner-start", 1);
                let msg = args[0]
                    .as_str()
                    .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?
                    .to_string();

                let stop = Arc::new(SpinnerStop::new());
                let message = Arc::new(Mutex::new(msg));
                let thread = spawn_spinner(Arc::clone(&stop), Arc::clone(&message))?;
                let id = start_registry.insert(SpinnerHandle {
                    stop,
                    message,
                    thread: Some(thread),
                });
                start_registry.ensure_teardown_hook(ctx);
                Ok(Value::int(id))
            },
        )),
    );

    // (term/spinner-stop id) or (term/spinner-stop id {:symbol "✔" :text "Done" :color :green})
    // In a runtime quantum the bounded join is offloaded via an External wait so a
    // sibling task runs meanwhile; the host (non-quantum) path joins inline,
    // bounded by one frame interval via the condvar wake.
    let stop_value_registry = Rc::clone(&spinner_registry);
    let stop_runtime_registry = Rc::clone(&spinner_registry);
    env.set(
        sema_core::intern("term/spinner-stop"),
        Value::native_fn(NativeFn::simple_with_runtime(
            "term/spinner-stop",
            move |args| spinner_stop_sync(&stop_value_registry, args),
            move |_ctx, args| {
                if sema_core::in_runtime_quantum() {
                    spinner_stop_offload(&stop_runtime_registry, args)
                } else {
                    spinner_stop_sync(&stop_runtime_registry, args).map(NativeOutcome::Return)
                }
            },
        )),
    );

    // (term/spinner-update id "new message")
    let update_registry = Rc::clone(&spinner_registry);
    register_fn(env, "term/spinner-update", move |args| {
        check_arity!(args, "term/spinner-update", 2);
        let id = args[0]
            .as_int()
            .ok_or_else(|| SemaError::type_error("integer", args[0].type_name()))?;
        let new_msg = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?
            .to_string();
        update_registry.update(id, new_msg);
        Ok(Value::nil())
    });

    register_screen_control(env);
}

// ── Screen control ──────────────────────────────────────────────────────────
//
// Raw terminal screen primitives that emit ANSI/VT control sequences to stdout
// so Sema TUIs (e.g. Sema Coder) don't have to hand-write escape codes. Each
// write is flushed immediately so the effect is visible at human interaction
// speed; render loops can still batch styled strings and call `term/flush`.

/// Write a control sequence to stdout and flush it. Control codes are useless
/// if they sit in a block buffer, so flush-per-call keeps behavior predictable.
fn emit(seq: &str) -> Result<Value, SemaError> {
    let mut out = std::io::stdout();
    out.write_all(seq.as_bytes())
        .and_then(|_| out.flush())
        .map_err(|e| SemaError::Io(format!("term: {e}")))?;
    Ok(Value::nil())
}

/// Register a zero-arg fn that emits a fixed control sequence.
fn make_emit_fn(env: &sema_core::Env, name: &str, seq: &'static str) {
    let fn_name = name.to_string();
    register_fn(env, name, move |args| {
        check_arity!(args, &fn_name, 0);
        emit(seq)
    });
}

/// 1-based row/col argument → terminal coordinate, clamped to ≥ 1 (VT cursor
/// addressing is 1-based; 0 is treated as 1 by most terminals but we normalize).
fn coord(arg: &Value) -> Result<i64, SemaError> {
    let n = arg
        .as_int()
        .ok_or_else(|| SemaError::type_error("integer", arg.type_name()))?;
    Ok(n.max(1))
}

fn register_screen_control(env: &sema_core::Env) {
    // Alternate screen buffer — enter on app start, leave to restore the user's
    // scrollback exactly as it was.
    make_emit_fn(env, "term/enter-alt-screen", "\x1b[?1049h");
    make_emit_fn(env, "term/leave-alt-screen", "\x1b[?1049l");

    // Clearing
    make_emit_fn(env, "term/clear", "\x1b[2J\x1b[H"); // whole screen + home
    make_emit_fn(env, "term/clear-line", "\x1b[2K"); // current line
    make_emit_fn(env, "term/clear-below", "\x1b[0J"); // cursor → end of screen

    // Cursor
    make_emit_fn(env, "term/cursor-home", "\x1b[H");
    make_emit_fn(env, "term/hide-cursor", "\x1b[?25l");
    make_emit_fn(env, "term/show-cursor", "\x1b[?25h");
    make_emit_fn(env, "term/save-cursor", "\x1b7");
    make_emit_fn(env, "term/restore-cursor", "\x1b8");

    // Mouse reporting: button events (1000) + button-motion/drag (1002) + SGR
    // extended coords (1006). io/read-key decodes the reports into {:kind :mouse …}.
    make_emit_fn(
        env,
        "term/enable-mouse",
        "\x1b[?1000h\x1b[?1002h\x1b[?1006h",
    );
    make_emit_fn(
        env,
        "term/disable-mouse",
        "\x1b[?1000l\x1b[?1002l\x1b[?1006l",
    );

    // Kitty keyboard protocol (opt-in). Default flags 17 = disambiguate (1) +
    // report-associated-text (16); pass a bitmask to request more (add 2 for
    // event types, 4 for alternate keys — see the kitty spec). Terminals without
    // support silently ignore this and keys keep coming through the legacy path.
    // io/read-key decodes `ESC [ … u` events into the usual key maps (+ :mods,
    // and :event when event types are enabled). Restore with the stack pop on exit.
    register_fn(env, "term/enable-kitty-keys!", |args| {
        if args.len() > 1 {
            return Err(SemaError::arity(
                "term/enable-kitty-keys!",
                "0-1",
                args.len(),
            ));
        }
        let flags = match args.first() {
            None => 17,
            Some(v) => v
                .as_int()
                .ok_or_else(|| SemaError::type_error("integer", v.type_name()))?,
        };
        emit(&format!("\x1b[>{flags}u"))
    });
    make_emit_fn(env, "term/disable-kitty-keys!", "\x1b[<u");
    // Query the terminal's active kitty flags (`CSI ?u`); the reply arrives via
    // io/read-key as {:kind :kitty-flags :flags N} (nothing if unsupported).
    make_emit_fn(env, "term/query-kitty-keys", "\x1b[?u");

    // Bracketed paste (opt-in): the terminal wraps pasted text in ESC[200~ … 201~,
    // which io/read-key returns as {:kind :paste :text …} instead of live keys.
    make_emit_fn(env, "term/enable-bracketed-paste", "\x1b[?2004h");
    make_emit_fn(env, "term/disable-bracketed-paste", "\x1b[?2004l");

    // Focus reporting (opt-in): ESC[I / ESC[O on focus in/out → {:kind :focus …}.
    make_emit_fn(env, "term/enable-focus-events", "\x1b[?1004h");
    make_emit_fn(env, "term/disable-focus-events", "\x1b[?1004l");

    // Terminal queries — each writes a request whose reply arrives via io/read-key:
    //   query-primary/secondary-da   → {:kind :device-attributes :device …}
    // (query-cursor-position lives in io.rs — it must also arm the CPR flag so a
    //  reply is told apart from modified-F3, which is byte-identical.)
    make_emit_fn(env, "term/query-primary-da", "\x1b[c");
    make_emit_fn(env, "term/query-secondary-da", "\x1b[>c");

    make_emit_fn(env, "term/bell", "\x07");

    register_fn(env, "term/move-to", |args| {
        check_arity!(args, "term/move-to", 2);
        let row = coord(&args[0])?;
        let col = coord(&args[1])?;
        emit(&format!("\x1b[{row};{col}H"))
    });

    register_fn(env, "term/write-at", |args| {
        check_arity!(args, "term/write-at", 3);
        let row = coord(&args[0])?;
        let col = coord(&args[1])?;
        let text = args[2]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[2].type_name()))?;
        emit(&format!("\x1b[{row};{col}H{text}"))
    });

    register_fn(env, "term/set-title", |args| {
        check_arity!(args, "term/set-title", 1);
        let title = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        // OSC 0 sets both icon name and window title; BEL terminates.
        emit(&format!("\x1b]0;{title}\x07"))
    });

    // term/flush — explicit flush for code that batches styled writes with
    // io/print before showing a frame. (Control fns above already self-flush.)
    register_fn(env, "term/flush", |args| {
        check_arity!(args, "term/flush", 0);
        std::io::stdout()
            .flush()
            .map_err(|e| SemaError::Io(format!("term/flush: {e}")))?;
        Ok(Value::nil())
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    // Both spinner tests move the process-global `SPINNER_LIVE_THREADS` gauge, so
    // serialize them (nextest already runs each in its own process; this keeps the
    // absolute 0→1→0 assertions safe under a threaded `cargo test` too).
    static SPINNER_TEST_LOCK: Mutex<()> = Mutex::new(());

    // `stop_all` joins each render thread, so the gauge is decremented by the time
    // teardown returns (no race).
    #[test]
    fn teardown_hook_stops_and_joins_live_spinner() {
        let _serialize = SPINNER_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let ctx = sema_core::EvalContext::new();
        let registry = Rc::new(SpinnerRegistry::new());

        // Start one live spinner parked in the registry.
        let stop = Arc::new(SpinnerStop::new());
        let message = Arc::new(Mutex::new("teardown probe".to_string()));
        let thread = spawn_spinner(Arc::clone(&stop), Arc::clone(&message)).expect("spawn spinner");
        let id = registry.insert(SpinnerHandle {
            stop,
            message,
            thread: Some(thread),
        });

        // Wait (bounded) for the render thread to actually start.
        let deadline = Instant::now() + Duration::from_secs(5);
        while spinner_live_thread_count() == 0 && Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert_eq!(
            spinner_live_thread_count(),
            1,
            "spinner render thread must be live before teardown"
        );
        assert!(registry.spinners.borrow().contains_key(&id));

        // Registering the teardown hook is idempotent — the flag flips exactly once.
        assert!(!registry.teardown_hook_registered.get());
        registry.ensure_teardown_hook(&ctx);
        assert!(
            registry.teardown_hook_registered.get(),
            "ensure_teardown_hook must register the interpreter hook"
        );
        registry.ensure_teardown_hook(&ctx); // second call is a no-op

        // Firing the interpreter teardown hooks stops+joins every live spinner.
        assert!(ctx.try_run_interpreter_teardown_hooks());
        assert!(
            registry.spinners.borrow().is_empty(),
            "teardown must drain the spinner registry"
        );
        assert!(
            !registry.teardown_hook_registered.get(),
            "stop_all must reset the teardown-hook flag"
        );
        assert_eq!(
            spinner_live_thread_count(),
            0,
            "teardown must leave no live spinner render thread"
        );
    }

    // A `stop` that races the thread's frame park wakes it immediately (condvar),
    // so the join returns well within a couple of frame intervals rather than
    // hanging or waiting out a bare sleep.
    #[test]
    fn stop_wakes_and_joins_render_thread_promptly() {
        let _serialize = SPINNER_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let baseline = spinner_live_thread_count();
        let stop = Arc::new(SpinnerStop::new());
        let message = Arc::new(Mutex::new("wake probe".to_string()));
        let thread = spawn_spinner(Arc::clone(&stop), Arc::clone(&message)).expect("spawn spinner");
        let handle = SpinnerHandle {
            stop,
            message,
            thread: Some(thread),
        };

        let deadline = Instant::now() + Duration::from_secs(5);
        while spinner_live_thread_count() <= baseline && Instant::now() < deadline {
            std::thread::yield_now();
        }

        let started = Instant::now();
        handle.stop_and_join();
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "condvar wake must join the render thread promptly, took {:?}",
            started.elapsed()
        );
    }
}
