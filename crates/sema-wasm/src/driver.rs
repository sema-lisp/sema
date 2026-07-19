//! Macrotask-driven Promise host for the unified runtime.
//!
//! `WasmInterpreter::evalPromise`, the async compatibility wrappers, compiled
//! archive entry points, and the Promise debugger share one interpreter and
//! persistent runtime. This module drives their roots without blocking:
//!
//! * one call submits ONE root via `Interpreter::submit_str` and returns a
//!   `js_sys::Promise` immediately — the program body is never replayed;
//! * settlement is pumped by repeated, BOUNDED, NON-BLOCKING
//!   `Interpreter::drive_turn` calls scheduled across browser macrotasks
//!   (`schedule_drive`), which lets browser timer and fetch callbacks resume
//!   the original root in place.
//!
//! Each turn passes this driver's exact root set to the runtime. Dual-ABI HTTP
//! and output routing likewise consult the currently executing `RootId`, so a
//! synchronous re-entry cannot execute or capture a pending Promise root.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use js_sys::Function;
use sema_core::runtime::{
    downcast_send_payload, CompletionDecoder, CompletionKind, ExecutorAttachError,
    ExecutorDispatch, ExecutorLease, ExecutorShutdown, ExecutorSnapshot, ExecutorSubmission,
    ExternalFailure, InterruptibleResource, IoExecutor, NativeCallContext, NativeContinuation,
    NativeOutcome, NativeResult, NativeSuspend, PreparedExternalOperation, ResumeInput, RootId,
    RunningSubmission, RuntimeId, SendPayload, SubmissionRejected, Trace, WaitKind,
};
use sema_core::{SemaError, Value, ValueView};
use sema_eval::Interpreter;
use sema_vm::runtime::{DriveState, RootHandle, RootOptions, RootPoll};
use sema_vm::{DebugState, StepMode, StopInfo, StopReason, VM};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;
use web_time::Instant;

use crate::output::{PromiseOutput, PromiseOutputEvent};

const DEBUG_INSTRUCTION_BUDGET: u32 = 500_000;
const LEGACY_DEBUG_CONFLICT: &str =
    "Promise-driven execution cannot start while the synchronous debugger is active on this interpreter";
const PROMISE_PREPARATION_LOST: &str =
    "Promise-driven execution lost its admission reservation before root submission";

// ─────────────────────────────────────────────────────────────────────────
// Promise table + macrotask driver
// ─────────────────────────────────────────────────────────────────────────

/// One pending root's JS resolve/reject pair, plus the `RootHandle` itself —
/// the handle MUST be retained until settlement is observed and delivered:
/// `RootHandle::drop` only releases this table's retain count (the root stays
/// alive internally via the runtime's own root table regardless), but
/// dropping it before polling would leave no way to ask the runtime for the
/// settlement this table exists to deliver.
struct PromiseSettlers {
    handle: RootHandle,
    resolve: Function,
    reject: Function,
}

/// One JS debugger action (`start`, `continue`, or a step) waiting for the
/// session to reach its next stable stop or terminal result.
struct DebugActionSettler {
    resolve: Function,
    include_breakpoint_info: bool,
}

/// Promise-driven debugger state owned by one interpreter. The runtime owns
/// the paused VM; this record owns the exact root handle, its `DebugState`, and
/// the currently outstanding JS action Promise (if execution is in flight).
struct PromiseDebugSession {
    debug: DebugState,
    handle: RootHandle,
    action: Option<DebugActionSettler>,
    output: Vec<PromiseOutputEvent>,
    valid_lines: Vec<u32>,
    breakpoints: Vec<u32>,
}

thread_local! {
    /// Completion callbacks receive only the runtime identity through the
    /// `Send + Sync` executor ABI. This registry routes that identity back to
    /// the matching single-threaded driver without owning it.
    static DRIVER_REGISTRY: RefCell<HashMap<RuntimeId, Weak<PromiseDriver>>> =
        RefCell::new(HashMap::new());
}

/// Promise-root state owned by one [`crate::WasmInterpreter`]. Local root ids
/// are only meaningful inside this table; scheduling and cancellation never
/// inspect another interpreter's roots.
pub(crate) struct PromiseDriver {
    interp: Rc<Interpreter>,
    promises: RefCell<HashMap<RootId, PromiseSettlers>>,
    /// Source compilation and Promise-debugger setup can invoke registered JS
    /// functions during macro expansion. A scoped count closes legacy-debugger
    /// admission until that preparation either fails or hands off to a root.
    preparing_roots: Cell<usize>,
    drive_scheduled: Cell<bool>,
    drive_timeout_id: Cell<Option<i32>>,
    runtime_id: Cell<Option<RuntimeId>>,
    /// A pending JS Promise must keep its interpreter alive even if the JS
    /// wrapper is collected while an external completion is outstanding. The
    /// temporary self-retain is released when the last Promise settles.
    active_retain: RefCell<Option<Rc<PromiseDriver>>>,
    output: PromiseOutput,
    debug_session: RefCell<Option<PromiseDebugSession>>,
    debug_stop_requested: Cell<Option<RootId>>,
    debug_breakpoints_pending: RefCell<Option<Vec<u32>>>,
    debug_root: Cell<Option<RootId>>,
    retiring_debug_roots: RefCell<HashMap<RootId, RootHandle>>,
}

impl PromiseDriver {
    pub(crate) fn new(interp: Rc<Interpreter>) -> Rc<Self> {
        Rc::new(Self {
            interp,
            promises: RefCell::new(HashMap::new()),
            preparing_roots: Cell::new(0),
            drive_scheduled: Cell::new(false),
            drive_timeout_id: Cell::new(None),
            runtime_id: Cell::new(None),
            active_retain: RefCell::new(None),
            output: PromiseOutput::default(),
            debug_session: RefCell::new(None),
            debug_stop_requested: Cell::new(None),
            debug_breakpoints_pending: RefCell::new(None),
            debug_root: Cell::new(None),
            retiring_debug_roots: RefCell::new(HashMap::new()),
        })
    }

    pub(crate) fn set_output_sink(&self, sink: Option<Function>) {
        self.output.set_sink(sink);
    }

    pub(crate) fn take_output_sink(&self) -> Option<Function> {
        self.output.take_sink()
    }
}

/// Owns the Promise debugger session outside its `RefCell` while the runtime
/// can invoke arbitrary JavaScript. Re-entrant debugger calls communicate
/// through the driver's scalar/pending-command fields; dropping this guard
/// restores the session on every Rust exit path.
struct PromiseDebugDrive<'a> {
    driver: &'a PromiseDriver,
    session: Option<PromiseDebugSession>,
}

impl<'a> PromiseDebugDrive<'a> {
    fn begin(driver: &'a PromiseDriver) -> Self {
        let session = driver.debug_session.borrow_mut().take();
        debug_assert!(driver.debug_stop_requested.replace(None).is_none());
        driver.debug_breakpoints_pending.borrow_mut().take();
        Self { driver, session }
    }

    fn session_mut(&mut self) -> Option<&mut PromiseDebugSession> {
        self.session.as_mut()
    }

    fn restore(&mut self) {
        let Some(mut session) = self.session.take() else {
            self.driver.debug_breakpoints_pending.borrow_mut().take();
            return;
        };
        if let Some(lines) = self.driver.debug_breakpoints_pending.borrow_mut().take() {
            set_session_breakpoints(&mut session, &lines);
        }
        let mut slot = self.driver.debug_session.borrow_mut();
        assert!(
            slot.is_none(),
            "Promise debug session cannot be replaced during a drive"
        );
        *slot = Some(session);
    }

    fn finish(mut self) -> Option<RootId> {
        let stop_requested = self.driver.debug_stop_requested.replace(None);
        self.restore();
        stop_requested
    }
}

impl Drop for PromiseDebugDrive<'_> {
    fn drop(&mut self) {
        self.restore();
        self.driver.debug_stop_requested.set(None);
    }
}

impl Drop for PromiseDriver {
    fn drop(&mut self) {
        if let Some(timeout) = self.drive_timeout_id.take() {
            clear_timeout(timeout);
        }
        if let Some(session) = self.debug_session.get_mut().take() {
            let root = session.handle.id();
            let _ = self.interp.command_handle().cancel_root(root);
            if self.interp.runtime().is_debug_paused_for(root) {
                self.interp.runtime().debug_cancel_paused_root(root);
            }
        }
        for root in self.retiring_debug_roots.get_mut().keys().copied() {
            let _ = self.interp.command_handle().cancel_root(root);
        }
        if let Some(runtime_id) = self.runtime_id.get() {
            DRIVER_REGISTRY.with(|registry| {
                registry.borrow_mut().remove(&runtime_id);
            });
        }
    }
}

