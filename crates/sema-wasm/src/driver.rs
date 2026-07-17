//! Macrotask-driven Promise seam for the unified runtime (P6-3 step 2 —
//! `docs/plans/2026-07-16-wasm-promise-driven-roots.md`).
//!
//! This is a SECOND, additive way to evaluate Sema in the wasm build, exposed
//! as `WasmInterpreter::evalPromise`. It shares the same `sema_eval::Interpreter`
//! (and therefore the same global env / persistent runtime / detached tasks)
//! as every pre-existing entry point (`eval`, `evalAsync`, `evalVM`,
//! `evalVMAsync`, `runEntryAsync`, …), but drives it differently:
//!
//! * one call submits ONE root via `Interpreter::submit_str` and returns a
//!   `js_sys::Promise` immediately — the program body is never replayed;
//! * settlement is pumped by repeated, BOUNDED, NON-BLOCKING
//!   `Interpreter::drive_turn` calls scheduled across browser macrotasks
//!   (`schedule_drive`), never by blocking the calling thread. This is the
//!   part the shipped `eval*` entry points cannot do: their
//!   `drive_vm_on_runtime` → `drive_handle_to_settlement` path BLOCKS the
//!   calling thread on `WaitRuntime::block_on_inbox`, which internally calls
//!   `Receiver::recv_timeout` — unconditionally unsupported on
//!   wasm32-unknown-unknown (confirmed: it hits `Instant::now()` inside
//!   std's own `mpmc` channel and panics) the moment a program actually
//!   suspends on a timer or external wait. `evalPromise` is therefore also
//!   the FIRST wasm entry point that can correctly run `async/sleep` or a
//!   real `http/get` at all — not just a Promise-flavored return type.
//!
//! Old paths are untouched: they never call anything in this module, and
//! nothing here is reachable except through `evalPromise` (plus the
//! `WaitKind::External` runtime-ABI http natives below, which only activate
//! when a native is invoked FROM UNDER the runtime quantum — see
//! `NativeFn::simple_with_runtime` in `register_wasm_io`'s http registration).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

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
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;
use web_time::Instant;

use crate::output::pump_output;

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

thread_local! {
    static PROMISE_TABLE: RefCell<HashMap<RootId, PromiseSettlers>> = RefCell::new(HashMap::new());
    /// Coalescing flag: a macrotask is already queued to call `drive_and_settle`.
    /// Every wake source (`submit`, a settled `WaitKind::External` completion,
    /// a fired timer) calls `schedule_drive`, which is a no-op while this is
    /// set — so N wakes in the same turn still schedule exactly one macrotask.
    static DRIVE_SCHEDULED: Cell<bool> = const { Cell::new(false) };
    /// The interpreter the macrotask driver pumps. Bound by the first
    /// `evalPromise` submission and shared by the driver's macrotask
    /// callback, which runs detached from any JS call stack (so it cannot
    /// simply borrow `self` from a `WasmInterpreter` method).
    static DRIVE_INTERPRETER: RefCell<Option<Rc<Interpreter>>> = const { RefCell::new(None) };
}

/// Submit `src` as a fresh root on `interp`'s persistent runtime with
/// `capture_output: true`, stash `resolve`/`reject`, and kick the macrotask
/// driver. Called by `WasmInterpreter::evalPromise`, which owns the
/// `js_sys::Promise` executor these callbacks come from.
pub(crate) fn submit(interp: &Rc<Interpreter>, src: &str, resolve: Function, reject: Function) {
    DRIVE_INTERPRETER.with(|slot| {
        *slot.borrow_mut() = Some(Rc::clone(interp));
    });
    let opts = RootOptions {
        name: None,
        capture_output: true,
    };
    match interp.submit_str(src, opts) {
        Ok(handle) => {
            let root = handle.id();
            PROMISE_TABLE.with(|t| {
                t.borrow_mut().insert(
                    root,
                    PromiseSettlers {
                        handle,
                        resolve,
                        reject,
                    },
                );
            });
            schedule_drive();
        }
        Err(err) => {
            reject_with_error(&reject, &err);
        }
    }
}

/// Queue exactly one macrotask (if none is already queued) that drives the
/// bound interpreter. Idempotent — safe to call from `submit`, a
/// `WasmExecutor` completion, or a fired timer without ever double-scheduling.
pub(crate) fn schedule_drive() {
    if DRIVE_SCHEDULED.with(|f| f.replace(true)) {
        return; // already scheduled
    }
    schedule_macrotask(drive_and_settle);
}