/// Whether the currently executing runtime quantum belongs to the Promise
/// driver registered for its exact runtime. `RootId` includes `RuntimeId`, so
/// equal local ids in different interpreters cannot collide.
pub(crate) fn promise_driven_root_active() -> bool {
    let Some(root) = sema_core::current_root() else {
        return false;
    };
    DRIVER_REGISTRY.with(|registry| {
        registry
            .borrow()
            .get(&root.runtime())
            .and_then(Weak::upgrade)
            .is_some_and(|driver| driver.owns_root(root))
    })
}

pub(crate) fn ensure_promise_admission(driver: &PromiseDriver) -> Result<(), &'static str> {
    if crate::legacy_debug_active_for(&driver.interp) {
        Err(LEGACY_DEBUG_CONFLICT)
    } else {
        Ok(())
    }
}

/// Scoped Promise-side ownership while source/debug preparation may re-enter
/// JavaScript. The count supports nested Promise submissions on one
/// interpreter; `Drop` closes every early-return and unwind path.
pub(crate) struct PromiseAdmissionReservation {
    driver: Rc<PromiseDriver>,
}

impl PromiseAdmissionReservation {
    pub(crate) fn ensure_pending(&self) -> Result<(), &'static str> {
        if self.driver.preparing_roots.get() == 0 {
            return Err(PROMISE_PREPARATION_LOST);
        }
        ensure_promise_admission(&self.driver)
    }
}

impl Drop for PromiseAdmissionReservation {
    fn drop(&mut self) {
        let preparing = self.driver.preparing_roots.get();
        debug_assert!(preparing > 0, "Promise admission reservation underflow");
        self.driver.preparing_roots.set(preparing.saturating_sub(1));
    }
}

pub(crate) fn reserve_promise_admission(
    driver: &Rc<PromiseDriver>,
) -> Result<PromiseAdmissionReservation, &'static str> {
    ensure_promise_admission(driver)?;
    let preparing = driver
        .preparing_roots
        .get()
        .checked_add(1)
        .ok_or("too many nested Promise admission reservations")?;
    driver.preparing_roots.set(preparing);
    Ok(PromiseAdmissionReservation {
        driver: Rc::clone(driver),
    })
}

impl PromiseDriver {
    pub(crate) fn has_active_roots(&self) -> bool {
        !self.promises.borrow().is_empty()
            || self.debug_root.get().is_some()
            || !self.retiring_debug_roots.borrow().is_empty()
    }

    pub(crate) fn blocks_legacy_debug_start(&self) -> bool {
        self.preparing_roots.get() > 0 || self.has_active_roots()
    }

    fn owns_root(&self, root: RootId) -> bool {
        self.promises.borrow().contains_key(&root)
            || self.debug_root.get() == Some(root)
            || self.retiring_debug_roots.borrow().contains_key(&root)
    }

    fn owned_roots(&self) -> Vec<RootId> {
        let mut roots: Vec<_> = self.promises.borrow().keys().copied().collect();
        if let Some(root) = self.debug_root.get() {
            roots.push(root);
        }
        roots.extend(self.retiring_debug_roots.borrow().keys().copied());
        roots
    }
}

/// Submit `src` as a fresh root on `interp`'s persistent runtime with
/// `capture_output: true`, stash `resolve`/`reject`, and kick the macrotask
/// driver. Called by `WasmInterpreter::evalPromise`, which owns the
/// `js_sys::Promise` executor these callbacks come from.
///
/// `on_root`, if given, is invoked SYNCHRONOUSLY (before this function
/// returns, and therefore before `evalPromise`'s own `js_sys::Promise::new`
/// executor returns to its caller) with the new root's id as a JS `number` —
/// this is the only way a caller can learn the id in time to correlate a
/// later cancel with the exact root this call submitted (playground "Stop",
/// P6-3 step 3 / design doc §2.4). Not called on submission failure (there is
/// no root to report).
pub(crate) fn submit(
    driver: &Rc<PromiseDriver>,
    src: &str,
    resolve: Function,
    reject: Function,
    on_root: Option<Function>,
) {
    let reservation = match reserve_promise_admission(driver) {
        Ok(reservation) => reservation,
        Err(message) => {
            reject_with_message(&reject, message);
            return;
        }
    };
    let opts = RootOptions {
        name: None,
        capture_output: true,
    };
    match driver.interp.submit_str_guarded(src, opts, || {
        reservation.ensure_pending().map_err(SemaError::eval)
    }) {
        Ok(handle) => adopt(driver, handle, resolve, reject, on_root),
        Err(err) => {
            reject_with_error(&reject, &err);
        }
    }
}

/// Adopt an already-submitted root into the same Promise table used by
/// `evalPromise`. Compiled archive entry points use this after
/// `Interpreter::submit_compile_result`, so deserialization happens once and
/// the resulting VM is resumed in place across timer/external waits.
pub(crate) fn adopt(
    driver: &Rc<PromiseDriver>,
    handle: RootHandle,
    resolve: Function,
    reject: Function,
    on_root: Option<Function>,
) {
    let reservation = match reserve_promise_admission(driver) {
        Ok(reservation) => reservation,
        Err(message) => {
            handle.cancel(sema_core::runtime::CancelReason::HostStop);
            reject_with_message(&reject, message);
            schedule_drive(driver);
            return;
        }
    };
    if let Err(message) = reservation.ensure_pending() {
        handle.cancel(sema_core::runtime::CancelReason::HostStop);
        reject_with_message(&reject, message);
        schedule_drive(driver);
        return;
    }
    let root = handle.id();
    register_driver(driver, root.runtime());
    driver.promises.borrow_mut().insert(
        root,
        PromiseSettlers {
            handle,
            resolve,
            reject,
        },
    );
    retain_while_active(driver);
    if let Some(f) = on_root {
        let _ = f.call1(&JsValue::NULL, &JsValue::from_f64(root.get() as f64));
    }
    schedule_drive(driver);
}

fn retain_while_active(driver: &Rc<PromiseDriver>) {
    if driver.active_retain.borrow().is_none() {
        *driver.active_retain.borrow_mut() = Some(Rc::clone(driver));
    }
}

/// Request cancellation of the root whose id (as reported to `submit`'s
/// `on_root` callback) is `raw_root_id`. Looks the live `RootId` up by its
/// local numeric component in this driver's Promise table (there is no public
/// constructor from a raw `u64` back to a `RootId` — see
/// `sema_core::runtime::ids` — so a linear scan of the still-pending roots is
/// the supported way back) and routes through
/// `RuntimeCommandHandle::cancel_root`, exactly like a native host's Ctrl-C.
/// Returns `false` if no pending root matches `raw_root_id` (already settled,
/// or never existed) — a stale/late cancel is a harmless no-op, matching
/// `cancel_root`'s own liveness semantics.
pub(crate) fn cancel_root(driver: &Rc<PromiseDriver>, raw_root_id: f64) -> bool {
    let raw = raw_root_id as u64;
    let found = driver
        .promises
        .borrow()
        .keys()
        .find(|root| root.get() == raw)
        .copied();
    let cancelled = match found {
        Some(root) => driver.interp.command_handle().cancel_root(root),
        None => return false,
    };
    // The cancel command rides the runtime's own inbox, applied at the top of
    // the next `drive` turn — but a turn may currently be scheduled minutes
    // in the "future" via `schedule_drive_after`, honoring an unrelated (or
    // this very root's) `WaitKind::Timer` deadline. Without forcing an
    // immediate turn here, a cancelled `(async/sleep 5000)` would sit
    // uncancelled for up to 5 real seconds — `schedule_drive` cancels that
    // pending delayed timer and posts an immediate macrotask instead.
    schedule_drive(driver);
    cancelled
}

/// Start a cooperative debug root and return one Promise for the first stable
/// stop or terminal result. The seeded VM is submitted exactly once; later
/// actions resume this same root through [`resume_debug`].
pub(crate) fn start_debug(
    driver: &Rc<PromiseDriver>,
    vm: VM,
    debug: DebugState,
    valid_lines: Vec<u32>,
    breakpoints: Vec<u32>,
) -> js_sys::Promise {
    let driver = Rc::clone(driver);
    let mut vm = Some(vm);
    let mut debug = Some(debug);
    js_sys::Promise::new(&mut move |resolve, _reject| {
        let reservation = match reserve_promise_admission(&driver) {
            Ok(reservation) => reservation,
            Err(message) => {
                resolve_debug_immediately(&resolve, debug_error_result(message));
                return;
            }
        };
        if debug_action_is_driving(&driver) {
            resolve_debug_immediately(
                &resolve,
                debug_error_result("A Promise debug action is already running"),
            );
            return;
        }
        let replaced_own_session = stop_debug(&driver);
        if !replaced_own_session && driver.interp.runtime().is_debug_paused() {
            resolve_debug_immediately(
                &resolve,
                debug_error_result("another debugger is already paused on this interpreter"),
            );
            return;
        }
        let opts = RootOptions {
            name: Some("wasm-debugger".to_string()),
            capture_output: true,
        };
        let Some(vm) = vm.take() else {
            resolve_debug_immediately(
                &resolve,
                debug_error_result("debug VM was already submitted"),
            );
            return;
        };
        let Some(debug) = debug.take() else {
            resolve_debug_immediately(
                &resolve,
                debug_error_result("debug state was already submitted"),
            );
            return;
        };
        if let Err(message) = reservation.ensure_pending() {
            resolve_debug_immediately(&resolve, debug_error_result(message));
            return;
        }
        match driver.interp.runtime().submit_root_with_options(vm, &opts) {
            Ok(handle) => {
                let root = handle.id();
                register_driver(&driver, root.runtime());
                *driver.debug_session.borrow_mut() = Some(PromiseDebugSession {
                    debug,
                    handle,
                    action: Some(DebugActionSettler {
                        resolve,
                        include_breakpoint_info: true,
                    }),
                    output: Vec::new(),
                    valid_lines: valid_lines.clone(),
                    breakpoints: breakpoints.clone(),
                });
                driver.debug_root.set(Some(root));
                retain_while_active(&driver);
                schedule_drive(&driver);
            }
            Err(error) => resolve_debug_immediately(
                &resolve,
                debug_error_result(&format!("debug root submission failed: {error:?}")),
            ),
        }
    })
}

/// Resume the active Promise-driven debug session in place and settle when it
/// next stops or terminates. A second action cannot replace one already in
/// flight; it resolves to an explicit error instead.
pub(crate) fn resume_debug(driver: &Rc<PromiseDriver>, mode: StepMode) -> js_sys::Promise {
    let driver = Rc::clone(driver);
    js_sys::Promise::new(&mut move |resolve, _reject| {
        if debug_action_is_driving(&driver) {
            resolve_debug_immediately(
                &resolve,
                debug_error_result("A Promise debug action is already running"),
            );
            return;
        }
        let runtime = driver.interp.runtime();
        let install_error = {
            let mut slot = driver.debug_session.borrow_mut();
            match slot.as_mut() {
                None => Some("No active Promise debug session"),
                Some(session) if session.action.is_some() => {
                    Some("A Promise debug action is already running")
                }
                Some(session) if !runtime.is_debug_paused_for(session.handle.id()) => {
                    Some("The Promise debug session is not paused")
                }
                Some(session) => {
                    let root = session.handle.id();
                    session.debug.step_mode = mode;
                    if mode != StepMode::Continue {
                        if let Some(depth) =
                            runtime.with_paused_root_vm(root, |vm| vm.frame_count())
                        {
                            session.debug.step_frame_depth = depth;
                        }
                    }
                    session.action = Some(DebugActionSettler {
                        resolve: resolve.clone(),
                        include_breakpoint_info: false,
                    });
                    None
                }
            }
        };
        if let Some(message) = install_error {
            resolve_debug_immediately(&resolve, debug_error_result(message));
            return;
        }
        let root = driver
            .debug_session
            .borrow()
            .as_ref()
            .map(|session| session.handle.id());
        if root.is_none_or(|root| !runtime.debug_resume_root(root)) {
            let result = debug_error_result("The Promise debug session lost its paused root");
            let mut session = driver.debug_session.borrow_mut().take();
            let action = session.as_mut().and_then(|session| session.action.take());
            if let Some(session) = session {
                retire_debug_root(&driver, session.handle);
            } else {
                schedule_drive(&driver);
            }
            if let Some(action) = action {
                resolve_debug_immediately(&action.resolve, result);
            }
            return;
        }
        retain_while_active(&driver);
        schedule_drive(&driver);
    })
}

/// Cancel this driver's exact debug root. If an action Promise is waiting on a
/// timer/fetch, settle it immediately as cancelled; the runtime cancellation
/// command is still scheduled for the next turn so resources are reaped.
pub(crate) fn stop_debug(driver: &Rc<PromiseDriver>) -> bool {
    let session = driver.debug_session.borrow_mut().take();
    let Some(mut session) = session else {
        let Some(root) = driver.debug_root.get() else {
            return false;
        };
        driver.debug_stop_requested.set(Some(root));
        let _ = sema_vm::with_active_debug(|debug| {
            debug.pause_requested = true;
            debug.instructions_remaining = debug.instructions_remaining.clamp(1, 128);
        });
        let _ = driver.interp.command_handle().cancel_root(root);
        return true;
    };
    let root = session.handle.id();
    let action = session.action.take();
    let result = debug_cancelled_result(root, std::mem::take(&mut session.output));
    retire_debug_root(driver, session.handle);
    if let Some(action) = action {
        resolve_debug_immediately(&action.resolve, result);
    }
    true
}

/// Cancel a detached debugger session while preserving exact-root scheduling
/// ownership until the runtime publishes its terminal result.
fn retire_debug_root(driver: &Rc<PromiseDriver>, handle: RootHandle) {
    let root = handle.id();
    driver.debug_root.set(None);
    let runtime = driver.interp.runtime();
    let _ = driver.interp.command_handle().cancel_root(root);
    if runtime.is_debug_paused_for(root) {
        runtime.debug_cancel_paused_root(root);
    }
    driver
        .retiring_debug_roots
        .borrow_mut()
        .insert(root, handle);
    retain_while_active(driver);
    schedule_drive(driver);
}

pub(crate) fn debug_is_active(driver: &PromiseDriver) -> bool {
    driver.debug_root.get().is_some()
}

pub(crate) fn debug_action_is_driving(driver: &PromiseDriver) -> bool {
    driver.debug_root.get().is_some() && driver.debug_session.borrow().is_none()
}

pub(crate) fn debug_locals(driver: &PromiseDriver) -> JsValue {
    if debug_action_is_driving(driver) {
        return JsValue::NULL;
    }
    let Some(root) = driver.debug_root.get() else {
        return JsValue::NULL;
    };
    let locals = driver
        .interp
        .runtime()
        .with_paused_root_vm(root, |vm| {
            let frame = vm.frame_count().saturating_sub(1);
            vm.debug_locals(frame)
        })
        .unwrap_or_default();
    let array = js_sys::Array::new();
    for variable in locals {
        let object = js_sys::Object::new();
        set_property(&object, "name", JsValue::from_str(&variable.name));
        set_property(&object, "value", JsValue::from_str(&variable.value));
        set_property(&object, "type", JsValue::from_str(&variable.type_name));
        array.push(&object);
    }
    array.into()
}

pub(crate) fn debug_stack_trace(driver: &PromiseDriver) -> JsValue {
    if debug_action_is_driving(driver) {
        return js_sys::Array::new().into();
    }
    let Some(root) = driver.debug_root.get() else {
        return js_sys::Array::new().into();
    };
    let frames = driver
        .interp
        .runtime()
        .with_paused_root_vm(root, |vm| vm.debug_stack_trace())
        .unwrap_or_default();
    let array = js_sys::Array::new();
    for frame in frames {
        let object = js_sys::Object::new();
        set_property(&object, "name", JsValue::from_str(&frame.name));
        set_property(&object, "line", JsValue::from_f64(frame.line as f64));
        set_property(&object, "column", JsValue::from_f64(frame.column as f64));
        array.push(&object);
    }
    array.into()
}

pub(crate) fn set_debug_breakpoints(driver: &PromiseDriver, lines: &[u32]) -> bool {
    if debug_action_is_driving(driver) {
        *driver.debug_breakpoints_pending.borrow_mut() = Some(lines.to_vec());
        return true;
    }
    let mut slot = driver.debug_session.borrow_mut();
    let Some(session) = slot.as_mut() else {
        return false;
    };
    set_session_breakpoints(session, lines);
    true
}

fn set_session_breakpoints(session: &mut PromiseDebugSession, lines: &[u32]) {
    let source = std::path::PathBuf::from("<playground>");
    session.debug.set_breakpoints(&source, lines);
    session.breakpoints = lines.to_vec();
}

fn register_driver(driver: &Rc<PromiseDriver>, runtime_id: RuntimeId) {
    match driver.runtime_id.get() {
        Some(registered) => {
            debug_assert_eq!(registered, runtime_id);
        }
        None => driver.runtime_id.set(Some(runtime_id)),
    }
    DRIVER_REGISTRY.with(|registry| {
        registry
            .borrow_mut()
            .insert(runtime_id, Rc::downgrade(driver));
    });
}