/// Like `schedule_drive`, but the macrotask fires after `delay_ms` — used to
/// honor a pending `WaitKind::Timer` deadline (`async/sleep`) instead of
/// busy-polling `drive_turn` until it fires.
fn schedule_drive_after(delay_ms: i32) {
    if DRIVE_SCHEDULED.with(|f| f.replace(true)) {
        return;
    }
    schedule_timeout(drive_and_settle, delay_ms.max(0));
}

/// Post `f` as a genuine macrotask: a `MessageChannel` round-trip when
/// available (a true macrotask that yields to rendering/input, per the
/// design doc), falling back to `setTimeout(f, 0)` — both are supported in a
/// Window, a Worker, and Node.
fn schedule_macrotask(f: fn()) {
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
        let closure = Closure::once_into_js(move || {
            f();
            port1_for_close.close();
        });
        port1.set_onmessage(Some(closure.unchecked_ref()));
        if port2.post_message(&JsValue::UNDEFINED).is_ok() {
            return;
        }
        port1.close();
    }
    schedule_timeout(f, 0);
}

fn schedule_timeout(f: fn(), delay_ms: i32) {
    let closure = Closure::once_into_js(f);
    let target = js_sys::global();
    if let Ok(set_timeout) = js_sys::Reflect::get(&target, &JsValue::from_str("setTimeout")) {
        if let Some(set_timeout) = set_timeout.dyn_ref::<Function>() {
            let _ = set_timeout.call2(
                &target,
                closure.unchecked_ref(),
                &JsValue::from_f64(delay_ms as f64),
            );
            return;
        }
    }
    // No global `setTimeout` at all (an unexpected host): run inline rather
    // than dropping the drive request — same-turn re-entrancy is safe since
    // `drive_and_settle` only touches thread-local state.
    f();
}

/// The macrotask body: run one bounded, non-blocking `drive_turn`, deliver
/// output + settle any roots that finished, then either stop (idle, nothing
/// pending) or schedule the next turn — immediately if there is more ready
/// work, or after the next timer deadline if everything is waiting on one.
fn drive_and_settle() {
    DRIVE_SCHEDULED.with(|f| f.set(false));
    let Some(interp) = DRIVE_INTERPRETER.with(|slot| slot.borrow().clone()) else {
        return;
    };

    let next_deadline = match interp.drive_turn() {
        Ok(DriveState::Idle {
            next_deadline,
            inbox_wakeup_required: _,
        }) => next_deadline,
        Ok(_) => None, // Progress / Quiescent / ShutdownComplete / DebugStopped
        Err(fault) => {
            pump_output(&interp);
            fail_all_pending(&format!("runtime fault: {fault:?}"));
            return;
        }
    };

    pump_output(&interp);
    settle_ready_roots();

    let pending = PROMISE_TABLE.with(|t| !t.borrow().is_empty());
    if !pending {
        return; // idle with nothing left to settle — stop scheduling, no busy loop
    }
    match next_deadline {
        Some(deadline) => {
            let now = Instant::now();
            let delay_ms = deadline
                .checked_duration_since(now)
                .map(|d| d.as_millis().min(i32::MAX as u128) as i32)
                .unwrap_or(0);
            schedule_drive_after(delay_ms);
        }
        // Either more work is ready right now, or we are only waiting on an
        // external completion (http) — in that case `WasmExecutor` calls
        // `schedule_drive` itself when the completion lands, so no
        // unconditional reschedule here would just busy-loop for nothing.
        // `Progress`/ready-remaining is the one case that DOES need an
        // immediate reschedule; distinguishing them isn't observable from
        // `DriveState` alone once idle, so re-check via another drive_turn
        // next macrotask is the conservative, still-non-busy choice: a
        // genuinely idle-on-external-completion turn returns `Idle` again
        // immediately and stops (see below) rather than spin.
        None => schedule_drive(),
    }
}