fn unregister_driver(driver: &Rc<PromiseDriver>) {
    let Some(runtime_id) = driver.runtime_id.get() else {
        return;
    };
    let expected = Rc::downgrade(driver);
    DRIVER_REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        if registry
            .get(&runtime_id)
            .is_some_and(|registered| Weak::ptr_eq(registered, &expected))
        {
            registry.remove(&runtime_id);
        }
    });
}

fn schedule_runtime(runtime_id: RuntimeId) {
    let driver =
        DRIVER_REGISTRY.with(|registry| registry.borrow().get(&runtime_id).and_then(Weak::upgrade));
    if let Some(driver) = driver {
        schedule_drive(&driver);
    } else {
        DRIVER_REGISTRY.with(|registry| {
            registry.borrow_mut().remove(&runtime_id);
        });
    }
}

/// Queue exactly one macrotask (if none is already queued) that drives this
/// interpreter — ASAP, not honoring any pending `WaitKind::Timer`
/// deadline. Idempotent — safe to call from `submit`, a `WasmExecutor`
/// completion, or `cancel_root` without ever double-scheduling.
///
/// If a turn is currently scheduled only via `schedule_drive_after` (waiting
/// out a future timer deadline), this cancels that `setTimeout` and posts an
/// immediate macrotask instead: an external completion or an explicit cancel
/// command must be observed on the next possible turn, not delayed behind an
/// unrelated (or now-irrelevant) timer wait.
fn schedule_drive(driver: &Rc<PromiseDriver>) {
    if let Some(id) = driver.drive_timeout_id.take() {
        clear_timeout(id);
        driver.drive_scheduled.set(false);
    }
    if driver.drive_scheduled.replace(true) {
        return; // already scheduled (an immediate turn is already pending)
    }
    schedule_macrotask(Rc::clone(driver));
}

/// Like `schedule_drive`, but the macrotask fires after `delay_ms` — used to
/// honor a pending `WaitKind::Timer` deadline (`async/sleep`) instead of
/// busy-polling `drive_turn` until it fires. The scheduled `setTimeout` id is
/// retained on the driver so a later `schedule_drive` call (an external
/// completion or a cancel command) can preempt it.
fn schedule_drive_after(driver: &Rc<PromiseDriver>, delay_ms: i32) {
    if driver.drive_scheduled.replace(true) {
        return;
    }
    let id = schedule_timeout_tracked(Rc::clone(driver), delay_ms.max(0));
    driver.drive_timeout_id.set(id);
}

/// Post this driver's next turn as a genuine macrotask: a `MessageChannel`
/// round-trip when available (a true macrotask that yields to rendering/input,
/// per the design doc), falling back to `setTimeout(..., 0)` — both are
/// supported in a Window, a Worker, and Node.
fn schedule_macrotask(driver: Rc<PromiseDriver>) {
    if let Ok(channel) = web_sys::MessageChannel::new() {
        let port1 = channel.port1();
        let port2 = channel.port2();
        // Close `port1` once the message is delivered: on Node (the "nodejs"
        // wasm-pack target used by this crate's own smoke tests), an
        // unclosed `MessagePort` keeps the event loop alive indefinitely even
        // after its one message has fired — a real, observed hang. A browser
        // doesn't have that "keep process alive" concept, but closing the
        // port promptly is correct there too (no reason to leak it).
        let port1_for_close = port1.clone();
        let scheduled_driver = Rc::clone(&driver);
        let closure = Closure::once_into_js(move || {
            drive_and_settle(&scheduled_driver);
            port1_for_close.close();
        });
        port1.set_onmessage(Some(closure.unchecked_ref()));
        if port2.post_message(&JsValue::UNDEFINED).is_ok() {
            return;
        }
        port1.close();
    }
    schedule_timeout_tracked(driver, 0);
}

/// `setTimeout(f, delay_ms)`, returning the timer id (for `clearTimeout`) if
/// the host actually has a global `setTimeout` to call — `None` when it ran
/// `f` inline instead (no id to track; nothing to cancel).
fn schedule_timeout_tracked(driver: Rc<PromiseDriver>, delay_ms: i32) -> Option<i32> {
    let scheduled_driver = Rc::clone(&driver);
    let closure = Closure::once_into_js(move || drive_and_settle(&scheduled_driver));
    let target = js_sys::global();
    if let Ok(set_timeout) = js_sys::Reflect::get(&target, &JsValue::from_str("setTimeout")) {
        if let Some(set_timeout) = set_timeout.dyn_ref::<Function>() {
            if let Ok(id) = set_timeout.call2(
                &target,
                closure.unchecked_ref(),
                &JsValue::from_f64(delay_ms as f64),
            ) {
                return id.as_f64().map(|n| n as i32);
            }
        }
    }
    // No global `setTimeout` at all (an unexpected host): run inline rather
    // than dropping the drive request — same-turn re-entrancy is safe since
    // `drive_and_settle` only touches this driver's single-threaded state.
    drive_and_settle(&driver);
    None
}

/// `clearTimeout(id)` — best-effort; a no-op if the host has none (can't
/// happen if `id` came from a successful `schedule_timeout_tracked` call on
/// the same host, but this stays defensive rather than panicking).
fn clear_timeout(id: i32) {
    let target = js_sys::global();
    if let Ok(clear_timeout) = js_sys::Reflect::get(&target, &JsValue::from_str("clearTimeout")) {
        if let Some(clear_timeout) = clear_timeout.dyn_ref::<Function>() {
            let _ = clear_timeout.call1(&target, &JsValue::from_f64(id as f64));
        }
    }
}

/// The macrotask body: run one bounded, non-blocking `drive_turn`, deliver
/// output + settle any roots that finished, then either stop (idle, nothing
/// pending) or schedule the next turn — immediately if there is more ready
/// work, or after the next timer deadline if everything is waiting on one.
fn drive_and_settle(driver: &Rc<PromiseDriver>) {
    driver.drive_scheduled.set(false);
    // The timer that fired to reach this turn (if any) has already run; its
    // id is stale. Clearing it also lets a `schedule_drive_after` called
    // later in THIS turn (a fresh, shorter timer deadline) register its own
    // id cleanly rather than appearing to still have one pending.
    driver.drive_timeout_id.set(None);
    let interp = &driver.interp;
    let owned_roots = driver.owned_roots();

    let (drive_state, debug_stop_requested) = {
        let mut debug_drive = PromiseDebugDrive::begin(driver);
        if let Some(session) = debug_drive.session_mut() {
            session.debug.instructions_remaining = DEBUG_INSTRUCTION_BUDGET;
        }
        let debug_guard = debug_drive.session_mut().map(|session| {
            sema_vm::ActiveDebugGuard::enter_for_root(&mut session.debug, session.handle.id())
        });
        let drive_result = interp.drive_roots(&owned_roots);
        drop(debug_guard);
        let stop_requested = debug_drive.finish();
        match drive_result {
            Ok(state) => (state, stop_requested),
            Err(fault) => {
                pump_output(driver);
                fail_all_pending(driver, &format!("runtime fault: {fault:?}"));
                return;
            }
        }
    };

    pump_output(driver);
    if debug_stop_requested.is_some_and(|root| driver.debug_root.get() == Some(root)) {
        let _ = stop_debug(driver);
    }
    settle_ready_roots(driver);
    settle_debug_action(driver, &drive_state);
    settle_retiring_debug_roots(driver);

    let ordinary_pending = !driver.promises.borrow().is_empty();
    let retiring_debug_pending = !driver.retiring_debug_roots.borrow().is_empty();
    let (debug_active, debug_action_pending) = driver
        .debug_session
        .borrow()
        .as_ref()
        .map(|session| (true, session.action.is_some()))
        .unwrap_or((false, false));
    if !ordinary_pending && !debug_active && !retiring_debug_pending {
        unregister_driver(driver);
        driver.active_retain.borrow_mut().take();
        return; // idle with nothing left to settle — stop scheduling, no busy loop
    }
    // A stable cooperative stop intentionally leaves the session alive but has
    // no action Promise in flight. The debug barrier freezes this runtime until
    // an explicit continue/step call; scheduling here would busy-spin on the
    // same `DebugStopped` forever.
    if matches!(drive_state, DriveState::DebugStopped { .. })
        || (driver
            .debug_session
            .borrow()
            .as_ref()
            .is_some_and(|session| {
                driver
                    .interp
                    .runtime()
                    .is_debug_paused_for(session.handle.id())
            })
            && !debug_action_pending)
    {
        if !ordinary_pending && !retiring_debug_pending {
            // No Promise is awaiting progress while paused. Let the JS
            // interpreter wrapper own this stable session; retaining `self`
            // here would form a permanent cycle if that wrapper were dropped
            // without an explicit Stop.
            driver.active_retain.borrow_mut().take();
        }
        return;
    }
    match drive_state {
        DriveState::Progress { .. } => schedule_drive(driver),
        DriveState::DebugStopped { .. } => {}
        DriveState::Idle {
            next_deadline: Some(deadline),
            ..
        } => {
            let now = Instant::now();
            let delay_ms = deadline
                .checked_duration_since(now)
                .map(|d| d.as_millis().min(i32::MAX as u128) as i32)
                .unwrap_or(0);
            schedule_drive_after(driver, delay_ms);
        }
        // Idle with no timer deadline but an external wait (http fetch, …)
        // still outstanding: do NOT reschedule here. `WasmExecutor::submit`
        // calls `schedule_drive()` itself the instant the completion lands
        // (see its `spawn_local` body below) — that IS the wake. Rescheduling
        // unconditionally here (the P6-3 step 2 bug) would instead spin an
        // unbounded stream of no-op macrotask turns until the fetch resolves,
        // for no benefit (nothing here can make earlier progress than the
        // completion callback already guarantees).
        DriveState::Idle {
            next_deadline: None,
            inbox_wakeup_required: true,
        } => {}
        // Fully idle: no timer, no external wait, yet a promise is still
        // unsettled — an intra-runtime deadlock (e.g. a channel op with no
        // possible sender) the runtime cannot resolve on its own and nothing
        // will ever wake another turn. Reject rather than hang the returned
        // `Promise` forever.
        DriveState::Idle {
            next_deadline: None,
            inbox_wakeup_required: false,
        } => {
            fail_all_pending(
                driver,
                "runtime deadlocked: no pending timer or external wait, but a root has not settled",
            );
        }
        DriveState::Quiescent | DriveState::ShutdownComplete => {}
    }
}

fn pump_output(driver: &PromiseDriver) {
    let debug_root = driver
        .debug_session
        .borrow()
        .as_ref()
        .map(|session| session.handle.id());
    driver.output.pump(&driver.interp, debug_root, |captured| {
        if let Some(session) = driver.debug_session.borrow_mut().as_mut() {
            session.output.extend(captured);
        }
    });
}

/// Settle the Promise debugger's current action at a stable stop or terminal
/// root state. JS callbacks run only after the session `RefCell` borrow is
/// released, so a `.then(...)` callback may re-enter the interpreter safely.
fn settle_debug_action(driver: &PromiseDriver, drive_state: &DriveState) {
    let delivery = {
        let mut slot = driver.debug_session.borrow_mut();
        let Some(session) = slot.as_mut() else {
            return;
        };
        let poll = session.handle.poll_result();
        match poll {
            RootPoll::Ready(settlement) => {
                let root = session.handle.id();
                let output = std::mem::take(&mut session.output);
                let result = match &settlement.outcome {
                    sema_core::runtime::TaskOutcome::Returned(value) => {
                        debug_finished_result(root, value, output)
                    }
                    sema_core::runtime::TaskOutcome::Failed(error) => {
                        debug_failed_result(root, &format_debug_error(error), output)
                    }
                    sema_core::runtime::TaskOutcome::Cancelled(_) => {
                        debug_cancelled_result(root, output)
                    }
                };
                let action = session.action.take();
                let delivery = action.map(|action| {
                    let mut result = result;
                    if action.include_breakpoint_info {
                        attach_breakpoint_info(
                            &mut result,
                            &session.valid_lines,
                            &session.breakpoints,
                        );
                    }
                    (action.resolve, result)
                });
                *slot = None;
                delivery
            }
            RootPoll::Aborted(fault) => {
                let root = session.handle.id();
                let result = debug_failed_result(
                    root,
                    &format!("debug root aborted: {fault:?}"),
                    std::mem::take(&mut session.output),
                );
                let action = session.action.take();
                let delivery = action.map(|action| {
                    let mut result = result;
                    if action.include_breakpoint_info {
                        attach_breakpoint_info(
                            &mut result,
                            &session.valid_lines,
                            &session.breakpoints,
                        );
                    }
                    (action.resolve, result)
                });
                *slot = None;
                delivery
            }
            RootPoll::RuntimeDropped | RootPoll::InvariantViolation => {
                let root = session.handle.id();
                let result = debug_failed_result(
                    root,
                    "debug runtime invariant violation",
                    std::mem::take(&mut session.output),
                );
                let action = session.action.take();
                let delivery = action.map(|action| {
                    let mut result = result;
                    if action.include_breakpoint_info {
                        attach_breakpoint_info(
                            &mut result,
                            &session.valid_lines,
                            &session.breakpoints,
                        );
                    }
                    (action.resolve, result)
                });
                *slot = None;
                delivery
            }
            RootPoll::Pending => match drive_state {
                DriveState::DebugStopped { root, info, .. } if *root == session.handle.id() => {
                    let root = session.handle.id();
                    let action = session.action.take();
                    action.map(|action| {
                        let mut result =
                            debug_stopped_result(root, info, std::mem::take(&mut session.output));
                        if action.include_breakpoint_info {
                            attach_breakpoint_info(
                                &mut result,
                                &session.valid_lines,
                                &session.breakpoints,
                            );
                        }
                        (action.resolve, result)
                    })
                }
                _ => None,
            },
        }
    };
    if driver.debug_session.borrow().is_none() {
        driver.debug_root.set(None);
    }
    if let Some((resolve, result)) = delivery {
        resolve_debug_immediately(&resolve, result);
    }
}

/// Poll every pending root; settle (resolve/reject, remove from the table)
/// each one whose `poll_result()` is no longer `Pending`.
fn settle_ready_roots(driver: &PromiseDriver) {
    let ready: Vec<RootId> = driver
        .promises
        .borrow()
        .iter()
        .filter(|(_, entry)| !matches!(entry.handle.poll_result(), RootPoll::Pending))
        .map(|(&root, _)| root)
        .collect();
    for root in ready {
        let Some(entry) = driver.promises.borrow_mut().remove(&root) else {
            continue;
        };
        match entry.handle.poll_result() {
            RootPoll::Ready(settlement) => match &settlement.outcome {
                sema_core::runtime::TaskOutcome::Returned(value) => {
                    resolve_with_value(&entry.resolve, value);
                }
                sema_core::runtime::TaskOutcome::Failed(error) => {
                    reject_with_error(&entry.reject, error);
                }
                sema_core::runtime::TaskOutcome::Cancelled(reason) => {
                    reject_with_message(&entry.reject, &format!("cancelled: {reason:?}"));
                }
            },
            RootPoll::Aborted(fault) => {
                reject_with_message(&entry.reject, &format!("runtime fault: {fault:?}"));
            }
            RootPoll::RuntimeDropped => {
                reject_with_message(&entry.reject, "the interpreter's runtime was dropped");
            }
            RootPoll::InvariantViolation => {
                reject_with_message(&entry.reject, "internal error: runtime invariant violation");
            }
            RootPoll::Pending => unreachable!("filtered to non-pending above"),
        }
    }
}

/// Keep cancelled debugger roots in the driver's exact scheduling set until
/// their terminal result is observable. The action Promise is settled
/// separately; retaining only the handle lets cancellation and cleanup run
/// without exposing the retiring root as an active debugger session.
fn settle_retiring_debug_roots(driver: &PromiseDriver) {
    driver
        .retiring_debug_roots
        .borrow_mut()
        .retain(|_, handle| matches!(handle.poll_result(), RootPoll::Pending));
}

/// Reject every still-pending root the same way (a `Runtime::drive` fault is
/// not root-specific) and clear the table, so no promise is left hanging.
fn fail_all_pending(driver: &Rc<PromiseDriver>, message: &str) {
    let pending: Vec<PromiseSettlers> = driver
        .promises
        .borrow_mut()
        .drain()
        .map(|(_, value)| value)
        .collect();
    for entry in pending {
        reject_with_message(&entry.reject, message);
    }
    let debug_delivery = driver
        .debug_session
        .borrow_mut()
        .take()
        .and_then(|mut session| {
            let action = session.action.take()?;
            let result = debug_failed_result(
                session.handle.id(),
                message,
                std::mem::take(&mut session.output),
            );
            Some((action.resolve, result))
        });
    driver.debug_root.set(None);
    driver.retiring_debug_roots.borrow_mut().clear();
    if let Some((resolve, result)) = debug_delivery {
        resolve_debug_immediately(&resolve, result);
    }
    unregister_driver(driver);
    driver.active_retain.borrow_mut().take();
}