/// Poll every pending root; settle (resolve/reject, remove from the table)
/// each one whose `poll_result()` is no longer `Pending`.
fn settle_ready_roots() {
    let ready: Vec<RootId> = PROMISE_TABLE.with(|t| {
        t.borrow()
            .iter()
            .filter(|(_, entry)| !matches!(entry.handle.poll_result(), RootPoll::Pending))
            .map(|(&root, _)| root)
            .collect()
    });
    for root in ready {
        let Some(entry) = PROMISE_TABLE.with(|t| t.borrow_mut().remove(&root)) else {
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

/// Reject every still-pending root the same way (a `Runtime::drive` fault is
/// not root-specific) and clear the table, so no promise is left hanging.
fn fail_all_pending(message: &str) {
    let pending: Vec<PromiseSettlers> =
        PROMISE_TABLE.with(|t| t.borrow_mut().drain().map(|(_, v)| v).collect());
    for entry in pending {
        reject_with_message(&entry.reject, message);
    }
}

fn resolve_with_value(resolve: &Function, value: &Value) {
    let text = if value.is_nil() {
        JsValue::NULL
    } else {
        JsValue::from_str(&sema_core::pretty_print(value, 80))
    };
    let _ = resolve.call1(&JsValue::NULL, &text);
}

fn reject_with_error(reject: &Function, error: &SemaError) {
    let mut message = format!("{}", error.inner());
    if let Some(hint) = error.hint() {
        message.push_str(&format!("\n  hint: {hint}"));
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

struct WasmLease;

impl IoExecutor for WasmExecutor {
    fn attach_runtime(
        &self,
        _runtime_id: RuntimeId,
    ) -> Result<ExecutorLeaseArc, ExecutorAttachError> {
        Ok(std::sync::Arc::new(WasmLease))
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
                wasm_bindgen_futures::spawn_local(async move {
                    let _report = fut.await; // self-delivers its completion via the sink
                    schedule_drive();
                });
            }
            ExecutorDispatch::Blocking(dispatch) => {
                // No natives registered by this module build a `Blocking`
                // dispatch (there is no OS thread to block on wasm32); run it
                // inline so an unforeseen future caller still completes
                // rather than hanging silently.
                let _report = dispatch.run();
                schedule_drive();
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

/// Register the runtime ABI onto an existing wasm http `NativeFn` built with
/// [`sema_core::NativeFn::simple`] (the legacy marker-throw closure), turning
/// it into a dual-ABI native via
/// [`sema_core::NativeFn::simple_with_runtime`]. The legacy closure keeps
/// running unmodified for every pre-existing entry point (none of which
/// drives a runtime quantum); the new runtime closure activates ONLY when
/// this native is invoked from inside `Runtime::drive` (i.e. only reachable
/// through `evalPromise`), where it suspends structurally on a real
/// `WaitKind::External` fetch instead of throwing a marker.
pub(crate) fn runtime_http_fn(
    method: &'static str,
) -> impl for<'a> Fn(&mut NativeCallContext<'a>, &[Value]) -> NativeResult + 'static {
    move |_ctx, args| runtime_http_call(method, args)
}

fn runtime_http_call(default_method: &'static str, args: &[Value]) -> NativeResult {
    // Mirrors `wasm_http_request`'s calling conventions per verb: `http/get`
    // and `http/delete` take (url, opts?); `http/post`/`http/put` take (url,
    // body, opts?); `http/request` takes (method, url, body?, opts?).
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
        schedule_drive();
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

/// Perform the fetch entirely off the `Send` boundary (this whole function —
/// and everything it touches, `web_sys`/`JsValue` — runs on the wasm main
/// thread via `spawn_local`, never inside the job future the executor polls).
/// Returns only `Send`-safe data; building the `Value` happens later, on the
/// VM thread, in `WasmHttpDecoder`.
async fn perform_fetch_raw(
    method: &str,
    url: &str,
    body: Option<&str>,
    headers: &[(String, String)],
    timeout_ms: Option<u64>,
    abort_controller: Option<&web_sys::AbortController>,
) -> Result<RawHttpResponse, String> {
    let window = web_sys::window().ok_or_else(|| "no global `window` available".to_string())?;

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
        let closure = Closure::once(move || c.abort());
        let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(
            closure.as_ref().unchecked_ref(),
            ms.min(i32::MAX as u64) as i32,
        );
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

    let resp_jsvalue = JsFuture::from(window.fetch_with_request(&request))
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

/// Decodes the job's `Result<RawHttpResponse, String>` payload into the same
/// `{:status :headers :body}` map shape `wasm_http_request`'s legacy path
/// (via the JS marker replay) produces. Runs on the VM thread; holds no
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
}