fn debug_stopped_result(root: RootId, info: &StopInfo, output: Vec<PromiseOutputEvent>) -> JsValue {
    let object = js_sys::Object::new();
    set_property(&object, "status", JsValue::from_str("stopped"));
    set_property(&object, "rootId", JsValue::from_f64(root.get() as f64));
    set_property(&object, "line", JsValue::from_f64(info.line as f64));
    let reason = match info.reason {
        StopReason::Breakpoint => "breakpoint",
        StopReason::Step => "step",
        StopReason::Pause => "pause",
        StopReason::Entry => "entry",
        StopReason::Exception => "exception",
    };
    set_property(&object, "reason", JsValue::from_str(reason));
    attach_debug_output(&object, output);
    object.into()
}

fn debug_finished_result(root: RootId, value: &Value, output: Vec<PromiseOutputEvent>) -> JsValue {
    let object = js_sys::Object::new();
    set_property(&object, "status", JsValue::from_str("finished"));
    set_property(&object, "rootId", JsValue::from_f64(root.get() as f64));
    let value = if value.is_nil() {
        JsValue::NULL
    } else {
        JsValue::from_str(&sema_core::pretty_print(value, 80))
    };
    set_property(&object, "value", value);
    attach_debug_output(&object, output);
    object.into()
}

fn debug_failed_result(root: RootId, message: &str, output: Vec<PromiseOutputEvent>) -> JsValue {
    let object = js_sys::Object::new();
    set_property(&object, "status", JsValue::from_str("error"));
    set_property(&object, "rootId", JsValue::from_f64(root.get() as f64));
    set_property(&object, "error", JsValue::from_str(message));
    attach_debug_output(&object, output);
    object.into()
}

fn debug_cancelled_result(root: RootId, output: Vec<PromiseOutputEvent>) -> JsValue {
    let object = js_sys::Object::new();
    set_property(&object, "status", JsValue::from_str("cancelled"));
    set_property(&object, "rootId", JsValue::from_f64(root.get() as f64));
    attach_debug_output(&object, output);
    object.into()
}

fn debug_error_result(message: &str) -> JsValue {
    let object = js_sys::Object::new();
    set_property(&object, "status", JsValue::from_str("error"));
    set_property(&object, "error", JsValue::from_str(message));
    set_property(&object, "output", js_sys::Array::new().into());
    object.into()
}

fn attach_breakpoint_info(result: &mut JsValue, valid_lines: &[u32], breakpoints: &[u32]) {
    let valid = numbers_to_array(valid_lines);
    let snapped = numbers_to_array(breakpoints);
    let _ = js_sys::Reflect::set(result, &JsValue::from_str("validLines"), &valid);
    let _ = js_sys::Reflect::set(result, &JsValue::from_str("breakpoints"), &snapped);
}

fn attach_debug_output(object: &js_sys::Object, output: Vec<PromiseOutputEvent>) {
    let texts = js_sys::Array::new();
    let events = js_sys::Array::new();
    for event in output {
        texts.push(&JsValue::from_str(&event.text));
        let item = js_sys::Object::new();
        set_property(&item, "stream", JsValue::from_str(event.stream));
        set_property(&item, "text", JsValue::from_str(&event.text));
        events.push(&item);
    }
    set_property(object, "output", texts.into());
    set_property(object, "outputEvents", events.into());
}

fn numbers_to_array(values: &[u32]) -> js_sys::Array {
    let array = js_sys::Array::new();
    for value in values {
        array.push(&JsValue::from_f64(*value as f64));
    }
    array
}

fn set_property(object: &js_sys::Object, key: &str, value: JsValue) {
    let _ = js_sys::Reflect::set(object, &JsValue::from_str(key), &value);
}

fn resolve_debug_immediately(resolve: &Function, result: JsValue) {
    let _ = resolve.call1(&JsValue::NULL, &result);
}

fn format_debug_error(error: &SemaError) -> String {
    let mut message = format!("{}", error.inner());
    if let Some(trace) = error.stack_trace() {
        message.push_str(&format!("\n{trace}"));
    }
    if let Some(hint) = error.hint() {
        message.push_str(&format!("\n  hint: {hint}"));
    }
    if let Some(note) = error.note() {
        message.push_str(&format!("\n  note: {note}"));
    }
    message
}

fn resolve_with_value(resolve: &Function, value: &Value) {
    let text = if value.is_nil() {
        JsValue::NULL
    } else {
        JsValue::from_str(&sema_core::pretty_print(value, 80))
    };
    let _ = resolve.call1(&JsValue::NULL, &text);
}

/// Builds the full formatted error text — inner message, stack trace, hint,
/// note, in that order — matching every OLD entry point's `{"error": "..."}`
/// formatting (`lib.rs`'s `eval_error_result`/`eval_async`/…). Baking all of
/// it into the rejected `Error`'s message (previously only the inner message
/// and hint, before P6-3 step 5) means an OLD entry point's promise-driven
/// wrapper can recover full fidelity from a plain `JsFuture` rejection
/// without a second, parallel error-detail channel.
fn reject_with_error(reject: &Function, error: &SemaError) {
    let mut message = format!("{}", error.inner());
    if let Some(trace) = error.stack_trace() {
        message.push_str(&format!("\n{trace}"));
    }
    if let Some(hint) = error.hint() {
        message.push_str(&format!("\n  hint: {hint}"));
    }
    if let Some(note) = error.note() {
        message.push_str(&format!("\n  note: {note}"));
    }
    reject_with_message(reject, &message);
}

fn reject_with_message(reject: &Function, message: &str) {
    let err = js_sys::Error::new(message);
    let _ = reject.call1(&JsValue::NULL, &err);
}

// ─────────────────────────────────────────────────────────────────────────
// WasmExecutor: the `IoExecutor` backing `evalPromise`'s runtime.
// ─────────────────────────────────────────────────────────────────────────

/// There is no OS thread pool in the browser, so a submitted external-wait
/// dispatch's `Send` future is polled to completion via
/// `wasm_bindgen_futures::spawn_local` — a microtask queued on the SAME (only)
/// wasm thread, not a real worker. This is sound because every job the
/// runtime-ABI natives below build is constructed from a `futures_channel`
/// oneshot `Receiver` and never touches a `JsValue` (`!Send`) itself; the
/// actual browser work (`fetch`, `setTimeout`) runs OUTSIDE that `Send`
/// boundary, in the native function's own body (see `runtime_http_call`),
/// which is free to touch `JsValue` because it isn't part of the job closure.
pub(crate) struct WasmExecutor;

struct WasmLease {
    runtime_id: RuntimeId,
}

impl IoExecutor for WasmExecutor {
    fn attach_runtime(
        &self,
        runtime_id: RuntimeId,
    ) -> Result<ExecutorLeaseArc, ExecutorAttachError> {
        Ok(std::sync::Arc::new(WasmLease { runtime_id }))
    }

    fn snapshot(&self) -> ExecutorSnapshot {
        ExecutorSnapshot::default()
    }
}

/// Type alias only to keep the `attach_runtime` signature under rustfmt's line
/// width — `Arc<dyn ExecutorLease>` is the real (and only) return type.
type ExecutorLeaseArc = std::sync::Arc<dyn ExecutorLease>;

impl ExecutorLease for WasmLease {
    fn submit(
        &self,
        submission: ExecutorSubmission,
    ) -> Result<RunningSubmission, SubmissionRejected> {
        let operation_id = submission.operation_id();
        match submission.into_dispatch() {
            ExecutorDispatch::Async(dispatch) => {
                let fut = dispatch.into_future();
                let runtime_id = self.runtime_id;
                wasm_bindgen_futures::spawn_local(async move {
                    let _report = fut.await; // self-delivers its completion via the sink
                    schedule_runtime(runtime_id);
                });
            }
            ExecutorDispatch::Blocking(dispatch) => {
                // No natives registered by this module build a `Blocking`
                // dispatch (there is no OS thread to block on wasm32); run it
                // inline so an unforeseen future caller still completes
                // rather than hanging silently.
                let _report = dispatch.run();
                schedule_runtime(self.runtime_id);
            }
        }
        Ok(RunningSubmission::new(operation_id))
    }

    fn snapshot(&self) -> ExecutorSnapshot {
        ExecutorSnapshot::default()
    }

    fn shutdown(&self, _deadline: Instant) -> ExecutorShutdown {
        ExecutorShutdown::Drained(ExecutorSnapshot::default())
    }
}

// ─────────────────────────────────────────────────────────────────────────
// http/{get,post,put,delete,request} — runtime-ABI variant (External wait)
// ─────────────────────────────────────────────────────────────────────────

/// Completion tag for the Promise-driven HTTP external wait. Distinct from
/// the native stdlib's `0x6874_7470` ("http") — this runtime never registers
/// that build's natives (sema-wasm doesn't depend on sema-stdlib's `http.rs`;
/// it registers its own wasm-flavored natives), so collision is impossible
/// either way, but a distinct tag keeps the two mechanisms visibly separate
/// in any shared diagnostics.
const WASM_HTTP_COMPLETION_KIND: u64 = 0x7773_6d68; // "wsmh"

/// The `Send`-safe facts of an HTTP response that cross from the `fetch()`
/// JS callback back to the VM thread. Never a `Value`/`Rc` — decoded into one
/// only on the VM thread (`WasmHttpDecoder`), matching the documented
/// JS-callback boundary rule (serialized/plain data only, no `Value`).
struct RawHttpResponse {
    status: i64,
    headers: Vec<(String, String)>,
    body: String,
}

/// The rejecting value-ABI closure a dual-ABI wasm HTTP native uses outside a
/// Promise-driven turn. Named to keep
/// `runtime_http_fn`'s signature (and `lib.rs`'s call site) under clippy's
/// `type_complexity` threshold.
pub(crate) type SyncHttpFn = Rc<dyn Fn(&[Value]) -> Result<Value, SemaError>>;

/// Register the runtime ABI onto an existing wasm HTTP `NativeFn`, turning it
/// into a dual-ABI native via
/// [`sema_core::NativeFn::simple_with_runtime`].
///
/// Runtime-quantum state alone cannot identify a host that can pump browser
/// callbacks. The currently executing root must belong to this interpreter's
/// Promise driver; otherwise `synchronous` rejects with an `evalPromise` hint.
pub(crate) fn runtime_http_fn(
    method: &'static str,
    synchronous: SyncHttpFn,
) -> impl for<'a> Fn(&mut NativeCallContext<'a>, &[Value]) -> NativeResult + 'static {
    move |_ctx, args| {
        if promise_driven_root_active() {
            runtime_http_call(method, args)
        } else {
            (synchronous)(args).map(NativeOutcome::Return)
        }
    }
}

fn runtime_http_call(default_method: &'static str, args: &[Value]) -> NativeResult {
    // Preserve the public calling conventions per verb: `http/get` and
    // `http/delete` take (url, opts?); `http/post`/`http/put` take (url, body,
    // opts?); `http/request` takes (method, url, body?, opts?).
    let (method, url, body, opts): (String, String, Option<&Value>, Option<&Value>) =
        if default_method == "REQUEST" {
            let method = args
                .first()
                .and_then(|v| v.as_str())
                .ok_or_else(|| SemaError::type_error("string", "nil"))?
                .to_string();
            let url = args
                .get(1)
                .and_then(|v| v.as_str())
                .ok_or_else(|| SemaError::type_error("string", "nil"))?
                .to_string();
            (method, url, args.get(2), args.get(3))
        } else if matches!(default_method, "POST" | "PUT") {
            let url = args
                .first()
                .and_then(|v| v.as_str())
                .ok_or_else(|| SemaError::type_error("string", "nil"))?
                .to_string();
            (default_method.to_string(), url, args.get(1), args.get(2))
        } else {
            let url = args
                .first()
                .and_then(|v| v.as_str())
                .ok_or_else(|| SemaError::type_error("string", "nil"))?
                .to_string();
            (default_method.to_string(), url, None, args.get(1))
        };

    let mut headers: Vec<(String, String)> = Vec::new();
    let mut timeout_ms: Option<u64> = None;
    let mut has_content_type = false;
    if let Some(opts_val) = opts {
        if let Some(opts_map) = opts_val.as_map_rc() {
            if let Some(headers_val) = opts_map.get(&Value::keyword("headers")) {
                if let Some(hmap) = headers_val.as_map_rc() {
                    for (k, v) in hmap.iter() {
                        let key = match k.view() {
                            ValueView::String(s) => s.to_string(),
                            ValueView::Keyword(s) => sema_core::resolve(s),
                            _ => k.to_string(),
                        };
                        let val = match v.as_str() {
                            Some(s) => s.to_string(),
                            None => v.to_string(),
                        };
                        if key.eq_ignore_ascii_case("content-type") {
                            has_content_type = true;
                        }
                        headers.push((key, val));
                    }
                }
            }
            if let Some(timeout_val) = opts_map.get(&Value::keyword("timeout")) {
                if let Some(ms) = timeout_val.as_int() {
                    timeout_ms = Some(ms.max(0) as u64);
                }
            }
        }
    }
    let body_str: Option<String> = match body {
        Some(val) if val.is_nil() => None,
        Some(val) => Some(match val.as_str() {
            Some(s) => s.to_string(),
            None if val.as_map_rc().is_some() => {
                let json = sema_core::value_to_json_lossy(val);
                if !has_content_type {
                    headers.push(("Content-Type".to_string(), "application/json".to_string()));
                }
                serde_json::to_string(&json)
                    .map_err(|e| SemaError::eval(format!("http: json encode: {e}")))?
            }
            None => val.to_string(),
        }),
        None => None,
    };

    let (tx, rx) = futures_channel::oneshot::channel::<Result<RawHttpResponse, String>>();
    let abort_controller = web_sys::AbortController::new().ok();
    let cancel_controller = abort_controller.clone();

    wasm_bindgen_futures::spawn_local(async move {
        let result = perform_fetch_raw(
            &method,
            &url,
            body_str.as_deref(),
            &headers,
            timeout_ms,
            abort_controller.as_ref(),
        )
        .await;
        let _ = tx.send(result);
    });

    let kind = CompletionKind::try_from_raw(WASM_HTTP_COMPLETION_KIND)
        .expect("fixed nonzero completion kind constant");
    let decoder = Box::new(WasmHttpDecoder);
    let continuation = Box::new(WasmHttpContinuation);
    let resource = InterruptibleResource::new(
        "http",
        Box::new(WasmHttpCancelHook {
            controller: cancel_controller,
        }),
    );
    let prepared = PreparedExternalOperation::interruptible_async(kind, decoder, resource, {
        move || async move {
            let result = rx
                .await
                .unwrap_or_else(|_| Err("internal: response channel dropped".to_string()));
            Ok(Box::new(result) as SendPayload)
        }
    });
    Ok(NativeOutcome::Suspend(NativeSuspend {
        wait: WaitKind::External(Box::new(prepared)),
        continuation,
    }))
}

/// The two hosts this crate's `fetch`/`setTimeout` calls can run under: a
/// page (`Window`) or the playground's eval Web Worker
/// (`WorkerGlobalScope` — covers `DedicatedWorkerGlobalScope` too, since it
/// extends `WorkerGlobalScope` and `JsCast::dyn_into` follows the JS
/// prototype chain). Both expose `fetch`/`setTimeout` with identical
/// signatures but as unrelated `web-sys` types with no shared trait for them
/// in this crate's `web-sys` feature set, hence this small dispatch enum
/// instead of duplicating `perform_fetch_raw` per host.
enum GlobalScope {
    Window(web_sys::Window),
    Worker(web_sys::WorkerGlobalScope),
}

impl GlobalScope {
    fn current() -> Result<Self, String> {
        if let Some(window) = web_sys::window() {
            return Ok(Self::Window(window));
        }
        if let Ok(worker) = js_sys::global().dyn_into::<web_sys::WorkerGlobalScope>() {
            return Ok(Self::Worker(worker));
        }
        Err("no global `window` or `WorkerGlobalScope` available".to_string())
    }

    fn fetch_with_request(&self, request: &web_sys::Request) -> js_sys::Promise {
        match self {
            Self::Window(w) => w.fetch_with_request(request),
            Self::Worker(w) => w.fetch_with_request(request),
        }
    }

    fn set_timeout(&self, closure: &Closure<dyn FnMut()>, delay_ms: i32) {
        let _ = match self {
            Self::Window(w) => w.set_timeout_with_callback_and_timeout_and_arguments_0(
                closure.as_ref().unchecked_ref(),
                delay_ms,
            ),
            Self::Worker(w) => w.set_timeout_with_callback_and_timeout_and_arguments_0(
                closure.as_ref().unchecked_ref(),
                delay_ms,
            ),
        };
    }
}

/// Perform the fetch entirely off the `Send` boundary (this whole function —
/// and everything it touches, `web_sys`/`JsValue` — runs on the wasm main
/// thread via `spawn_local`, never inside the job future the executor polls).
/// Returns only `Send`-safe data; building the `Value` happens later, on the
/// VM thread, in `WasmHttpDecoder`.
///
/// Works both on the main thread (`Window`) and inside the playground's eval
/// Web Worker (`WorkerGlobalScope`, no `window` global there at all) — the
/// worker protocol rewrite (P6-3 step 3) runs `evalPromise` from inside a
/// Worker, so an unconditional `web_sys::window()` would make every `http/*`
/// call fail there with "no global `window` available".
async fn perform_fetch_raw(
    method: &str,
    url: &str,
    body: Option<&str>,
    headers: &[(String, String)],
    timeout_ms: Option<u64>,
    abort_controller: Option<&web_sys::AbortController>,
) -> Result<RawHttpResponse, String> {
    let scope = GlobalScope::current()?;

    let opts = web_sys::RequestInit::new();
    opts.set_method(method);
    opts.set_mode(web_sys::RequestMode::Cors);
    if let Some(body_str) = body {
        opts.set_body(&JsValue::from_str(body_str));
    }
    if let Some(controller) = abort_controller {
        opts.set_signal(Some(&controller.signal()));
    }
    if let (Some(ms), Some(controller)) = (timeout_ms, abort_controller) {
        let c = controller.clone();
        let closure = Closure::wrap(Box::new(move || c.abort()) as Box<dyn FnMut()>);
        scope.set_timeout(&closure, ms.min(i32::MAX as u64) as i32);
        closure.forget();
    }

    let request = web_sys::Request::new_with_str_and_init(url, &opts)
        .map_err(|e| format!("failed to create request: {}", js_err(&e)))?;
    for (k, v) in headers {
        request
            .headers()
            .set(k, v)
            .map_err(|e| format!("failed to set header: {}", js_err(&e)))?;
    }

    let resp_jsvalue = JsFuture::from(scope.fetch_with_request(&request))
        .await
        .map_err(|e| js_err(&e))?;
    let response: web_sys::Response = resp_jsvalue
        .dyn_into()
        .map_err(|_| "fetch did not return a Response".to_string())?;

    let status = response.status() as i64;
    let mut resp_headers = Vec::new();
    if let Ok(Some(iter)) = js_sys::try_iter(&response.headers()) {
        for entry in iter.flatten() {
            let arr: js_sys::Array = entry.into();
            if arr.length() >= 2 {
                let k = arr.get(0).as_string().unwrap_or_default();
                let v = arr.get(1).as_string().unwrap_or_default();
                resp_headers.push((k, v));
            }
        }
    }

    let body_promise = response
        .text()
        .map_err(|e| format!("failed to read response body: {}", js_err(&e)))?;
    let body_jsvalue = JsFuture::from(body_promise)
        .await
        .map_err(|e| format!("failed to read response body: {}", js_err(&e)))?;
    let body_text = body_jsvalue.as_string().unwrap_or_default();

    Ok(RawHttpResponse {
        status,
        headers: resp_headers,
        body: body_text,
    })
}

fn js_err(e: &JsValue) -> String {
    e.as_string()
        .or_else(|| {
            js_sys::Reflect::get(e, &JsValue::from_str("message"))
                .ok()
                .and_then(|m| m.as_string())
        })
        .unwrap_or_else(|| "error".to_string())
}

/// Decodes the job's `Result<RawHttpResponse, String>` payload into the public
/// `{:status :headers :body}` map shape. Runs on the VM thread; holds no
/// `Value`/`Env` (only builds one), matching Invariant I2.
struct WasmHttpDecoder;

impl Trace for WasmHttpDecoder {
    fn trace(&self, _sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        true
    }
}

impl CompletionDecoder for WasmHttpDecoder {
    fn decode(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        result: Result<SendPayload, ExternalFailure>,
    ) -> Result<Value, SemaError> {
        let payload = match result {
            Ok(payload) => payload,
            Err(failure) => return Err(SemaError::eval(format!("http: {}", failure.message()))),
        };
        match downcast_send_payload::<Result<RawHttpResponse, String>>(payload, "http") {
            Ok(Ok(resp)) => {
                let mut headers = std::collections::BTreeMap::new();
                for (k, v) in resp.headers {
                    headers.insert(Value::keyword(&k), Value::string(&v));
                }
                let mut out = std::collections::BTreeMap::new();
                out.insert(Value::keyword("status"), Value::int(resp.status));
                out.insert(Value::keyword("headers"), Value::map(headers));
                out.insert(Value::keyword("body"), Value::string(&resp.body));
                Ok(Value::map(out))
            }
            Ok(Err(message)) => Err(SemaError::Io(message)),
            Err(failure) => Err(SemaError::eval(failure.message().to_string())),
        }
    }
}

/// Resumes the parked frame once the fetch settles: the decoded response is
/// injected at the call site, or the failure/cancellation is raised there
/// (catchable, same as every other suspending native). Holds no `Value`.
struct WasmHttpContinuation;

impl Trace for WasmHttpContinuation {
    fn trace(&self, _sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        true
    }
}

impl NativeContinuation for WasmHttpContinuation {
    fn resume(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        match input {
            ResumeInput::Returned(value) => Ok(NativeOutcome::Return(value)),
            ResumeInput::Failed(error) => Err(error),
            ResumeInput::Cancelled(reason) => Err(SemaError::eval(format!(
                "http request was cancelled ({reason:?})"
            ))),
            ResumeInput::Runtime(_) => Err(SemaError::eval(
                "http continuation received an unexpected runtime response",
            )),
        }
    }
}

/// Cancels an in-flight fetch by aborting its `AbortController` (if one was
/// created — always, today). Runs on the VM thread; holds no `Value`.
struct WasmHttpCancelHook {
    controller: Option<web_sys::AbortController>,
}

impl Trace for WasmHttpCancelHook {
    fn trace(&self, _sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        true
    }
}

impl sema_core::runtime::CancelHook for WasmHttpCancelHook {
    fn cancel(
        &mut self,
    ) -> Result<sema_core::runtime::CancelDisposition, sema_core::runtime::CancelHookError> {
        if let Some(controller) = self.controller.take() {
            controller.abort();
        }
        Ok(sema_core::runtime::CancelDisposition::Reaped)
    }
    fn reap(
        &mut self,
    ) -> Result<sema_core::runtime::CancelDisposition, sema_core::runtime::CancelHookError> {
        Ok(sema_core::runtime::CancelDisposition::Reaped)
    }
}

#[cfg(test)]
mod trace_tests {
    use super::*;

    fn edge_count(trace: &dyn Trace) -> usize {
        let mut count = 0;
        assert!(trace.trace(&mut |_| count += 1));
        count
    }

    /// CORE-2 GC invariant (I2) audit: none of the three resume-record types
    /// this module adds for the http external wait may carry a live
    /// `Value`/`Env` edge the collector can't see. `WasmHttpDecoder` and
    /// `WasmHttpContinuation` are unit structs (nothing to hold); the decoded
    /// `Value` `WasmHttpDecoder::decode` builds exists only as ITS RETURN
    /// VALUE, never stored in a field, so it needs no edge. `WasmHttpCancelHook`
    /// holds only an `Option<web_sys::AbortController>` — a `JsValue` wrapper,
    /// not a `Value`/`Env` — so it is edge-free regardless of whether a
    /// controller is present (constructed as `None` here: `AbortController`
    /// requires a JS host and isn't constructible in this native unit test).
    #[test]
    fn http_external_wait_records_are_edge_free() {
        assert_eq!(edge_count(&WasmHttpDecoder), 0);
        assert_eq!(edge_count(&WasmHttpContinuation), 0);
        assert_eq!(edge_count(&WasmHttpCancelHook { controller: None }), 0);
    }

    #[test]
    fn nested_promise_admission_reservations_release_independently() {
        let driver = PromiseDriver::new(Rc::new(Interpreter::new()));
        let outer = reserve_promise_admission(&driver).unwrap();
        let inner = reserve_promise_admission(&driver).unwrap();
        assert!(driver.blocks_legacy_debug_start());

        drop(inner);
        assert!(driver.blocks_legacy_debug_start());
        drop(outer);
        assert!(!driver.blocks_legacy_debug_start());
    }

    #[test]
    fn promise_admission_reservation_releases_while_unwinding() {
        let driver = PromiseDriver::new(Rc::new(Interpreter::new()));
        let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe({
            let driver = Rc::clone(&driver);
            move || {
                let _reservation = reserve_promise_admission(&driver).unwrap();
                assert!(driver.blocks_legacy_debug_start());
                panic!("exercise reservation cleanup");
            }
        }));

        assert!(unwind.is_err());
        assert!(!driver.blocks_legacy_debug_start());
    }
}
