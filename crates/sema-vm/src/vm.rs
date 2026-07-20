use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::num::NonZeroUsize;
use std::rc::{Rc, Weak};

use smallvec::SmallVec;
use web_time::Instant;

use sema_core::runtime::{
    multimethod_call, CancellationView, NativeCallContext, NativeContinuation, NativeOutcome,
    NativeResult, ResumeInput, RootId, Trace,
};
use sema_core::{
    bits_to_spur,
    error::{suggest_similar, veteran_hint, CallFrame as CoreCallFrame, StackTrace},
    number::SemaNumber,
    resolve as resolve_spur, Env, EnvBindings, EvalContext, GcEdge, NativeFn, NodePtr,
    OpaqueTraceFn, SemaError, Spur, Value, ValueViewRef, NAN_INT_SMALL_PATTERN, NAN_PAYLOAD_BITS,
    NAN_PAYLOAD_MASK, NAN_TAG_MASK, TAG_NATIVE_FN,
};

use crate::chunk::Function;
use crate::debug::VmPendingOutcome;
use crate::opcodes::op;
use crate::opcodes::Op;
use crate::restricted::{run_program_restricted, RestrictedRunPolicy};

/// Result of dispatching a native through the runtime ABI at a VM call site.
enum NativeDispatchResult {
    /// The native produced an immediate value; push it and continue.
    Value(Value),
    /// A runtime-ABI native returned a structural, non-`Return` outcome; park the
    /// frame and surface it as [`VmExecResult::Pending`].
    Pending(VmPendingOutcome),
}

/// A parked-frame signal produced by a helper-mediated native dispatch
/// (`call_value`/`call_value_with`/`call_native_with`) and consumed by the owning
/// opcode arm, which parks the frame and returns the matching [`VmExecResult`].
/// VM-scoped state, not a thread-local: it never outlives the single opcode that
/// set it.
enum VmNativeSignal {
    Pending(VmPendingOutcome),
}

impl VmNativeSignal {
    fn into_exec_result(self) -> crate::debug::VmExecResult {
        match self {
            Self::Pending(outcome) => crate::debug::VmExecResult::Pending(outcome),
        }
    }
}

const DEBUG_VALUE_REF_BASE: u64 = crate::debug::DEBUG_VALUE_REF_BASE;
const DEBUG_EVALUATION_INSTRUCTION_LIMIT: usize = 100_000;
const DEBUG_EVALUATION_TRANSITION_LIMIT: usize = 10_000;
const DEBUG_EVALUATION_SNAPSHOT_NODE_LIMIT: usize = 100_000;

/// Outcome of [`VM::handle_debug_stop`]: whether the caller should resume
/// execution (step mode already set per the resume command) or terminate the
/// debug session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DebugStopResume {
    /// Continue running (Continue/Step*/closed-channel).
    Resume,
    /// The frontend disconnected; stop the program.
    Disconnect,
}

/// State of a captured variable (upvalue).
#[derive(Debug)]
pub enum UpvalueState {
    /// Points into the VM stack while the defining frame is alive.
    Open { frame_base: usize, slot: usize },
    /// Owns the value after the defining frame has exited.
    Closed(Value),
    /// Detached from a foreign VM's stack — the cell owns its `value`, so it is
    /// safe to read/write on ANY VM stack (C1: it no longer indexes the owning
    /// VM's stack) — yet the defining frame is STILL LIVE and still tracks this
    /// slot in its `open_upvalues`. A closure that escapes onto a foreign stack
    /// (an `async/spawn` task VM, a fresh fallback VM, an inline-task HOF)
    /// transitions its still-`Open` cells here instead of fully `Closed`ing them,
    /// so the defining frame's later `StoreLocal`/`StoreUpvalue` writes continue
    /// to flow into `value` (see the STORE_LOCAL / STORE_UPVALUE dispatch arms).
    /// This preserves capture-by-cell semantics across the spawn boundary. The
    /// frame promotes it to a real `Closed` (with the final `value`) when it
    /// exits (`close_open_upvalues`). `frame_base`/`slot` are retained for
    /// symmetry with `Open` and to identify the tracking frame slot.
    Tracked {
        frame_base: usize,
        slot: usize,
        value: Value,
    },
}

/// A mutable cell for captured variables (upvalues).
#[derive(Debug)]
pub struct UpvalueCell {
    pub state: RefCell<UpvalueState>,
}

impl UpvalueCell {
    pub fn new_closed(value: Value) -> Self {
        UpvalueCell {
            state: RefCell::new(UpvalueState::Closed(value)),
        }
    }

    pub fn new_open(frame_base: usize, slot: usize) -> Self {
        UpvalueCell {
            state: RefCell::new(UpvalueState::Open { frame_base, slot }),
        }
    }
}

/// A runtime closure: function template + captured upvalues.
#[derive(Debug, Clone)]
pub struct Closure {
    pub func: Rc<Function>,
    pub upvalues: Vec<Rc<UpvalueCell>>,
    /// Home globals env: the global environment in which this closure was
    /// *defined*. `GetGlobal`/`SetGlobal`/`DefineGlobal` resolve against this
    /// env (not the executing VM's `self.globals`) so a closure exported from
    /// one module and run inside another module's VM still sees its own
    /// module-level defines. `None` for the top-level "main" closure, which is
    /// always run by the VM that owns its globals — it falls back to
    /// `self.globals` at execution time. Closures built via `MakeClosure`
    /// always carry a concrete `Some(home)` so they remain correct when
    /// exported across VMs (M1: closure home-globals).
    pub globals: Option<Rc<Env>>,
    /// Home function table: the compilation unit (`Vec<Function>`) this closure's
    /// `func`/upvalue indices and `MakeClosure`/`Call` func-ids point into. The
    /// executing VM sets `self.functions` to this on every frame activation, so
    /// an imported closure (whose table differs from the importer's) resolves
    /// its own functions even when called from the importer's VM — and the
    /// importer's frames restore *their* table on return. `None` for the
    /// top-level main closure, which uses the VM's own (base) table (M4: import
    /// on the VM).
    pub functions: Option<Rc<Vec<Rc<Function>>>>,
}

/// Payload stored in NativeFn for VM closures.
/// Carries both the closure and the function table from its compilation context.
struct VmClosurePayload {
    closure: Rc<Closure>,
    functions: Rc<Vec<Rc<Function>>>,
    native_fns: Rc<Vec<Rc<NativeFn>>>,
}

/// A decoded global binding held in an inline-cache slot. `LoadGlobal` slots
/// store the plain value; `CallGlobal` slots pre-decode the callee once at
/// fill time so every subsequent hit dispatches with no `Value` clone and no
/// payload downcast. Each variant keeps the bound `Value` reachable so a slot
/// satisfies either opcode (foreign function tables can alias cache indices;
/// the spur+version guard, not the variant, is the correctness gate).
#[derive(Clone)]
enum CachedGlobal {
    /// The bound value as-is: `LoadGlobal` slots, and `CallGlobal` slots whose
    /// callee is not a native fn (keywords, callables routed via `call_callback`).
    Plain(Value),
    /// A VM closure: the `NativeFn`'s `VmClosurePayload` pre-extracted. A hit
    /// re-checks `functions` against the VM's current table by pointer,
    /// exactly as the uncached decode does (the table swaps per frame).
    VmClosure {
        value: Value,
        closure: Rc<Closure>,
        functions: Rc<Vec<Rc<Function>>>,
    },
    /// A native fn with no VM-closure payload: called without re-extracting
    /// the `Rc` from the `Value`.
    Native { value: Value, func: Rc<NativeFn> },
}

impl CachedGlobal {
    /// Decode a callee for a `CallGlobal` slot.
    fn decode(val: Value) -> CachedGlobal {
        if val.raw_tag() == Some(TAG_NATIVE_FN) {
            let payload = {
                let native = val.as_native_fn_ref().unwrap();
                native
                    .payload
                    .as_ref()
                    .and_then(|p| p.downcast_ref::<VmClosurePayload>())
                    .map(|vmc| (vmc.closure.clone(), vmc.functions.clone()))
            };
            if let Some((closure, functions)) = payload {
                return CachedGlobal::VmClosure {
                    value: val,
                    closure,
                    functions,
                };
            }
            let func = val.as_native_fn_rc().unwrap();
            return CachedGlobal::Native { value: val, func };
        }
        CachedGlobal::Plain(val)
    }

    /// The bound value, regardless of how it was decoded.
    fn value(&self) -> &Value {
        match self {
            CachedGlobal::Plain(v)
            | CachedGlobal::VmClosure { value: v, .. }
            | CachedGlobal::Native { value: v, .. } => v,
        }
    }
}

// ── Cycle-collector wiring (CORE-2, plan §3/§5.2) ────────────────────────
//
// sema-core's collector cannot see through `VmClosurePayload` or
// `UpvalueCell` (sema-vm types), so sema-vm registers a payload tracer that
// reports every heap edge the closure wrapper owns, with exact multiplicity
// (trial deletion is arithmetic on these: an undercount leaks, an overcount
// frees live data). Opaque trace/sever fns recover `&T` from the node's data
// pointer — the collector keeps every traced allocation alive for the whole
// pass, so the casts are sound.

thread_local! {
    /// Once-guard: the payload tracer is registered (per thread) at VM
    /// construction — every `make_closure` (the sole producer of collector
    /// candidates) runs inside a VM, so wiring there keeps this check off
    /// the closure-creation hot path.
    static CYCLE_GC_WIRED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn ensure_cycle_gc_wired() {
    CYCLE_GC_WIRED.with(|wired| {
        if !wired.get() {
            wired.set(true);
            sema_core::register_payload_tracer(
                std::any::TypeId::of::<VmClosurePayload>(),
                vm_closure_payload_tracer,
            );
        }
    });
}

/// Edges of the whole closure-wrapper `NativeFn`: it holds exactly two strong
/// refs to the payload allocation — the `payload` field and the fallback
/// box's capture (`make_closure`'s invariant-I2 simplification) — so the
/// payload edge is reported twice.
fn vm_closure_payload_tracer(
    payload: &Rc<dyn std::any::Any>,
    sink: &mut dyn FnMut(sema_core::GcEdge),
) -> bool {
    for _ in 0..2 {
        sink(sema_core::GcEdge::Opaque {
            ptr: sema_core::NodePtr::of_rc(payload),
            strong_count: Rc::strong_count(payload),
            trace: trace_vm_closure_payload,
            sever: sever_nothing,
        });
    }
    true
}

/// Edges of a `VmClosurePayload`: one strong ref to the closure, one to the
/// home function table.
fn trace_vm_closure_payload(
    ptr: sema_core::NodePtr,
    sink: &mut dyn FnMut(sema_core::GcEdge),
) -> bool {
    // SAFETY: `ptr` is the data pointer of a live `Rc<VmClosurePayload>` —
    // the collector holds every traced allocation alive for the duration of
    // the pass (snapshot + side-map handles + deferred drops).
    let payload = unsafe { &*(ptr.raw() as *const VmClosurePayload) };
    sink(sema_core::GcEdge::Opaque {
        ptr: sema_core::NodePtr::of_rc(&payload.closure),
        strong_count: Rc::strong_count(&payload.closure),
        trace: trace_vm_closure,
        sever: sever_nothing,
    });
    sink(function_table_edge(&payload.functions));
    sink(native_table_edge(&payload.native_fns));
    true
}

/// Edges of a `Closure`: its `Function` template, one per upvalue-cell slot,
/// the home-globals env wrapper, and the home function table.
fn trace_vm_closure(ptr: sema_core::NodePtr, sink: &mut dyn FnMut(sema_core::GcEdge)) -> bool {
    // SAFETY: live `Rc<Closure>` data pointer — see trace_vm_closure_payload.
    let closure = unsafe { &*(ptr.raw() as *const Closure) };
    sink(function_edge(&closure.func));
    for cell in &closure.upvalues {
        sink(sema_core::GcEdge::Opaque {
            ptr: sema_core::NodePtr::of_rc(cell),
            strong_count: Rc::strong_count(cell),
            trace: trace_upvalue_cell,
            sever: sever_upvalue_cell,
        });
    }
    if let Some(globals) = &closure.globals {
        sink(sema_core::GcEdge::Env(globals));
    }
    if let Some(functions) = &closure.functions {
        sink(function_table_edge(functions));
    }
    true
}

fn vm_closure_edge(closure: &Rc<Closure>) -> sema_core::GcEdge<'static> {
    sema_core::GcEdge::Opaque {
        ptr: sema_core::NodePtr::of_rc(closure),
        strong_count: Rc::strong_count(closure),
        trace: trace_vm_closure,
        sever: sever_nothing,
    }
}

/// Edge into a compilation unit's function table. Tables and `Function`
/// templates are interior pass-through nodes (immutable, no severable cell)
/// but NOT leaves: chunk consts are usually reader literals, yet macro
/// expansion can compile any live value — including a closure — into them
/// (`lower_expr`'s catch-all lowers a non-list expansion result to
/// `CoreExpr::Const`). Left untraced, that consts edge would be a phantom
/// external count pinning the closure's entire env graph past teardown.
fn function_table_edge(table: &Rc<Vec<Rc<Function>>>) -> sema_core::GcEdge<'static> {
    sema_core::GcEdge::Opaque {
        ptr: sema_core::NodePtr::of_rc(table),
        strong_count: Rc::strong_count(table),
        trace: trace_function_table,
        sever: sever_nothing,
    }
}

/// Edge into one `Function` template (see [`function_table_edge`]).
fn function_edge(func: &Rc<Function>) -> sema_core::GcEdge<'static> {
    sema_core::GcEdge::Opaque {
        ptr: sema_core::NodePtr::of_rc(func),
        strong_count: Rc::strong_count(func),
        trace: trace_function,
        sever: sever_nothing,
    }
}

/// Edges of a function table: one strong ref per `Function` template.
fn trace_function_table(ptr: sema_core::NodePtr, sink: &mut dyn FnMut(sema_core::GcEdge)) -> bool {
    // SAFETY: live `Rc<Vec<Rc<Function>>>` data pointer — see
    // trace_vm_closure_payload.
    let table = unsafe { &*(ptr.raw() as *const Vec<Rc<Function>>) };
    for func in table {
        sink(function_edge(func));
    }
    true
}

/// Edge into the immutable native-function table shared by a VM and every
/// closure payload it creates. Owners report the table allocation itself;
/// the table reports each contained `NativeFn` exactly once.
fn native_table_edge(table: &Rc<Vec<Rc<NativeFn>>>) -> sema_core::GcEdge<'static> {
    sema_core::GcEdge::Opaque {
        ptr: sema_core::NodePtr::of_rc(table),
        strong_count: Rc::strong_count(table),
        trace: trace_native_table,
        sever: sever_nothing,
    }
}

fn trace_native_table(ptr: sema_core::NodePtr, sink: &mut dyn FnMut(sema_core::GcEdge)) -> bool {
    // SAFETY: live `Rc<Vec<Rc<NativeFn>>>` data pointer — see
    // trace_vm_closure_payload.
    let table = unsafe { &*(ptr.raw() as *const Vec<Rc<NativeFn>>) };
    for native in table {
        let value = Value::native_fn_from_rc(Rc::clone(native));
        sink(sema_core::GcEdge::Value(&value));
    }
    true
}

/// Edges of a `Function` template: one strong ref per chunk const.
fn trace_function(ptr: sema_core::NodePtr, sink: &mut dyn FnMut(sema_core::GcEdge)) -> bool {
    // SAFETY: live `Rc<Function>` data pointer — see trace_vm_closure_payload.
    let func = unsafe { &*(ptr.raw() as *const Function) };
    for c in &func.chunk.consts {
        sink(sema_core::GcEdge::Value(c));
    }
    true
}

/// Edges of an `UpvalueCell`: `Closed` owns one `Value`; `Open` owns nothing
/// (it points into a VM stack, whose slot is an external strong count that
/// keeps the target black — and the owning frame's `open_upvalues` ref keeps
/// the cell itself black, so an open cell can never be severed).
fn trace_upvalue_cell(ptr: sema_core::NodePtr, sink: &mut dyn FnMut(sema_core::GcEdge)) -> bool {
    // SAFETY: live `Rc<UpvalueCell>` data pointer — see trace_vm_closure_payload.
    let cell = unsafe { &*(ptr.raw() as *const UpvalueCell) };
    match cell.state.try_borrow() {
        Ok(state) => {
            // `Closed` and `Tracked` both OWN a `Value` reachable only through
            // the cell — trace it so the collector keeps it alive. `Open` owns
            // nothing (its slot lives on a VM stack, an external strong count).
            match &*state {
                UpvalueState::Closed(v) => sink(sema_core::GcEdge::Value(v)),
                UpvalueState::Tracked { value, .. } => sink(sema_core::GcEdge::Value(value)),
                UpvalueState::Open { .. } => {}
            }
            true
        }
        Err(_) => false,
    }
}

/// Sever a white upvalue cell: a value-owning state (`Closed(v)`/`Tracked{value:v}`)
/// → `Closed(NIL)`, returning `v` so the collector defers its drop until all
/// severing has completed. `Open` owns nothing, so it yields `None`.
fn sever_upvalue_cell(ptr: sema_core::NodePtr) -> Option<Value> {
    // SAFETY: live `Rc<UpvalueCell>` data pointer — see trace_vm_closure_payload.
    let cell = unsafe { &*(ptr.raw() as *const UpvalueCell) };
    match cell.state.try_borrow_mut() {
        Ok(mut state) => match std::mem::replace(&mut *state, UpvalueState::Closed(Value::NIL)) {
            UpvalueState::Closed(v) => Some(v),
            // A `Tracked` cell owns its value like `Closed`; hand it to the
            // collector for deferred drop. (Defensive: a tracked cell is kept
            // black by its live frame's `open_upvalues`, so it should never be
            // reached as white/severable.)
            UpvalueState::Tracked { value, .. } => Some(value),
            UpvalueState::Open { .. } => None,
        },
        Err(_) => {
            debug_assert!(false, "white upvalue cell borrowed during severing");
            None
        }
    }
}

/// Payload, closure, function-table, and `Function` nodes have no severable
/// cell of their own; their memory is reclaimed by the `Rc` cascade once
/// cells/envs are cleared.
fn sever_nothing(_: sema_core::NodePtr) -> Option<Value> {
    None
}

fn legacy_vm_entry_during_quantum_error() -> SemaError {
    SemaError::eval(
        "internal error: legacy native callback cannot re-enter a VM during an active runtime quantum",
    )
}

fn ensure_legacy_vm_entry_allowed(ctx: &EvalContext) -> Result<(), SemaError> {
    if ctx.runtime_quantum_active() || sema_core::in_runtime_quantum() {
        return Err(legacy_vm_entry_during_quantum_error());
    }
    Ok(())
}

/// Extracted VM closure: the closure itself and the function table from its compilation context.
pub type VmClosureInfo = (Rc<Closure>, Rc<Vec<Rc<Function>>>, Rc<Vec<Rc<NativeFn>>>);

/// Extract a VM closure from a Value, if it wraps a VmClosurePayload.
/// Returns the closure and the function table needed to create a task VM.
pub fn extract_vm_closure(val: &Value) -> Option<VmClosureInfo> {
    let nf = val.as_native_fn_ref()?;
    let payload = nf.payload.as_ref()?.downcast_ref::<VmClosurePayload>()?;
    Some((
        payload.closure.clone(),
        payload.functions.clone(),
        payload.native_fns.clone(),
    ))
}

/// Build an `Unbound` error decorated with a "Did you mean ...?" hint
/// when the name closely matches one in `globals`.
fn unbound_global_error(name_spur: Spur, globals: &Env) -> SemaError {
    let name = resolve_spur(name_spur);
    let mut err = SemaError::Unbound(name.clone());
    if let Some(hint) = veteran_hint(&name) {
        err = err.with_hint(hint);
    } else {
        let all_names: Vec<String> = globals
            .all_names()
            .iter()
            .map(|s| resolve_spur(*s))
            .collect();
        let candidates: Vec<&str> = all_names.iter().map(|s| s.as_str()).collect();
        if let Some(suggestion) = suggest_similar(&name, &candidates) {
            err = err.with_hint(format!("Did you mean '{suggestion}'?"));
        }
    }
    err
}

/// A call frame in the VM's call stack.
struct CallFrame {
    closure: Rc<Closure>,
    pc: usize,
    base: usize,
    /// Open upvalue cells for locals in this frame.
    /// Maps local slot → shared UpvalueCell. Created lazily when a local is captured.
    /// `None` means no locals have been captured yet (avoids heap allocation).
    open_upvalues: Option<Vec<Option<Rc<UpvalueCell>>>>,
    /// Base offset into VM::inline_cache for this function's cache slots.
    cache_base: usize,
}

/// Maximum number of call frames before raising a stack overflow error.
const MAX_FRAMES: usize = 2048;

/// The bytecode virtual machine.
pub struct VM {
    stack: Vec<Value>,
    frames: Vec<CallFrame>,
    globals: Rc<Env>,
    functions: Rc<Vec<Rc<Function>>>,
    /// The VM's *base* (top-level main) function table, fixed at construction.
    /// `self.functions` swaps to a callee's table on every cross-unit VM-closure
    /// call and is restored on return, so it is NOT a stable "main" reference:
    /// when a quantum yields mid-call, `run_quantum` returns with `self.functions`
    /// still pointing at the callee's table. `run_inner` must resolve a `None`
    /// (top-level main) closure's table from THIS stable field, not from whatever
    /// `self.functions` happens to be at re-entry — otherwise the next quantum
    /// adopts the callee's table as the main's and a later `MakeClosure` indexes
    /// the wrong (too-short) table.
    base_functions: Rc<Vec<Rc<Function>>>,
    /// Per-instruction inline cache for global lookups:
    /// (spur_bits, env_version, decoded binding).
    /// spur_bits distinguishes globals sharing the same slot (cross-VM closures).
    inline_cache: Vec<(u32, u64, CachedGlobal)>,
    /// Resolved native function table: native_id → (NativeFn Rc, name).
    /// Populated at VM creation from the compiler's native_table + global env.
    native_fns: Rc<Vec<Rc<NativeFn>>>,
    debug_values: HashMap<u64, Value>,
    next_debug_value_ref: u64,
    /// One-entry cache of the last home env this VM registered with the
    /// cycle collector (CORE-2): consecutive `make_closure`s share a home,
    /// so a pointer-equality hit skips the collector's seen-set probe. The
    /// `Weak` guards address reuse — a dead entry (strong count 0) never
    /// matches, even if a fresh env landed on the same address.
    gc_adopted_home: std::cell::RefCell<Weak<Env>>,
    instruction_budget: Option<usize>,
    instructions_executed: usize,
    /// Pending error to raise into a parked frame on the next `run_inner`.
    ///
    /// The value-resume path (`replace_stack_top`) resumes a frame parked on a
    /// structural suspend by injecting the awaited value onto its stack top.
    /// This is the rejection counterpart: when the awaited promise settled Failed, the
    /// runtime arms this so the next dispatch entry raises the error at the
    /// parked call site — exactly as if the yielding native had returned
    /// `Err(error)` — routing it through the normal exception machinery so an
    /// enclosing `try`/`catch` can handle it (and, if uncaught, it propagates
    /// out of `run_quantum` as an ordinary `Err`).
    pending_resume_error: Option<SemaError>,
    /// Cancellation snapshot for the currently running runtime quantum, used to
    /// build the [`NativeCallContext`] for structural native dispatch. Default
    /// (not requested) outside a runtime quantum. Set by `run_quantum` for the
    /// quantum's lifetime; reset afterward.
    quantum_cancellation: CancellationView,
    /// Parked-frame signal stashed by a helper-mediated native dispatch and taken
    /// by the owning opcode arm in the same iteration. Always `None` between
    /// opcodes.
    native_signal: Option<VmNativeSignal>,
}

impl sema_core::runtime::Trace for VM {
    fn trace(&self, sink: &mut dyn FnMut(sema_core::GcEdge<'_>)) -> bool {
        sink(sema_core::GcEdge::Env(&self.globals));
        sink(function_table_edge(&self.functions));
        for value in &self.stack {
            sink(sema_core::GcEdge::Value(value));
        }
        for frame in &self.frames {
            sink(vm_closure_edge(&frame.closure));
            if let Some(open) = &frame.open_upvalues {
                for cell in open.iter().flatten() {
                    sink(sema_core::GcEdge::Opaque {
                        ptr: sema_core::NodePtr::of_rc(cell),
                        strong_count: Rc::strong_count(cell),
                        trace: trace_upvalue_cell,
                        sever: sever_upvalue_cell,
                    });
                }
            }
        }
        for (_, _, cached) in &self.inline_cache {
            sink(sema_core::GcEdge::Value(cached.value()));
            match cached {
                CachedGlobal::Plain(_) => {}
                CachedGlobal::VmClosure {
                    closure, functions, ..
                } => {
                    sink(vm_closure_edge(closure));
                    sink(function_table_edge(functions));
                }
                CachedGlobal::Native { .. } => {}
            }
        }
        sink(native_table_edge(&self.native_fns));
        for value in self.debug_values.values() {
            sink(sema_core::GcEdge::Value(value));
        }
        true
    }
}

/// Cooperative continuation for a runtime-quantum multimethod call
/// (`call_value`/`call_value_with`'s `call_non_native`, Step G): the selected
/// method is driven as one `NativeOutcome::Call`, and this continuation just
/// forwards whatever it settles with straight through — the multimethod
/// call's result IS the selected method's result. Mirrors
/// `IdentityContinuation` in `sema-stdlib::list` (`apply`'s cooperative
/// path). Holds no state, so nothing to trace.
struct MultimethodCallContinuation;

impl Trace for MultimethodCallContinuation {
    fn trace(&self, _sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        true
    }
}

impl NativeContinuation for MultimethodCallContinuation {
    fn resume(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        match input {
            ResumeInput::Returned(value) => Ok(NativeOutcome::Return(value)),
            ResumeInput::Failed(error) => Err(error),
            ResumeInput::Cancelled(reason) => Err(SemaError::eval(format!(
                "multimethod call was cancelled ({reason:?})"
            ))),
            ResumeInput::Runtime(_) => Err(SemaError::eval(
                "multimethod continuation received an unexpected runtime response",
            )),
        }
    }
}

/// Args handoff mode when pushing a callee frame from a native boundary:
/// `Borrowed` clones each value into its local slot (the caller keeps its
/// buffer intact); `Owned` moves each value out, leaving nil behind (the
/// caller promised not to reuse the buffer). `Owned` is the refcount-shedding
/// protocol behind [`call_closure_owned`] — it keeps a uniquely-owned
/// accumulator uniquely owned across the callback boundary.
enum CallArgs<'a> {
    Borrowed(&'a [Value]),
    Owned(&'a mut [Value]),
}

impl CallArgs<'_> {
    fn len(&self) -> usize {
        match self {
            CallArgs::Borrowed(a) => a.len(),
            CallArgs::Owned(a) => a.len(),
        }
    }
}

/// Call a NativeFn-wrapped VM closure with an args buffer the caller owns and
/// will NOT reuse: the values are MOVED into the callee frame (the buffer is
/// left holding nils). Combined with the compiler's `TakeLocal` last-use
/// moves, this lets a fold accumulator reach the stdlib's `strong_count == 1`
/// in-place fast paths (`assoc` & co.) instead of deep-cloning per step.
///
/// Mirrors the host-only borrowed fallback wrapper built in `make_closure`.
/// Returns `None` when `func` is not a VM closure; the caller then falls back
/// to the borrowed protocol.
pub fn call_closure_owned(
    func: &Value,
    ctx: &EvalContext,
    args: &mut [Value],
) -> Option<Result<Value, SemaError>> {
    let (closure, functions, native_fns) = extract_vm_closure(func)?;
    if let Err(error) = ensure_legacy_vm_entry_allowed(ctx) {
        return Some(Err(error));
    }
    // The top-level main closure never travels as a callback value; if it
    // somehow does (globals is None), let the generic borrowed path handle it.
    let globals = closure.globals.as_ref()?.clone();
    // No authoritative owner is available in this fallback. Traverse values
    // that were already detached by the explicit caller; an Open cell is
    // rejected when the foreign VM tries to dereference it.
    close_closure_upvalues_for_foreign_run(&closure);
    let mut vm = VM::new_with_rc_functions(globals, functions, native_fns);
    if let Err(e) = vm.setup_for_call_args(closure, CallArgs::Owned(args)) {
        return Some(Err(e));
    }
    Some(vm.run(ctx))
}

thread_local! {
    /// Stack of pointers to the `DebugState` of an active debug session on this
    /// thread. Set by `execute_debug` (and the cooperative WASM start) around the
    /// run loop, popped on exit. The async scheduler is reached through the
    /// `RUN_SCHEDULER_CALLBACK` fn-pointer seam (`async_signal.rs`), which cannot
    /// carry a borrowed `&mut DebugState`; it consults this thread-local instead so
    /// task steps run in debug mode and a mid-task breakpoint can stop/resume.
    ///
    /// SAFETY: each pointer is valid for as long as the `execute_debug` frame that
    /// pushed it is on the Rust call stack. While that frame is blocked inside a
    /// native call that re-enters the scheduler, the `&mut DebugState` it owns is
    /// DORMANT (not otherwise touched) — the scheduler reborrows it through this
    /// raw pointer for the duration of one task step and drops the borrow before
    /// returning, so no two live `&mut` ever alias.
    static ACTIVE_DEBUG: RefCell<Vec<ActiveDebugTarget>> = const { RefCell::new(Vec::new()) };
}

#[derive(Clone, Copy)]
struct ActiveDebugTarget {
    state: *mut crate::debug::DebugState,
    root: Option<RootId>,
}

/// RAII guard registering a `DebugState` as the active debug session for the
/// duration of a debug run, unregistering it on drop (including panic unwind).
///
/// Public so a host (the native DAP backend) can register its `DebugState`
/// around a runtime drive: the runtime's `run_parked_quantum` observes the
/// session via [`is_debug_session_active`] and reaches the state through
/// [`with_active_debug`] to run the debug-aware quantum.
pub struct ActiveDebugGuard;

impl ActiveDebugGuard {
    pub fn enter(debug: &mut crate::debug::DebugState) -> Self {
        Self::enter_target(debug, None)
    }

    /// Register a debugger that applies only to `root`. Other ready roots in
    /// the same runtime continue through ordinary non-debug quanta.
    pub fn enter_for_root(debug: &mut crate::debug::DebugState, root: RootId) -> Self {
        Self::enter_target(debug, Some(root))
    }

    fn enter_target(debug: &mut crate::debug::DebugState, root: Option<RootId>) -> Self {
        ACTIVE_DEBUG.with(|stack| {
            stack
                .borrow_mut()
                .push(ActiveDebugTarget { state: debug, root });
        });
        ActiveDebugGuard
    }
}

impl Drop for ActiveDebugGuard {
    fn drop(&mut self) {
        ACTIVE_DEBUG.with(|stack| {
            stack.borrow_mut().pop();
        });
    }
}

/// True when a debug session is active on this thread (cheap: a thread-local
/// length check). The async scheduler gates its debug-aware task-step path on
/// this so the non-debug hot path stays byte-identical when not debugging.
pub fn is_debug_session_active() -> bool {
    ACTIVE_DEBUG.with(|stack| !stack.borrow().is_empty())
}

/// True when the innermost active debugger applies to `root`. An unscoped
/// debugger applies to every root, preserving the native DAP contract.
pub(crate) fn is_debug_session_active_for(root: RootId) -> bool {
    ACTIVE_DEBUG.with(|stack| {
        stack
            .borrow()
            .last()
            .is_some_and(|target| target.root.is_none_or(|expected| expected == root))
    })
}

/// Run `f` with a mutable borrow of the innermost active `DebugState`, if any.
/// Returns `None` when no debug session is active on this thread (the scheduler's
/// non-debug path). Used by the scheduler to reach the `DebugState` it cannot
/// receive by reference through the fn-pointer callback seam.
///
/// SAFETY: see `ACTIVE_DEBUG`. The top pointer was registered by a live
/// `execute_debug` frame on this thread's Rust stack; that frame is blocked in the
/// native call that re-entered the scheduler and does not touch its
/// `&mut DebugState` while blocked. The reborrow does not escape `f`.
pub fn with_active_debug<R>(f: impl FnOnce(&mut crate::debug::DebugState) -> R) -> Option<R> {
    let ptr = ACTIVE_DEBUG.with(|stack| stack.borrow().last().map(|target| target.state))?;
    // SAFETY: as documented above.
    let debug = unsafe { &mut *ptr };
    Some(f(debug))
}

pub(crate) fn with_active_debug_for_root<R>(
    root: RootId,
    f: impl FnOnce(&mut crate::debug::DebugState) -> R,
) -> Option<R> {
    let ptr = ACTIVE_DEBUG.with(|stack| {
        stack.borrow().last().and_then(|target| {
            target
                .root
                .is_none_or(|expected| expected == root)
                .then_some(target.state)
        })
    })?;
    // SAFETY: see `ACTIVE_DEBUG`. Root filtering does not change the guard's
    // lifetime or permit the pointer to escape this call.
    let debug = unsafe { &mut *ptr };
    Some(f(debug))
}

/// Snapshot the open upvalues of a cooperative HOF callback (and any closures
/// carried in its `args`) against `owner_vm` — the parent (HOF-invoking) VM
/// whose stack those cells point into — before the callback runs on a foreign
/// callback VM. The walker reads only this explicit borrowed owner and turns
/// matching open cells into shared `Tracked` cells. A `set!` performed on the
/// callback VM therefore remains visible after the parent synchronizes its
/// tracked cells back to the stack.
pub fn snapshot_escaping_call_with_owner(owner_vm: &mut VM, callable: &Value, args: &[Value]) {
    let mut walker = EscapingValueWalker::with_owner(owner_vm);
    walker.visit_value(callable);
    for arg in args {
        walker.visit_value(arg);
    }
}

fn snapshot_escaping_args_with_owner(owner_vm: &VM, args: &[Value]) {
    let mut walker = EscapingValueWalker::with_owner(owner_vm);
    for arg in args {
        walker.visit_value(arg);
    }
}

fn snapshot_native_escaping_args(owner_vm: &VM, native: &NativeFn, args: &[Value]) {
    let indices = native.escaping_args();
    if indices.is_empty()
        || !indices
            .iter()
            .filter_map(|&index| args.get(index))
            .any(|value| NodePtr::of_value(value).is_some())
    {
        return;
    }
    let mut walker = EscapingValueWalker::with_owner(owner_vm);
    for &index in indices {
        if let Some(value) = args.get(index) {
            walker.visit_value(value);
        }
    }
}

/// Snapshot only the arguments a native declares it will retain, using the
/// parked caller VM as the defining-frame owner.
pub(crate) fn snapshot_native_escaping_args_with_owner(
    owner_vm: &mut VM,
    native: &NativeFn,
    args: &[Value],
) {
    if native.escaping_args().is_empty() {
        return;
    }
    snapshot_native_escaping_args(owner_vm, native, args);
}

struct EscapingValueWalker<'a> {
    visited_values: HashSet<NodePtr>,
    visited_closures: HashSet<*const Closure>,
    owner_vm: Option<&'a VM>,
}

/// Original paused-frame upvalue states retained while a debugger scratch VM
/// runs. Rejected expressions restore these cells before the owner resumes;
/// successful expressions keep their writes and synchronize tracked locals.
struct DebugUpvalueRollback {
    seen: HashSet<*const UpvalueCell>,
    entries: Vec<(Rc<UpvalueCell>, DebugUpvalueRollbackState)>,
}

enum DebugUpvalueRollbackState {
    Closed(Value),
    Tracked {
        frame_base: usize,
        slot: usize,
        value: Value,
    },
}

impl DebugUpvalueRollback {
    fn new() -> Self {
        Self {
            seen: HashSet::new(),
            entries: Vec::new(),
        }
    }

    fn capture_owner_frames(
        &mut self,
        owner_vm: &VM,
        budget: &mut DebugTraversalBudget,
    ) -> Result<(), SemaError> {
        for frame in &owner_vm.frames {
            if let Some(open_upvalues) = &frame.open_upvalues {
                for cell in open_upvalues.iter().flatten() {
                    if self.capture_cell(cell) {
                        budget.reserve_node()?;
                    }
                }
            }
            for cell in &frame.closure.upvalues {
                if self.capture_cell(cell) {
                    budget.reserve_node()?;
                }
            }
        }
        Ok(())
    }

    fn capture_cell(&mut self, cell: &Rc<UpvalueCell>) -> bool {
        let state = cell.state.borrow();
        if matches!(&*state, UpvalueState::Open { .. }) || !self.seen.insert(Rc::as_ptr(cell)) {
            return false;
        }
        let original = match &*state {
            UpvalueState::Open { .. } => unreachable!("open cells are not rollback-owned"),
            UpvalueState::Closed(value) => DebugUpvalueRollbackState::Closed(value.clone()),
            UpvalueState::Tracked {
                frame_base,
                slot,
                value,
            } => DebugUpvalueRollbackState::Tracked {
                frame_base: *frame_base,
                slot: *slot,
                value: value.clone(),
            },
        };
        drop(state);
        self.entries.push((Rc::clone(cell), original));
        true
    }

    fn restore(self) {
        for (cell, original) in self.entries {
            *cell.state.borrow_mut() = match original {
                DebugUpvalueRollbackState::Closed(value) => UpvalueState::Closed(value),
                DebugUpvalueRollbackState::Tracked {
                    frame_base,
                    slot,
                    value,
                } => UpvalueState::Tracked {
                    frame_base,
                    slot,
                    value,
                },
            };
        }
    }
}

struct DebugTraversalBudget {
    nodes_remaining: usize,
    ordinary_edges_remaining: usize,
    cancellation: CancellationView,
    deadline: Option<Instant>,
}

impl DebugTraversalBudget {
    fn new(cancellation: CancellationView, deadline: Option<Instant>) -> Self {
        Self {
            nodes_remaining: DEBUG_EVALUATION_SNAPSHOT_NODE_LIMIT,
            ordinary_edges_remaining: DEBUG_EVALUATION_SNAPSHOT_NODE_LIMIT,
            cancellation,
            deadline,
        }
    }

    fn reserve_node(&mut self) -> Result<(), SemaError> {
        self.check_boundary()?;
        if self.nodes_remaining == 0 {
            return Err(SemaError::eval(
                "debug evaluation exceeded snapshot node limit",
            ));
        }
        self.nodes_remaining -= 1;
        Ok(())
    }

    fn reserve_ordinary_edges(&mut self, count: usize) -> Result<(), SemaError> {
        self.check_boundary()?;
        if count > self.ordinary_edges_remaining {
            return Err(SemaError::eval(
                "debug evaluation exceeded snapshot node limit",
            ));
        }
        self.ordinary_edges_remaining -= count;
        Ok(())
    }

    fn check_boundary(&self) -> Result<(), SemaError> {
        if self.cancellation.is_requested() {
            return Err(SemaError::eval("debug evaluation was cancelled"));
        }
        if self
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            return Err(SemaError::eval("debug evaluation exceeded deadline"));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DebugGraphMode {
    CaptureRollback,
    SnapshotOwner,
}

enum DebugGraphWork {
    Value(Value),
    Closure(Rc<Closure>),
    Env(Rc<Env>),
    EnvBindings(Rc<EnvBindings>),
    Function(Rc<Function>),
    FunctionTable(Rc<Vec<Rc<Function>>>),
    Opaque {
        ptr: NodePtr,
        trace: OpaqueTraceFn,
        keepalive: Value,
    },
}

enum DebugVariableTarget {
    Local {
        frame_id: usize,
        slot: usize,
        stack_index: usize,
    },
    Upvalue {
        frame_id: usize,
        cell: Rc<UpvalueCell>,
    },
}

impl DebugVariableTarget {
    fn frame_id(&self) -> usize {
        match self {
            Self::Local { frame_id, .. } | Self::Upvalue { frame_id, .. } => *frame_id,
        }
    }
}

/// Iterative traversal of values reachable by debugger evaluation. Scheduled
/// graph state and ordinary container/environment fanout are bounded; opaque
/// payload and runtime-interior tracers are boundary-checked edge by edge, but
/// their callback API cannot abort enumeration partway through one tracer.
/// Rollback capture is rooted only in stopped-frame bindings; owner snapshotting
/// separately follows globals and closure homes so foreign execution is safe
/// without making unrelated global state transactional.
struct DebugValueGraphWalker<'owner, 'state> {
    owner_vm: &'owner VM,
    mode: DebugGraphMode,
    rollback: Option<&'state mut DebugUpvalueRollback>,
    budget: &'state mut DebugTraversalBudget,
    work: Vec<DebugGraphWork>,
    visited_values: HashSet<NodePtr>,
    visited_closures: HashSet<*const Closure>,
    visited_envs: HashSet<NodePtr>,
    visited_opaque: HashSet<NodePtr>,
    visited_functions: HashSet<*const Function>,
    visited_function_tables: HashSet<*const Vec<Rc<Function>>>,
}

impl<'owner, 'state> DebugValueGraphWalker<'owner, 'state> {
    fn capture_stopped_bindings(
        owner_vm: &'owner VM,
        env: &Env,
        rollback: &'state mut DebugUpvalueRollback,
        budget: &'state mut DebugTraversalBudget,
    ) -> Result<(), SemaError> {
        let mut walker = Self::new(
            owner_vm,
            DebugGraphMode::CaptureRollback,
            Some(rollback),
            budget,
        );
        let bindings = env
            .bindings
            .try_borrow()
            .map_err(|_| SemaError::eval("debug evaluation could not inspect a borrowed value"))?;
        walker.budget.reserve_ordinary_edges(bindings.len())?;
        for value in bindings.values() {
            walker.schedule_value(value.clone())?;
        }
        drop(bindings);
        walker.run()
    }

    fn snapshot_reachable(
        owner_vm: &'owner VM,
        env: Rc<Env>,
        budget: &'state mut DebugTraversalBudget,
    ) -> Result<(), SemaError> {
        let mut walker = Self::new(owner_vm, DebugGraphMode::SnapshotOwner, None, budget);
        walker.schedule_env(env)?;
        walker.run()
    }

    fn new(
        owner_vm: &'owner VM,
        mode: DebugGraphMode,
        rollback: Option<&'state mut DebugUpvalueRollback>,
        budget: &'state mut DebugTraversalBudget,
    ) -> Self {
        Self {
            owner_vm,
            mode,
            rollback,
            budget,
            work: Vec::new(),
            visited_values: HashSet::new(),
            visited_closures: HashSet::new(),
            visited_envs: HashSet::new(),
            visited_opaque: HashSet::new(),
            visited_functions: HashSet::new(),
            visited_function_tables: HashSet::new(),
        }
    }

    fn run(&mut self) -> Result<(), SemaError> {
        while let Some(work) = self.work.pop() {
            match work {
                DebugGraphWork::Value(value) => self.visit_value(value)?,
                DebugGraphWork::Closure(closure) => self.visit_closure(closure)?,
                DebugGraphWork::Env(env) => self.visit_env(env)?,
                DebugGraphWork::EnvBindings(bindings) => self.visit_env_bindings(bindings)?,
                DebugGraphWork::Function(function) => self.visit_function(function)?,
                DebugGraphWork::FunctionTable(functions) => self.visit_function_table(functions)?,
                DebugGraphWork::Opaque {
                    ptr,
                    trace,
                    keepalive,
                } => self.visit_opaque(ptr, trace, keepalive)?,
            }
        }
        self.budget.check_boundary()
    }

    fn schedule_value(&mut self, value: Value) -> Result<(), SemaError> {
        if NodePtr::of_value(&value).is_some_and(|node| !self.visited_values.insert(node)) {
            return Ok(());
        }
        self.budget.reserve_node()?;
        self.work.push(DebugGraphWork::Value(value));
        Ok(())
    }

    fn schedule_closure(&mut self, closure: Rc<Closure>) -> Result<(), SemaError> {
        if !self
            .visited_closures
            .insert(std::ptr::from_ref(closure.as_ref()))
        {
            return Ok(());
        }
        self.budget.reserve_node()?;
        self.work.push(DebugGraphWork::Closure(closure));
        Ok(())
    }

    fn schedule_env(&mut self, env: Rc<Env>) -> Result<(), SemaError> {
        if !self
            .visited_envs
            .insert(NodePtr::of_env_bindings(env.as_ref()))
        {
            return Ok(());
        }
        self.budget.reserve_node()?;
        self.work.push(DebugGraphWork::Env(env));
        Ok(())
    }

    fn schedule_env_bindings(&mut self, bindings: Rc<EnvBindings>) -> Result<(), SemaError> {
        let node = NodePtr::of_rc(&bindings);
        if !self.visited_envs.insert(node) {
            return Ok(());
        }
        self.budget.reserve_node()?;
        self.work.push(DebugGraphWork::EnvBindings(bindings));
        Ok(())
    }

    fn schedule_opaque(
        &mut self,
        ptr: NodePtr,
        trace: OpaqueTraceFn,
        keepalive: Value,
    ) -> Result<(), SemaError> {
        if !self.visited_opaque.insert(ptr) {
            return Ok(());
        }
        self.budget.reserve_node()?;
        self.work.push(DebugGraphWork::Opaque {
            ptr,
            trace,
            keepalive,
        });
        Ok(())
    }

    fn schedule_function(&mut self, function: Rc<Function>) -> Result<(), SemaError> {
        if !self
            .visited_functions
            .insert(std::ptr::from_ref(function.as_ref()))
        {
            return Ok(());
        }
        self.budget.reserve_node()?;
        self.work.push(DebugGraphWork::Function(function));
        Ok(())
    }

    fn schedule_function_table(
        &mut self,
        functions: Rc<Vec<Rc<Function>>>,
    ) -> Result<(), SemaError> {
        if !self.visited_function_tables.insert(Rc::as_ptr(&functions)) {
            return Ok(());
        }
        self.budget.reserve_node()?;
        self.work.push(DebugGraphWork::FunctionTable(functions));
        Ok(())
    }

    fn visit_value(&mut self, value: Value) -> Result<(), SemaError> {
        if let Some((closure, functions, _native_fns)) = extract_vm_closure(&value) {
            self.schedule_closure(closure)?;
            self.schedule_function_table(functions)?;
        }

        let ordinary_edge_count = Self::ordinary_value_edge_count(&value)?;
        if let Some(count) = ordinary_edge_count {
            self.budget.reserve_ordinary_edges(count)?;
        }
        let keepalive = value.clone();
        let mut schedule_error = None;
        let complete = sema_core::trace_value(&value, &mut |edge| {
            if schedule_error.is_none() {
                if ordinary_edge_count.is_none() {
                    schedule_error = self.budget.reserve_ordinary_edges(1).err();
                }
                if schedule_error.is_none() {
                    schedule_error = self.schedule_gc_edge(edge, &keepalive).err();
                }
            }
        });
        if !complete {
            return Err(SemaError::eval(
                "debug evaluation could not inspect a borrowed value",
            ));
        }
        match schedule_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn ordinary_value_edge_count(value: &Value) -> Result<Option<usize>, SemaError> {
        let borrowed_error =
            || SemaError::eval("debug evaluation could not inspect a borrowed value");
        let count = match value.view_ref() {
            ValueViewRef::List(items) | ValueViewRef::Vector(items) => Some(items.len()),
            ValueViewRef::Map(map) => Some(map.len().saturating_mul(2)),
            ValueViewRef::HashMap(map) => Some(map.len().saturating_mul(2)),
            ValueViewRef::Record(record) => Some(record.fields.len()),
            ValueViewRef::ToolDef(_) => Some(2),
            ValueViewRef::Agent(agent) => Some(agent.tools.len()),
            ValueViewRef::Thunk(thunk) => {
                let forced = thunk.forced.try_borrow().map_err(|_| borrowed_error())?;
                Some(1 + usize::from(forced.is_some()))
            }
            ValueViewRef::MutableArray(array) => {
                let items = array.items.try_borrow().map_err(|_| borrowed_error())?;
                Some(items.len())
            }
            ValueViewRef::MutableCell(cell) => {
                cell.value.try_borrow().map_err(|_| borrowed_error())?;
                Some(1)
            }
            ValueViewRef::MultiMethod(multimethod) => {
                let methods = multimethod
                    .methods
                    .try_borrow()
                    .map_err(|_| borrowed_error())?;
                let default = multimethod
                    .default
                    .try_borrow()
                    .map_err(|_| borrowed_error())?;
                Some(
                    1usize
                        .saturating_add(methods.len().saturating_mul(2))
                        .saturating_add(usize::from(default.is_some())),
                )
            }
            ValueViewRef::Macro(expander) => Some(
                expander.body.len().saturating_add(
                    expander
                        .syntax_rules
                        .as_ref()
                        .map_or(0, |rules| rules.rules.len().saturating_mul(2)),
                ),
            ),
            ValueViewRef::Lambda(lambda) => Some(
                lambda
                    .body
                    .len()
                    .saturating_add(1)
                    .saturating_add(usize::from(lambda.env.parent.is_some())),
            ),
            ValueViewRef::AsyncPromise(_)
            | ValueViewRef::Channel(_)
            | ValueViewRef::NativeFn(_) => None,
            _ => Some(0),
        };
        Ok(count)
    }

    fn schedule_gc_edge(&mut self, edge: GcEdge<'_>, keepalive: &Value) -> Result<(), SemaError> {
        match edge {
            GcEdge::Value(value) => self.schedule_value(value.clone()),
            GcEdge::Env(env) if self.mode == DebugGraphMode::SnapshotOwner => {
                self.schedule_env(Rc::clone(env))
            }
            GcEdge::EnvBindings(bindings) if self.mode == DebugGraphMode::SnapshotOwner => {
                self.schedule_env_bindings(Rc::clone(bindings))
            }
            GcEdge::Env(_) | GcEdge::EnvBindings(_) => Ok(()),
            GcEdge::Opaque { ptr, trace, .. } => {
                self.schedule_opaque(ptr, trace, keepalive.clone())
            }
        }
    }

    fn visit_closure(&mut self, closure: Rc<Closure>) -> Result<(), SemaError> {
        self.schedule_function(Rc::clone(&closure.func))?;
        if let Some(functions) = &closure.functions {
            self.schedule_function_table(Rc::clone(functions))?;
        }
        for cell in &closure.upvalues {
            let state = cell.state.borrow();
            let next = match &*state {
                UpvalueState::Open { frame_base, slot } => {
                    let open = (*frame_base, *slot);
                    drop(state);
                    let Some(value) = self.owner_open_value(cell, open.0, open.1) else {
                        continue;
                    };
                    if self.mode == DebugGraphMode::SnapshotOwner {
                        *cell.state.borrow_mut() = UpvalueState::Tracked {
                            frame_base: open.0,
                            slot: open.1,
                            value: value.clone(),
                        };
                    }
                    value
                }
                UpvalueState::Closed(value) | UpvalueState::Tracked { value, .. } => {
                    let value = value.clone();
                    drop(state);
                    if let Some(rollback) = self.rollback.as_deref_mut() {
                        rollback.capture_cell(cell);
                    }
                    value
                }
            };
            self.schedule_value(next)?;
        }
        if self.mode == DebugGraphMode::SnapshotOwner {
            if let Some(globals) = &closure.globals {
                self.schedule_env(Rc::clone(globals))?;
            }
        }
        Ok(())
    }

    fn visit_env(&mut self, env: Rc<Env>) -> Result<(), SemaError> {
        debug_assert_eq!(self.mode, DebugGraphMode::SnapshotOwner);
        let bindings = env
            .bindings
            .try_borrow()
            .map_err(|_| SemaError::eval("debug evaluation could not inspect a borrowed value"))?;
        self.budget.reserve_ordinary_edges(bindings.len())?;
        for value in bindings.values() {
            self.schedule_value(value.clone())?;
        }
        drop(bindings);
        if let Some(parent) = &env.parent {
            self.schedule_env(Rc::clone(parent))?;
        }
        Ok(())
    }

    fn visit_env_bindings(&mut self, bindings: Rc<EnvBindings>) -> Result<(), SemaError> {
        let bindings = bindings
            .try_borrow()
            .map_err(|_| SemaError::eval("debug evaluation could not inspect a borrowed value"))?;
        self.budget.reserve_ordinary_edges(bindings.len())?;
        for value in bindings.values() {
            self.schedule_value(value.clone())?;
        }
        Ok(())
    }

    fn visit_function(&mut self, function: Rc<Function>) -> Result<(), SemaError> {
        self.budget
            .reserve_ordinary_edges(function.chunk.consts.len())?;
        for value in &function.chunk.consts {
            self.schedule_value(value.clone())?;
        }
        Ok(())
    }

    fn visit_function_table(&mut self, functions: Rc<Vec<Rc<Function>>>) -> Result<(), SemaError> {
        self.budget.reserve_ordinary_edges(functions.len())?;
        for function in functions.iter() {
            self.schedule_function(Rc::clone(function))?;
        }
        Ok(())
    }

    fn visit_opaque(
        &mut self,
        ptr: NodePtr,
        trace: OpaqueTraceFn,
        keepalive: Value,
    ) -> Result<(), SemaError> {
        let mut trace_error = None;
        let complete = trace(ptr, &mut |edge| {
            if trace_error.is_none() {
                trace_error = self.budget.reserve_ordinary_edges(1).err();
                if trace_error.is_none() {
                    trace_error = self.schedule_gc_edge(edge, &keepalive).err();
                }
            }
        });
        if !complete {
            return Err(SemaError::eval(
                "debug evaluation could not inspect a borrowed value",
            ));
        }
        match trace_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn owner_open_value(
        &self,
        cell: &Rc<UpvalueCell>,
        frame_base: usize,
        slot: usize,
    ) -> Option<Value> {
        let owns_cell = self.owner_vm.frames.iter().any(|frame| {
            frame.base == frame_base
                && frame
                    .open_upvalues
                    .as_ref()
                    .and_then(|open| open.get(slot))
                    .and_then(Option::as_ref)
                    .is_some_and(|candidate| Rc::ptr_eq(candidate, cell))
        });
        owns_cell
            .then(|| self.owner_vm.stack.get(frame_base + slot).cloned())
            .flatten()
    }
}

impl<'a> EscapingValueWalker<'a> {
    fn new(owner_vm: Option<&'a VM>) -> Self {
        Self {
            visited_values: HashSet::new(),
            visited_closures: HashSet::new(),
            owner_vm,
        }
    }

    fn with_owner(owner_vm: &'a VM) -> Self {
        Self::new(Some(owner_vm))
    }

    fn without_owner() -> Self {
        Self::new(None)
    }

    fn visit_value(&mut self, value: &Value) {
        if NodePtr::of_value(value).is_some_and(|node| !self.visited_values.insert(node)) {
            return;
        }
        if let Some((closure, _functions, _native_fns)) = extract_vm_closure(value) {
            self.visit_closure(&closure);
        }

        // Collect direct children before recursing so no RefCell borrow held by
        // a multimethod/mutable container trace crosses a nested snapshot.
        let mut children = Vec::new();
        // Escape snapshots run at a VM/runtime handoff after the producing
        // native has returned, so no participant may retain a conflicting
        // container borrow across this boundary.
        let complete = sema_core::trace_value(value, &mut |edge| {
            if let GcEdge::Value(child) = edge {
                children.push(child.clone());
            }
        });
        assert!(
            complete,
            "escaping-value snapshot must run without a conflicting container borrow"
        );
        for child in children {
            self.visit_value(&child);
        }
    }

    fn visit_closure(&mut self, closure: &Closure) {
        if !self.visited_closures.insert(std::ptr::from_ref(closure)) {
            return;
        }
        self.close_upvalues(closure);
    }

    fn close_upvalues(&mut self, closure: &Closure) {
        for cell in &closure.upvalues {
            let open = {
                let state = cell.state.borrow();
                match &*state {
                    UpvalueState::Open { frame_base, slot } => Some((*frame_base, *slot)),
                    UpvalueState::Closed(value) | UpvalueState::Tracked { value, .. } => {
                        let value = value.clone();
                        drop(state);
                        self.visit_value(&value);
                        None
                    }
                }
            };
            let Some((frame_base, slot)) = open else {
                continue;
            };

            if let Some(vm) = self.owner_vm {
                let owns_cell = vm.frames.iter().any(|frame| {
                    frame.base == frame_base
                        && frame
                            .open_upvalues
                            .as_ref()
                            .and_then(|open| open.get(slot))
                            .and_then(Option::as_ref)
                            .is_some_and(|candidate| Rc::ptr_eq(candidate, cell))
                });
                if frame_base + slot < vm.stack.len() && owns_cell {
                    let value = vm.stack[frame_base + slot].clone();
                    let nested = value.clone();
                    // Keep the cell associated with its live defining frame so
                    // parent and foreign writes continue to share one value.
                    *cell.state.borrow_mut() = UpvalueState::Tracked {
                        frame_base,
                        slot,
                        value,
                    };
                    // Marking Tracked precedes recursion; the allocation set
                    // terminates cycles that return through this cell's value.
                    self.visit_value(&nested);
                }
            }
        }
    }
}

/// Traverse an already-detached closure graph without an owner stack.
///
/// Open cells remain open because there is no authoritative stack to read.
/// Closed and tracked values are still traversed so nested closure graphs are
/// normalized without consulting ambient VM state.
pub fn close_closure_upvalues_for_foreign_run(closure: &Closure) {
    EscapingValueWalker::without_owner().visit_closure(closure);
}

/// Snapshot `closure`'s still-open upvalue cells against the explicit paused VM
/// that owns their stack slots. This is used before spawn and other foreign-VM
/// handoffs, where retaining an `Open` cell would make the new VM index the
/// owner's stack coordinates.
pub fn close_closure_upvalues_with_owner(owner_vm: &mut VM, closure: &Closure) {
    EscapingValueWalker::with_owner(owner_vm).visit_closure(closure);
}

/// Error for dereferencing an Open upvalue cell whose stack slot is not on the
/// executing VM's stack — a closure with open upvalues escaped its owning VM
/// without an explicit-owner snapshot (see `close_closure_upvalues_with_owner`).
#[cold]
#[inline(never)]
fn foreign_upvalue_error() -> SemaError {
    SemaError::eval(
        "captured variable's stack slot is not on this VM \
         (a closure with open upvalues escaped its owning VM)",
    )
}

/// Reject interior-mutable containers (mutable arrays/cells) as keys in map
/// literals (`{k v}` / hashmap literals): their contents can change after
/// insertion, which would silently corrupt the map's lookup invariants. The
/// check is deep — a key wrapping a mutable container mutates all the same.
/// `items` is the flattened `[k, v, k, v, …]` slice popped for the literal.
fn check_literal_map_keys(items: &[Value]) -> Result<(), SemaError> {
    for pair in items.chunks(2) {
        if pair[0].contains_mutable_container() {
            return Err(
                SemaError::type_error("immutable map key", pair[0].type_name())
                    .with_hint("freeze the key first (mutable-array/->vector or mutable-cell/get)"),
            );
        }
    }
    Ok(())
}

/// Close all open upvalues in the given open_upvalues vec, reading from the stack.
///
/// `Open` cells are closed with the current stack value. A `Tracked` cell
/// (detached-but-live: its defining frame — this one — is exiting) is finalized
/// with its OWN `value`, which already reflects the latest parent `StoreLocal`
/// and task `StoreUpvalue` writes (its stack slot only saw the parent writes),
/// so the tracked value is the authoritative final value.
fn close_open_upvalues(open: &mut [Option<Rc<UpvalueCell>>], stack: &[Value], base: usize) {
    for (slot, maybe_cell) in open.iter_mut().enumerate() {
        if let Some(cell) = maybe_cell {
            let mut state = cell.state.borrow_mut();
            let closed = match &mut *state {
                UpvalueState::Open { .. } => Some(stack[base + slot].clone()),
                UpvalueState::Tracked { value, .. } => Some(std::mem::replace(value, Value::nil())),
                UpvalueState::Closed(_) => None,
            };
            if let Some(v) = closed {
                *state = UpvalueState::Closed(v);
            }
        }
        *maybe_cell = None;
    }
}

/// Close open upvalues above a given slot threshold AND clear the entries.
fn close_open_upvalues_above(
    open: &mut [Option<Rc<UpvalueCell>>],
    stack: &[Value],
    base: usize,
    min_slot: usize,
) {
    for (slot, maybe_cell) in open.iter_mut().enumerate() {
        if slot >= min_slot {
            if let Some(cell) = maybe_cell {
                let mut state = cell.state.borrow_mut();
                // Mirror `close_open_upvalues`: `Open` closes from the stack, a
                // detached-but-live `Tracked` finalizes with its own value.
                let closed = match &mut *state {
                    UpvalueState::Open { .. } => Some(stack[base + slot].clone()),
                    UpvalueState::Tracked { value, .. } => {
                        Some(std::mem::replace(value, Value::nil()))
                    }
                    UpvalueState::Closed(_) => None,
                };
                if let Some(v) = closed {
                    *state = UpvalueState::Closed(v);
                }
            }
            *maybe_cell = None;
        }
    }
}

impl VM {
    /// Create a new VM. If `native_spurs` is non-empty, each entry is resolved
    /// from `globals` to build a direct-dispatch table for CallNative opcodes.
    pub fn new(
        globals: Rc<Env>,
        mut functions: Vec<Rc<Function>>,
        native_spurs: &[Spur],
        main_cache_slots: u16,
    ) -> Result<Self, SemaError> {
        let native_fns = Self::resolve_native_table(&globals, native_spurs)?;
        // Assign cache_offset to each function and compute total cache size.
        // Main closure's cache_offset is 0; child functions start after it.
        let mut total_cache_slots: usize = main_cache_slots as usize;
        for func_rc in &mut functions {
            let func = Rc::make_mut(func_rc);
            func.cache_offset = total_cache_slots;
            total_cache_slots += func.chunk.n_global_cache_slots as usize;
        }
        ensure_cycle_gc_wired();
        let functions = Rc::new(functions);
        Ok(VM {
            stack: Vec::with_capacity(256),
            frames: Vec::with_capacity(64),
            globals,
            functions: functions.clone(),
            base_functions: functions,
            inline_cache: vec![(u32::MAX, 0, CachedGlobal::Plain(Value::nil())); total_cache_slots],
            native_fns: Rc::new(native_fns),
            debug_values: HashMap::new(),
            next_debug_value_ref: DEBUG_VALUE_REF_BASE,
            gc_adopted_home: std::cell::RefCell::new(Weak::new()),
            instruction_budget: None,
            instructions_executed: 0,
            pending_resume_error: None,
            quantum_cancellation: CancellationView::default(),
            native_signal: None,
        })
    }

    /// Resolve a native_id → Spur table into a Vec<Rc<NativeFn>> by looking up the global env.
    fn resolve_native_table(
        globals: &Env,
        native_spurs: &[Spur],
    ) -> Result<Vec<Rc<NativeFn>>, SemaError> {
        let mut table = Vec::with_capacity(native_spurs.len());
        for &spur in native_spurs {
            let val = globals.get(spur).ok_or_else(|| {
                SemaError::eval(format!(
                    "CallNative: native function '{}' not found in global env",
                    resolve_spur(spur)
                ))
            })?;
            let native_rc = val.as_native_fn_rc().ok_or_else(|| {
                SemaError::eval(format!(
                    "CallNative: '{}' is not a native function",
                    resolve_spur(spur)
                ))
            })?;
            table.push(native_rc);
        }
        Ok(table)
    }

    /// Ensure the inline_cache has enough slots for a function's cache needs.
    fn ensure_cache_space(&mut self, func: &Function) {
        let needed = func.cache_offset + func.chunk.n_global_cache_slots as usize;
        if needed > self.inline_cache.len() {
            self.inline_cache
                .resize(needed, (u32::MAX, 0, CachedGlobal::Plain(Value::nil())));
        }
    }

    fn new_with_rc_functions(
        globals: Rc<Env>,
        functions: Rc<Vec<Rc<Function>>>,
        native_fns: Rc<Vec<Rc<NativeFn>>>,
    ) -> Self {
        let total_cache_slots: usize = functions
            .iter()
            .map(|f| f.chunk.n_global_cache_slots as usize)
            .sum();
        ensure_cycle_gc_wired();
        VM {
            stack: Vec::with_capacity(256),
            frames: Vec::with_capacity(64),
            globals,
            base_functions: functions.clone(),
            functions,
            inline_cache: vec![(u32::MAX, 0, CachedGlobal::Plain(Value::nil())); total_cache_slots],
            native_fns,
            debug_values: HashMap::new(),
            next_debug_value_ref: DEBUG_VALUE_REF_BASE,
            gc_adopted_home: std::cell::RefCell::new(Weak::new()),
            instruction_budget: None,
            instructions_executed: 0,
            pending_resume_error: None,
            quantum_cancellation: CancellationView::default(),
            native_signal: None,
        }
    }

    /// Create a new VM for an async task, sharing globals and functions with the parent.
    pub fn new_for_task(
        globals: Rc<Env>,
        functions: Rc<Vec<Rc<Function>>>,
        native_spurs: &[Spur],
    ) -> Result<Self, SemaError> {
        let native_fns = Self::resolve_native_table(&globals, native_spurs)?;
        let total_cache_slots: usize = functions
            .iter()
            .map(|f| f.chunk.n_global_cache_slots as usize)
            .sum();
        ensure_cycle_gc_wired();
        Ok(VM {
            stack: Vec::with_capacity(256),
            frames: Vec::with_capacity(64),
            globals,
            base_functions: functions.clone(),
            functions,
            inline_cache: vec![(u32::MAX, 0, CachedGlobal::Plain(Value::nil())); total_cache_slots],
            native_fns: Rc::new(native_fns),
            debug_values: HashMap::new(),
            next_debug_value_ref: DEBUG_VALUE_REF_BASE,
            gc_adopted_home: std::cell::RefCell::new(Weak::new()),
            instruction_budget: None,
            instructions_executed: 0,
            pending_resume_error: None,
            quantum_cancellation: CancellationView::default(),
            native_signal: None,
        })
    }

    pub fn new_for_task_with_native_fns(
        globals: Rc<Env>,
        functions: Rc<Vec<Rc<Function>>>,
        native_fns: Rc<Vec<Rc<NativeFn>>>,
    ) -> Self {
        Self::new_with_rc_functions(globals, functions, native_fns)
    }

    /// Re-target an IDLE VM (no frames — the prior call, if any, already ran to
    /// completion) at a different closure's home env / function table / native
    /// table, reusing its `stack`/`frames`/`inline_cache` heap allocations
    /// instead of building a fresh `VM`. Used by the runtime's in-place HOF
    /// callback fast path (`invoke_vm_callback_loop`) to keep one scratch VM
    /// alive across unrelated cooperative-HOF invocations rather than
    /// allocating one per element (the cost this fast path exists to kill).
    pub fn reset_for_task_with_native_fns(
        &mut self,
        globals: Rc<Env>,
        functions: Rc<Vec<Rc<Function>>>,
        native_fns: Rc<Vec<Rc<NativeFn>>>,
    ) {
        debug_assert!(
            self.frames.is_empty() && self.stack.is_empty(),
            "reset_for_task_with_native_fns called on a VM with an in-flight frame"
        );
        self.stack.clear();
        self.frames.clear();
        let total_cache_slots: usize = functions
            .iter()
            .map(|f| f.chunk.n_global_cache_slots as usize)
            .sum();
        self.inline_cache.clear();
        self.inline_cache.resize(
            total_cache_slots,
            (u32::MAX, 0, CachedGlobal::Plain(Value::nil())),
        );
        self.globals = globals;
        self.base_functions = functions.clone();
        self.functions = functions;
        self.native_fns = native_fns;
        self.debug_values.clear();
        self.next_debug_value_ref = DEBUG_VALUE_REF_BASE;
        *self.gc_adopted_home.borrow_mut() = Weak::new();
        self.instruction_budget = None;
        self.instructions_executed = 0;
        self.pending_resume_error = None;
        self.quantum_cancellation = CancellationView::default();
        self.native_signal = None;
    }

    pub fn execute(&mut self, closure: Rc<Closure>, ctx: &EvalContext) -> Result<Value, SemaError> {
        ensure_legacy_vm_entry_allowed(ctx)?;
        self.ensure_cache_space(&closure.func);
        let base = self.stack.len();
        // Reserve space for locals
        let n_locals = closure.func.chunk.n_locals as usize;
        self.stack.resize(base + n_locals, Value::nil());
        self.frames.push(CallFrame {
            cache_base: closure.func.cache_offset,
            closure,
            pc: 0,
            base,
            open_upvalues: None,
        });
        self.run(ctx)
    }

    pub fn execute_debug(
        &mut self,
        closure: Rc<Closure>,
        ctx: &EvalContext,
        debug: &mut crate::debug::DebugState,
    ) -> Result<Value, SemaError> {
        ensure_legacy_vm_entry_allowed(ctx)?;
        self.ensure_cache_space(&closure.func);
        let base = self.stack.len();
        let n_locals = closure.func.chunk.n_locals as usize;
        self.stack.resize(base + n_locals, Value::nil());
        self.frames.push(CallFrame {
            cache_base: closure.func.cache_offset,
            closure,
            pc: 0,
            base,
            open_upvalues: None,
        });

        // Register this DebugState as the active session so the async scheduler
        // (reached via the RUN_SCHEDULER_CALLBACK seam during a native call) can
        // run task steps in debug mode and stop/resume on a mid-task breakpoint.
        // Popped on return/panic by the guard's Drop.
        let _active = ActiveDebugGuard::enter(debug);

        loop {
            let step = match self.run_inner::<true>(ctx, Some(debug)) {
                Ok(step) => step,
                Err(e) => {
                    // Uncaught runtime error. If the exception breakpoint filter
                    // is enabled, stop and let the user inspect before the
                    // session ends; the program cannot resume past an uncaught
                    // error, so any resume/disconnect command just propagates it.
                    // Note: by the time the error reaches here the VM has already
                    // unwound its frames, so stack/variable inspection at the
                    // exception stop is best-effort (typically empty). The error
                    // message itself is delivered via the Stopped description and
                    // the exceptionInfo request.
                    if debug.break_on_uncaught {
                        debug.last_exception = Some(e.to_string());
                        self.debug_values.clear();
                        self.next_debug_value_ref = DEBUG_VALUE_REF_BASE;
                        let _ = debug.event_tx.send(crate::debug::DebugEvent::Stopped {
                            reason: crate::debug::StopReason::Exception,
                            description: Some(e.to_string()),
                        });
                        self.debug_exception_park(ctx, debug);
                    }
                    return Err(e);
                }
            };
            match step {
                crate::debug::VmExecResult::Finished(v) => return Ok(v),
                crate::debug::VmExecResult::Yielded => continue,
                crate::debug::VmExecResult::QuantumExpired { .. } => {
                    unreachable!("debug execution does not install a runtime quantum")
                }
                crate::debug::VmExecResult::Pending(_) => {
                    return Err(SemaError::eval(
                        "async yield outside of scheduler context".to_string(),
                    ));
                }
                crate::debug::VmExecResult::Stopped(info) => {
                    match self.handle_debug_stop(ctx, debug, info) {
                        DebugStopResume::Resume => {}
                        DebugStopResume::Disconnect => return Ok(Value::nil()),
                    }
                }
            }
        }
    }

    /// Handle a `Stopped` mid-execution: reset variable handles, emit the
    /// `Stopped` event, then block on `command_rx` serving inspection requests
    /// until a resume/step/disconnect arrives. Shared by the main VM debug loop
    /// (`execute_debug`) and the async scheduler's debug task step, so a breakpoint
    /// hit inside a task stops/resumes with the SAME loop — and the inspection
    /// commands (GetStackTrace/GetScopes/GetVariables/Evaluate) target `self`,
    /// which is the STOPPED task's VM in the scheduler case.
    ///
    /// Returns [`DebugStopResume::Resume`] to continue execution (the step mode was
    /// set per the resume command) or [`DebugStopResume::Disconnect`] to terminate.
    pub fn handle_debug_stop(
        &mut self,
        ctx: &EvalContext,
        debug: &mut crate::debug::DebugState,
        info: crate::debug::StopInfo,
    ) -> DebugStopResume {
        // Per the DAP spec, variablesReferences are only valid until the next stop.
        // Reset the handle map on each stop so Values expanded in prior stops are
        // not retained for the whole session (otherwise debug_values grows
        // unbounded across a long stepping session, pinning the underlying heap).
        self.debug_values.clear();
        self.next_debug_value_ref = DEBUG_VALUE_REF_BASE;

        let _ = debug.event_tx.send(crate::debug::DebugEvent::Stopped {
            reason: info.reason,
            description: None,
        });

        loop {
            match debug.command_rx.recv() {
                Ok(crate::debug::DebugCommand::Continue) => {
                    debug.step_mode = crate::debug::StepMode::Continue;
                    return DebugStopResume::Resume;
                }
                Ok(crate::debug::DebugCommand::StepInto) => {
                    debug.step_mode = crate::debug::StepMode::StepInto;
                    debug.step_frame_depth = self.frames.len();
                    return DebugStopResume::Resume;
                }
                Ok(crate::debug::DebugCommand::StepOver) => {
                    debug.step_mode = crate::debug::StepMode::StepOver;
                    debug.step_frame_depth = self.frames.len();
                    return DebugStopResume::Resume;
                }
                Ok(crate::debug::DebugCommand::StepOut) => {
                    debug.step_mode = crate::debug::StepMode::StepOut;
                    debug.step_frame_depth = self.frames.len();
                    return DebugStopResume::Resume;
                }
                Ok(crate::debug::DebugCommand::Pause) => {}
                Ok(crate::debug::DebugCommand::SetBreakpoints {
                    file,
                    breakpoints,
                    reply,
                }) => {
                    let ids = debug.set_breakpoints_with_conditions(&file, &breakpoints);
                    let _ = reply.send(ids);
                }
                Ok(crate::debug::DebugCommand::SetExceptionBreakpoints { break_on_uncaught }) => {
                    debug.break_on_uncaught = break_on_uncaught;
                }
                Ok(crate::debug::DebugCommand::GetStackTrace { reply }) => {
                    let _ = reply.send(self.debug_stack_trace());
                }
                Ok(crate::debug::DebugCommand::GetScopes { frame_id, reply }) => {
                    let _ = reply.send(self.debug_scopes(frame_id));
                }
                Ok(crate::debug::DebugCommand::GetVariables { reference, reply }) => {
                    let _ = reply.send(self.debug_variables(reference));
                }
                Ok(crate::debug::DebugCommand::Evaluate {
                    frame_id,
                    expression,
                    reply,
                }) => {
                    let result = sema_reader::read(&expression)
                        .map_err(|e| e.to_string())
                        .and_then(|expr| {
                            self.debug_evaluate_mut(frame_id, &expr, ctx, debug)
                                .map(|value| self.debug_value_to_variable("result", value))
                                .map_err(|e| e.to_string())
                        });
                    let _ = reply.send(result);
                }
                Ok(crate::debug::DebugCommand::SetVariable {
                    variables_reference,
                    name,
                    value_expression,
                    reply,
                }) => {
                    let result = self.debug_set_variable_expression(
                        variables_reference,
                        &name,
                        &value_expression,
                        ctx,
                        debug,
                    );
                    let _ = reply.send(result);
                }
                Ok(crate::debug::DebugCommand::Disconnect) => {
                    return DebugStopResume::Disconnect;
                }
                Err(_) => {
                    debug.step_mode = crate::debug::StepMode::Continue;
                    return DebugStopResume::Resume;
                }
            }
        }
    }

    /// Park after an uncaught exception, serving inspection requests until the
    /// user resumes or disconnects (or the command channel closes). Unlike the
    /// normal stop loop, the program cannot continue past an uncaught error, so
    /// any resume command simply releases the park and the caller propagates the
    /// error to terminate the session.
    pub fn debug_exception_park(
        &mut self,
        ctx: &EvalContext,
        debug: &mut crate::debug::DebugState,
    ) {
        loop {
            match debug.command_rx.recv() {
                Ok(crate::debug::DebugCommand::SetBreakpoints {
                    file,
                    breakpoints,
                    reply,
                }) => {
                    let ids = debug.set_breakpoints_with_conditions(&file, &breakpoints);
                    let _ = reply.send(ids);
                }
                Ok(crate::debug::DebugCommand::SetExceptionBreakpoints { break_on_uncaught }) => {
                    debug.break_on_uncaught = break_on_uncaught;
                }
                Ok(crate::debug::DebugCommand::GetStackTrace { reply }) => {
                    let _ = reply.send(self.debug_stack_trace());
                }
                Ok(crate::debug::DebugCommand::GetScopes { frame_id, reply }) => {
                    let _ = reply.send(self.debug_scopes(frame_id));
                }
                Ok(crate::debug::DebugCommand::GetVariables { reference, reply }) => {
                    let _ = reply.send(self.debug_variables(reference));
                }
                Ok(crate::debug::DebugCommand::Evaluate {
                    frame_id,
                    expression,
                    reply,
                }) => {
                    let result = sema_reader::read(&expression)
                        .map_err(|e| e.to_string())
                        .and_then(|expr| {
                            self.debug_evaluate_mut(frame_id, &expr, ctx, debug)
                                .map(|value| self.debug_value_to_variable("result", value))
                                .map_err(|e| e.to_string())
                        });
                    let _ = reply.send(result);
                }
                Ok(crate::debug::DebugCommand::SetVariable { reply, .. }) => {
                    let _ = reply.send(Err(
                        "setVariable is unavailable after an uncaught exception".to_string(),
                    ));
                }
                // Any resume command, a disconnect, or a closed channel ends the
                // park; the caller then propagates the uncaught error.
                Ok(crate::debug::DebugCommand::Continue)
                | Ok(crate::debug::DebugCommand::StepInto)
                | Ok(crate::debug::DebugCommand::StepOver)
                | Ok(crate::debug::DebugCommand::StepOut)
                | Ok(crate::debug::DebugCommand::Pause)
                | Ok(crate::debug::DebugCommand::Disconnect)
                | Err(_) => break,
            }
        }
    }

    /// Number of active call frames.
    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    pub(crate) fn active_globals(&self) -> Rc<Env> {
        self.globals.clone()
    }

    fn run(&mut self, ctx: &EvalContext) -> Result<Value, SemaError> {
        ensure_legacy_vm_entry_allowed(ctx)?;
        match self.run_inner::<false>(ctx, None)? {
            crate::debug::VmExecResult::Finished(v) => Ok(v),
            crate::debug::VmExecResult::Stopped(_) | crate::debug::VmExecResult::Yielded => {
                unreachable!("Stopped/Yielded without debug state")
            }
            crate::debug::VmExecResult::QuantumExpired { .. } => {
                unreachable!("unbounded VM execution cannot exhaust a quantum")
            }
            crate::debug::VmExecResult::Pending(_) => Err(SemaError::eval(
                "async yield outside of scheduler context".to_string(),
            )),
        }
    }

    /// Resume execution for at most `instruction_limit` bytecode instructions.
    ///
    /// `cancellation` is the task's cancellation snapshot for this quantum; it is
    /// handed to every native dispatched via the runtime ABI (through
    /// [`NativeCallContext`]) and reset when the quantum ends. Non-runtime callers
    /// pass [`CancellationView::default`].
    pub fn run_quantum(
        &mut self,
        ctx: &EvalContext,
        instruction_limit: usize,
        cancellation: CancellationView,
    ) -> crate::debug::VmQuantumResult {
        self.instruction_budget = Some(instruction_limit);
        self.instructions_executed = 0;
        self.quantum_cancellation = cancellation;
        let outcome = self.run_inner::<false>(ctx, None);
        let instructions = self.instructions_executed;
        self.instruction_budget = None;
        self.quantum_cancellation = CancellationView::default();
        crate::debug::VmQuantumResult {
            outcome,
            instructions,
        }
    }

    /// Debug-aware sibling of [`run_quantum`]: identical instruction-budgeted
    /// quantum, but running the debug interpreter (`run_inner::<true>`) with the
    /// breakpoint / condition / step-mode / exception machinery live.
    ///
    /// A breakpoint (or step) stop is served RIGHT HERE, inside the quantum, via
    /// [`handle_debug_stop`]: the driving thread parks on `debug.command_rx`
    /// answering GetStackTrace/GetScopes/GetVariables/Evaluate/SetVariable
    /// against `self` — the stopped task's own VM — and resumes back into
    /// `run_inner::<true>` on a Continue/step command. This is the native-DAP
    /// stop-the-world barrier: nothing else on the interpreter thread runs while
    /// parked (`run_parked_quantum` holds NO `RuntimeState` borrow across the
    /// quantum, so blocking here cannot deadlock the state cell). A Disconnect
    /// ends the task's quantum with nil (the DAP backend tears the session down).
    ///
    /// `break_on_uncaught` is intentionally NOT handled here: an `Err` from the
    /// debug interpreter is uncaught only within THIS task's frames — a parent
    /// awaiting the task may still catch the rejection — so the uncaught-exception
    /// park is done by the host once the ROOT settles Failed (see the DAP backend).
    pub fn run_quantum_debug(
        &mut self,
        ctx: &EvalContext,
        instruction_limit: usize,
        cancellation: CancellationView,
        debug: &mut crate::debug::DebugState,
    ) -> crate::debug::VmQuantumResult {
        self.instruction_budget = Some(instruction_limit);
        self.instructions_executed = 0;
        self.quantum_cancellation = cancellation;
        let outcome = loop {
            match self.run_inner::<true>(ctx, Some(debug)) {
                Ok(crate::debug::VmExecResult::Stopped(info)) => {
                    // Cooperative (wasm/headless) session: there is no command
                    // channel to block on. SURFACE the stop out of the quantum as
                    // `Stopped(info)` so `run_parked_quantum` parks the task, arms
                    // the runtime-wide debug barrier, and returns
                    // `TaskAction::DebugStop`; the host resumes via `debug_resume`.
                    if debug.is_headless() {
                        break Ok(crate::debug::VmExecResult::Stopped(info));
                    }
                    // Blocking (native DAP) session: serve inspection right here.
                    match self.handle_debug_stop(ctx, debug, info) {
                        DebugStopResume::Resume => continue,
                        DebugStopResume::Disconnect => {
                            break Ok(crate::debug::VmExecResult::Finished(Value::nil()));
                        }
                    }
                }
                other => break other,
            }
        };
        let instructions = self.instructions_executed;
        self.instruction_budget = None;
        self.quantum_cancellation = CancellationView::default();
        crate::debug::VmQuantumResult {
            outcome,
            instructions,
        }
    }

    /// Dispatch a native function through the canonical runtime ABI.
    ///
    /// When a [`TaskContext`](sema_core::runtime::TaskContext) is installed (a
    /// runtime quantum), the native is invoked via
    /// [`NativeFn::invoke_runtime`] under a fresh [`NativeCallContext`] built from
    /// the quantum's cancellation snapshot, mirroring the runtime's own
    /// `invoke_callable`. A `Return` is a plain value; any other outcome is
    /// structural ([`NativeDispatchResult::Pending`]). Outside a runtime quantum
    /// (nested, synchronous, or wasm entry) the native runs through the legacy
    /// value ABI.
    ///
    /// The native receives a cloned task-context handle and borrows it only for
    /// individual task-local operations, so re-entrant setup can access the same
    /// context without an invocation-wide `RefMut`.
    ///
    /// The runtime ABI is spoken only during a live runtime quantum. All
    /// synchronous fresh-VM and callback adapters reject while that quantum is
    /// active, so an unresolved task-owned wait cannot be bypassed by value-ABI
    /// re-entry.
    fn dispatch_native(
        &mut self,
        func: &Rc<NativeFn>,
        call_args: &[Value],
        ctx: &EvalContext,
    ) -> Result<NativeDispatchResult, SemaError> {
        if ctx.runtime_quantum_active() {
            if let Some(handle) = ctx.task_context() {
                let _installed = ctx.scope_task_context(handle.clone());
                let mut native_ctx = NativeCallContext {
                    eval_context: ctx,
                    task_context: handle,
                    call_env: Some(self.globals.clone()),
                    cancellation: self.quantum_cancellation.clone(),
                };
                let outcome = {
                    snapshot_native_escaping_args(self, func, call_args);
                    func.invoke_runtime(&mut native_ctx, call_args)
                }?;
                drop(_installed);
                return Ok(match outcome {
                    NativeOutcome::Return(value) => NativeDispatchResult::Value(value),
                    other => NativeDispatchResult::Pending(VmPendingOutcome::from_outcome(other)),
                });
            }
        }
        let result = {
            let _call_env = ctx.scope_legacy_call_env(&self.globals);
            snapshot_escaping_args_with_owner(self, call_args);
            (func.func)(ctx, call_args)
        };
        self.sync_tracked_upvalues_to_stack();
        let value = result?;
        Ok(NativeDispatchResult::Value(value))
    }

    /// Land a helper-mediated native dispatch result onto the stack: a value is
    /// pushed as the call result; a suspension pushes a nil placeholder (the
    /// resume value overwrites it) and stashes the signal in `native_signal` for
    /// the owning opcode arm to park on.
    fn stash_native_dispatch(&mut self, result: NativeDispatchResult) {
        match result {
            NativeDispatchResult::Value(value) => self.stack.push(value),
            NativeDispatchResult::Pending(outcome) => {
                self.stack.push(Value::nil());
                self.native_signal = Some(VmNativeSignal::Pending(outcome));
            }
        }
    }

    /// Replace the top of the stack with a value.
    /// Used by the scheduler to set the resume value before continuing
    /// a yielded task (the yield left a nil placeholder on the stack).
    pub fn replace_stack_top(&mut self, val: Value) {
        if let Some(top) = self.stack.last_mut() {
            *top = val;
        }
    }

    /// Refresh this VM's live-frame stack slots from any `Tracked` upvalue cells
    /// that captured them.
    ///
    /// A `Tracked` cell (a captured local closed for a FOREIGN run — an
    /// `async/spawn` task or a cooperative HOF callback VM) owns its value out of
    /// band while its defining frame stays live. Writes performed on the foreign
    /// VM land in `cell.value`, but the defining frame reads the local through
    /// `LOAD_LOCAL` (its stack slot), which the foreign write never touched. When
    /// the parked parent VM resumes it must observe those writes, so copy each
    /// live frame's `Tracked` cell value back into its stack slot. The cell stays
    /// `Tracked` (a later foreign run may write again; `propagate_local_store_to_
    /// tracked` keeps them in step on subsequent owner writes), and frame exit
    /// still finalizes with the authoritative tracked value.
    pub fn sync_tracked_upvalues_to_stack(&mut self) {
        let mut updates: Vec<(usize, Value)> = Vec::new();
        for frame in &self.frames {
            let base = frame.base;
            let Some(open) = &frame.open_upvalues else {
                continue;
            };
            for (slot, cell) in open.iter().enumerate() {
                let Some(cell) = cell else { continue };
                if let UpvalueState::Tracked { value, .. } = &*cell.state.borrow() {
                    updates.push((base + slot, value.clone()));
                }
            }
        }
        for (idx, value) in updates {
            if let Some(dst) = self.stack.get_mut(idx) {
                *dst = value;
            }
        }
    }

    /// Arm a rejection resume for a frame parked on a structural suspend.
    ///
    /// Rejection counterpart to [`replace_stack_top`]: instead of injecting a
    /// value onto the parked frame's stack top, the next `run_inner` raises
    /// `err` at the parked call site — as if the yielding native had returned
    /// `Err(err)` — so the VM's exception machinery (try/catch, exception
    /// tables) runs. Handled → the frame resumes in its catch handler; uncaught
    /// → the error propagates out of `run_quantum` as an ordinary `Err`.
    pub fn resume_with_error(&mut self, err: SemaError) {
        self.pending_resume_error = Some(err);
    }

    /// Seed the main call frame for `closure` without executing it, so the
    /// unified runtime can drive this VM as a task through `run_quantum`. Mirrors
    /// the frame setup in `execute_async`, but performs no evaluation — the first
    /// `run_quantum` runs the top-level chunk from its entry point.
    pub fn seed_main_frame(&mut self, closure: Rc<Closure>) {
        self.ensure_cache_space(&closure.func);
        let base = self.stack.len();
        let n_locals = closure.func.chunk.n_locals as usize;
        self.stack.resize(base + n_locals, Value::nil());
        self.frames.push(CallFrame {
            cache_base: closure.func.cache_offset,
            closure,
            pc: 0,
            base,
            open_upvalues: None,
        });
    }

    /// Prepare the VM to run `closure` with `args` already bound to its
    /// parameters, but do not run it. The scheduler uses this to register
    /// a closure-as-task whose first call will be `run_async`.
    pub fn setup_for_call(
        &mut self,
        closure: Rc<Closure>,
        args: &[Value],
    ) -> Result<(), SemaError> {
        self.setup_for_call_args(closure, CallArgs::Borrowed(args))
    }

    /// Prepare a callback call by moving each argument into its VM local slot.
    /// The unified runtime owns `NativeCall::args`, so its cooperative callback
    /// handoff can preserve uniqueness-sensitive accumulator fast paths instead
    /// of retaining a second reference for the duration of the call.
    pub(crate) fn setup_for_call_owned(
        &mut self,
        closure: Rc<Closure>,
        args: &mut [Value],
    ) -> Result<(), SemaError> {
        self.setup_for_call_args(closure, CallArgs::Owned(args))
    }

    /// [`Self::setup_for_call`] parametrized over the args handoff: `Borrowed`
    /// clones each value into its slot, `Owned` moves it out of the caller's
    /// buffer (leaving nil) so the slot holds the value's only new reference.
    fn setup_for_call_args(
        &mut self,
        closure: Rc<Closure>,
        args: CallArgs,
    ) -> Result<(), SemaError> {
        let func = &closure.func;
        let arity = func.arity as usize;
        let has_rest = func.has_rest;
        let n_locals = func.chunk.n_locals as usize;
        let argc = args.len();

        if has_rest {
            if argc < arity {
                return Err(SemaError::arity(
                    func.name
                        .map(resolve_spur)
                        .unwrap_or_else(|| "<lambda>".to_string()),
                    format!("{}+", arity),
                    argc,
                ));
            }
        } else if argc != arity {
            return Err(SemaError::arity(
                func.name
                    .map(resolve_spur)
                    .unwrap_or_else(|| "<lambda>".to_string()),
                arity.to_string(),
                argc,
            ));
        }

        self.ensure_cache_space(func);
        let base = self.stack.len();
        self.stack.resize(base + n_locals, Value::nil());
        match args {
            CallArgs::Borrowed(args) => {
                for i in 0..arity {
                    self.stack[base + i] = args.get(i).cloned().unwrap_or(Value::nil());
                }
                if has_rest {
                    let rest: Vec<Value> = args[arity..].to_vec();
                    self.stack[base + arity] = Value::list(rest);
                }
            }
            CallArgs::Owned(args) => {
                for (i, arg) in args.iter_mut().enumerate().take(arity) {
                    self.stack[base + i] = std::mem::replace(arg, Value::nil());
                }
                if has_rest {
                    let rest: Vec<Value> = args[arity..]
                        .iter_mut()
                        .map(|v| std::mem::replace(v, Value::nil()))
                        .collect();
                    self.stack[base + arity] = Value::list(rest);
                }
            }
        }
        self.frames.push(CallFrame {
            cache_base: func.cache_offset,
            closure,
            pc: 0,
            base,
            open_upvalues: None,
        });
        Ok(())
    }

    /// Core dispatch loop, monomorphized over `DEBUG`. The `DEBUG = false`
    /// instantiation compiles the per-instruction debug hook (breakpoints,
    /// command polling, span tracking) out entirely, keeping the release
    /// path's registers and i-cache free of it; every debug-session entry
    /// point routes through `DEBUG = true`. Callers must pass `debug: None`
    /// when `DEBUG` is `false`.
    fn run_inner<const DEBUG: bool>(
        &mut self,
        ctx: &EvalContext,
        mut debug: Option<&mut crate::debug::DebugState>,
    ) -> Result<crate::debug::VmExecResult, SemaError> {
        debug_assert!(
            DEBUG || debug.is_none(),
            "run_inner::<false> must not receive a DebugState"
        );
        // Raw-pointer macros for reading operands without bounds checks in inner loop
        //
        // SAFETY for read_u16!/read_u32!/read_i32!: $pc..$pc+N must be in-bounds
        // for $code. The in-process emitter (sema-vm/src/lower.rs) emits complete
        // instructions where every opcode is followed by its full operand bytes.
        // Deserialized bytecode is validated by advance_pc in serialize.rs, which
        // rejects truncated chunks. See FIXME(C11) above pop_unchecked for the
        // known gap with hand-crafted .semac files.
        macro_rules! read_u16 {
            ($code:expr, $pc:expr) => {{
                let v = unsafe { u16::from_le_bytes([*$code.add($pc), *$code.add($pc + 1)]) };
                $pc += 2;
                v
            }};
        }
        macro_rules! read_i32 {
            ($code:expr, $pc:expr) => {{
                let v = unsafe {
                    i32::from_le_bytes([
                        *$code.add($pc),
                        *$code.add($pc + 1),
                        *$code.add($pc + 2),
                        *$code.add($pc + 3),
                    ])
                };
                $pc += 4;
                v
            }};
        }
        macro_rules! read_u32 {
            ($code:expr, $pc:expr) => {{
                let v = unsafe {
                    u32::from_le_bytes([
                        *$code.add($pc),
                        *$code.add($pc + 1),
                        *$code.add($pc + 2),
                        *$code.add($pc + 3),
                    ])
                };
                $pc += 4;
                v
            }};
        }

        // Unsafe unchecked pop — valid when the bytecode is stack-balanced.
        //
        // SAFETY (C11 closed): every chunk reaching the VM is stack-balanced.
        // In-process bytecode is balanced by construction (the compiler emits
        // matched push/pop sequences). Deserialized `.semac` bytecode is proven
        // balanced by the abstract stack-depth verifier in
        // `crate::serialize::verify_stack_balance`, which runs inside
        // `validate_bytecode` before `deserialize_from_bytes` returns and rejects
        // any chunk where a reachable opcode could pop from an empty operand
        // stack. That verifier is the safety guarantee for this `set_len` /
        // `ptr::read`; the `debug_assert!` below catches verifier/dispatch drift
        // in debug builds. See `docs/adr.md` ADR #56 and `docs/limitations.md` #32.
        #[inline(always)]
        unsafe fn pop_unchecked(stack: &mut Vec<Value>) -> Value {
            let len = stack.len();
            debug_assert!(len > 0, "pop_unchecked on empty stack");
            let v = std::ptr::read(stack.as_ptr().add(len - 1));
            stack.set_len(len - 1);
            v
        }

        // Branchless sign-extension shift for NaN-boxed small ints
        const SIGN_SHIFT: u32 = 64 - NAN_PAYLOAD_BITS;

        // Cold-path macro: saves pc, handles exception, and dispatches or returns.
        // Keeps the error path out of the hot instruction sequence.
        macro_rules! handle_err {
            ($self:expr, $fi:expr, $pc:expr, $err:expr, $saved_pc:expr, $label:tt) => {{
                $self.frames[$fi].pc = $pc;
                match $self.handle_exception($err, $saved_pc)? {
                    ExceptionAction::Handled => continue $label,
                    ExceptionAction::Propagate(e) => return Err(e),
                }
            }};
        }

        // Snapshot the VM's base function table — the table used by the
        // top-level main closure (and any closure carrying no explicit table).
        // `self.functions` is reset from each frame's closure on every frame
        // activation; this immutable snapshot is the fallback for `None`
        // closures so it never observes a cross-module callee's swapped table
        // (M4: import on the VM).
        let base_functions = self.base_functions.clone();
        // Snapshot the VM's base globals — the env the top-level main closure
        // (which carries no explicit home env) resolves against. `self.globals`
        // is kept pointing at the running frame's home env (below); this
        // immutable snapshot is the fallback for `None` closures (M1).
        let base_globals = self.globals.clone();

        // Rejection resume: a frame parked on a structural suspend is being
        // re-run because its awaited promise settled Failed. The park left a nil
        // placeholder on the stack top (the awaited value's slot) and advanced
        // pc past the yielding call. Discard that placeholder and raise the
        // error at the parked call site, mirroring the native-`Err` path
        // (`handle_err!` → `handle_exception`): if a handler catches it the
        // frame resumes in its `catch`; otherwise it propagates out as `Err`.
        // `failing_pc` is `pc - 1` so the exception-table lookup lands inside
        // the (half-open) call instruction interval rather than at the resume
        // pc just past it — the same adjustment `handle_exception` applies when
        // unwinding into a parent frame.
        if let Some(err) = self.pending_resume_error.take() {
            if !self.stack.is_empty() {
                self.stack.pop();
            }
            let failing_pc = self
                .frames
                .last()
                .map(|f| f.pc.saturating_sub(1))
                .unwrap_or(0);
            match self.handle_exception(err, failing_pc)? {
                ExceptionAction::Handled => {}
                ExceptionAction::Propagate(e) => return Err(e),
            }
        }

        // Register-local instruction countdown for the dispatch loop's budget
        // check. The naive per-opcode check (`if let Some(budget) =
        // self.instruction_budget { ... self.instructions_executed += 1 }`)
        // pays an `Option` load plus two `self` field accesses on every single
        // instruction. `InstrCountdownGuard` hoists that into one `usize`
        // local (`remaining`) that the hot loop only compares to zero and
        // decrements; the unbudgeted case sets `remaining` to `usize::MAX` so
        // the same branch-free code path serves both (reaching zero from
        // `usize::MAX` would take billions of years, so the budget check is
        // never actually observed when no budget is installed).
        //
        // `self.instructions_executed` is read by `run_quantum`/
        // `run_quantum_debug` and by the nested-callback save/restore
        // (`self.instructions_executed` around the synchronous re-entry path)
        // immediately after `run_inner` returns — including on `Err`. Rather
        // than hand-editing every `return` in this function (Return/Finished,
        // every `handle_err!`/`?`-propagated `Err`, every Pending/Stopped/
        // Yielded suspend, QuantumExpired itself, and the DEBUG
        // instantiation's Disconnect/breakpoint-stop exits), the guard's
        // `Drop` reconciles `self.instructions_executed` from the countdown
        // on every exit from `run_inner`, however it returns.
        struct InstrCountdownGuard {
            // SAFETY: points at `self.instructions_executed`, valid for the
            // lifetime of this `run_inner` activation — the VM is not moved
            // while borrowed `&mut` by the caller. A raw pointer (rather than
            // a `&mut` field borrow) is required because `self` is used
            // pervasively elsewhere in this function body (`self.frames`,
            // `self.handle_exception`, …), which a live `&mut` borrow of one
            // field would conflict with under the borrow checker.
            target: *mut usize,
            has_budget: bool,
            start_executed: usize,
            start_remaining: usize,
            remaining: usize,
        }
        impl Drop for InstrCountdownGuard {
            #[inline]
            fn drop(&mut self) {
                if self.has_budget {
                    let consumed = self.start_remaining - self.remaining;
                    unsafe {
                        *self.target = self.start_executed + consumed;
                    }
                }
            }
        }
        let mut instr_guard = {
            let start_executed = self.instructions_executed;
            let (has_budget, start_remaining) = match self.instruction_budget {
                Some(budget) => (true, budget.saturating_sub(start_executed)),
                None => (false, usize::MAX),
            };
            InstrCountdownGuard {
                target: &mut self.instructions_executed as *mut usize,
                has_budget,
                start_executed,
                start_remaining,
                remaining: start_remaining,
            }
        };

        // Two-level dispatch: outer loop caches frame locals, inner loop dispatches opcodes.
        // We only break to the outer loop when frames change (Call/TailCall/Return/exceptions).
        let mut debug_poll_counter: u32 = 0;
        // Track whether we've been through at least one dispatch iteration.
        // On frame transitions (Call/TailCall/Return), resume_skip is cleared
        // so breakpoints re-trigger on new loop iterations.
        let mut dispatch_count: u32 = 0;
        'dispatch: loop {
            if DEBUG && dispatch_count > 0 {
                if let Some(ref mut dbg) = debug {
                    dbg.resume_skip = false;
                }
            }
            dispatch_count += 1;
            // Loop/recursion guard on every frame transition. Catches infinite
            // tail recursion like `(define (loop) (loop))` (re-enters here on
            // every TailCall) and honors step-limit / deadline / cancellation.
            ctx.check_loop_interrupt()?;
            let fi = self.frames.len() - 1;
            let frame = &self.frames[fi];
            // `mut` because the frame-preserving native-call fast paths
            // (CallNative / CallGlobal's Native arm) re-derive these from the
            // frame after the call instead of keeping them live across it —
            // that keeps the dispatch loop's register pressure flat.
            let mut code = frame.closure.func.chunk.code.as_ptr();
            let mut consts: *const [Value] = frame.closure.func.chunk.consts.as_slice();
            let mut base = frame.base;
            let mut pc = frame.pc;
            let mut code_len = frame.closure.func.chunk.code.len();
            // Point `self.globals` and `self.functions` at the running closure's
            // home env / function table (a `None` closure — the top-level main
            // closure — uses the VM's base snapshots). The hot global opcodes
            // read `self.globals` directly, so this pays no per-instruction cost.
            // Skip the Rc clone when already current (the common same-VM case):
            // tak-style call-dense code keeps the same env/table across millions
            // of frames, so the ptr-eq guard avoids needless refcount churn while
            // an imported (cross-module) closure still gets its own env/table
            // restored, and the caller regains theirs on return (M1 + M4).
            match &frame.closure.globals {
                Some(g) if !Rc::ptr_eq(g, &self.globals) => {
                    let g = g.clone();
                    self.globals = g;
                }
                None if !Rc::ptr_eq(&self.globals, &base_globals) => {
                    self.globals = base_globals.clone();
                }
                _ => {}
            }
            match &frame.closure.functions {
                Some(f) if !Rc::ptr_eq(f, &self.functions) => {
                    let f = f.clone();
                    self.functions = f;
                }
                None if !Rc::ptr_eq(&self.functions, &base_functions) => {
                    self.functions = base_functions.clone();
                }
                _ => {}
            }

            // Cache the next span boundary to avoid binary_search per instruction
            let (mut next_span_idx, mut next_span_pc) = if DEBUG && debug.is_some() {
                let spans = &frame.closure.func.chunk.spans;
                let idx = match spans.binary_search_by_key(&(pc as u32), |(p, _)| *p) {
                    Ok(i) => i,
                    Err(i) => i,
                };
                let npc = spans.get(idx).map(|(p, _)| *p).unwrap_or(u32::MAX);
                (idx, npc)
            } else {
                (0, u32::MAX)
            };

            let _ = frame; // release borrow so we can mutate self

            loop {
                // Provably dead for well-formed bytecode (the compiler
                // terminates every chunk with `Return` and patches jumps
                // in-chunk; `validate_bytecode` proves the same pc-bounds
                // invariants for loaded `.semac`) and measurably free: a
                // never-taken branch predicts perfectly, so an unchecked
                // fetch benches no faster (tak/nqueens, M2 Max). Kept as a
                // real check — it turns a VM bug into a clean error instead
                // of an out-of-bounds read.
                if pc >= code_len {
                    return Err(SemaError::eval(format!(
                        "VM: program counter out of bounds (pc={pc}, len={code_len})"
                    )));
                }
                if instr_guard.remaining == 0 {
                    self.frames[fi].pc = pc;
                    return Ok(crate::debug::VmExecResult::QuantumExpired {
                        instructions: instr_guard.start_executed + instr_guard.start_remaining,
                    });
                }
                instr_guard.remaining -= 1;
                let op = unsafe { *code.add(pc) };
                pc += 1;

                // Debug hook: span-cached check and command polling. Compiled
                // out entirely in the DEBUG = false instantiation.
                if DEBUG {
                    if let Some(ref mut dbg) = debug {
                        // Poll for Pause/Disconnect every 128 instructions
                        debug_poll_counter = debug_poll_counter.wrapping_add(1);
                        if debug_poll_counter & 127 == 0 {
                            while let Ok(cmd) = dbg.command_rx.try_recv() {
                                match cmd {
                                    crate::debug::DebugCommand::Pause => {
                                        dbg.pause_requested = true;
                                    }
                                    crate::debug::DebugCommand::Disconnect => {
                                        self.frames[fi].pc = pc;
                                        return Ok(crate::debug::VmExecResult::Finished(
                                            Value::nil(),
                                        ));
                                    }
                                    crate::debug::DebugCommand::SetBreakpoints {
                                        file,
                                        breakpoints,
                                        reply,
                                    } => {
                                        let ids = dbg
                                            .set_breakpoints_with_conditions(&file, &breakpoints);
                                        let _ = reply.send(ids);
                                    }
                                    crate::debug::DebugCommand::SetExceptionBreakpoints {
                                        break_on_uncaught,
                                    } => {
                                        dbg.break_on_uncaught = break_on_uncaught;
                                    }
                                    // State queries are valid while the program is
                                    // running. Reply with the current state instead
                                    // of dropping them: the DAP server blocks a
                                    // spawn_blocking thread on `reply_rx.recv()`, so
                                    // a dropped reply leaks that thread and hangs the
                                    // session (the `stackTrace`-while-running case).
                                    crate::debug::DebugCommand::GetStackTrace { reply } => {
                                        self.frames[fi].pc = pc; // sync live pc for the trace
                                        let _ = reply.send(self.debug_stack_trace());
                                    }
                                    crate::debug::DebugCommand::GetScopes { frame_id, reply } => {
                                        let _ = reply.send(self.debug_scopes(frame_id));
                                    }
                                    crate::debug::DebugCommand::GetVariables {
                                        reference,
                                        reply,
                                    } => {
                                        let _ = reply.send(self.debug_variables(reference));
                                    }
                                    crate::debug::DebugCommand::Evaluate { reply, .. } => {
                                        let _ = reply.send(Err(
                                            "evaluate is only available while execution is stopped"
                                                .to_string(),
                                        ));
                                    }
                                    crate::debug::DebugCommand::SetVariable { reply, .. } => {
                                        let _ = reply.send(Err(
                                        "setVariable is only available while execution is stopped"
                                            .to_string(),
                                    ));
                                    }
                                    // Step/Continue have no paused frame to act on
                                    // while running; ignore them.
                                    _ => {}
                                }
                            }
                        }

                        let op_pc = (pc - 1) as u32;
                        // Fast path: skip if not at a span boundary (single integer compare)
                        let at_span = if op_pc == next_span_pc {
                            let spans = &self.frames[fi].closure.func.chunk.spans;
                            let line = spans[next_span_idx].1.line as u32;
                            let file = self.frames[fi].closure.func.source_file.clone();
                            next_span_idx += 1;
                            next_span_pc = spans
                                .get(next_span_idx)
                                .map(|(p, _)| *p)
                                .unwrap_or(u32::MAX);
                            Some((file, line))
                        } else if op_pc > next_span_pc {
                            // Jumped past — resync via binary search
                            let spans = &self.frames[fi].closure.func.chunk.spans;
                            match spans.binary_search_by_key(&op_pc, |(p, _)| *p) {
                                Ok(i) => {
                                    let line = spans[i].1.line as u32;
                                    let file = self.frames[fi].closure.func.source_file.clone();
                                    next_span_idx = i + 1;
                                    next_span_pc = spans
                                        .get(next_span_idx)
                                        .map(|(p, _)| *p)
                                        .unwrap_or(u32::MAX);
                                    Some((file, line))
                                }
                                Err(i) => {
                                    next_span_idx = i;
                                    next_span_pc =
                                        spans.get(i).map(|(p, _)| *p).unwrap_or(u32::MAX);
                                    None
                                }
                            }
                        } else if next_span_idx > 0 {
                            // Check for backward jump: op_pc is before our current
                            // span window (e.g., loop back-edge). Resync via binary search.
                            // Clear resume_skip so breakpoints re-trigger on new iterations.
                            let spans = &self.frames[fi].closure.func.chunk.spans;
                            if op_pc <= spans[next_span_idx - 1].0 {
                                dbg.resume_skip = false;
                                match spans.binary_search_by_key(&op_pc, |(p, _)| *p) {
                                    Ok(i) => {
                                        let line = spans[i].1.line as u32;
                                        let file = self.frames[fi].closure.func.source_file.clone();
                                        next_span_idx = i + 1;
                                        next_span_pc = spans
                                            .get(next_span_idx)
                                            .map(|(p, _)| *p)
                                            .unwrap_or(u32::MAX);
                                        Some((file, line))
                                    }
                                    Err(i) => {
                                        next_span_idx = i;
                                        next_span_pc =
                                            spans.get(i).map(|(p, _)| *p).unwrap_or(u32::MAX);
                                        None
                                    }
                                }
                            } else {
                                None
                            }
                        } else {
                            None
                        };

                        if let Some((file, line)) = at_span {
                            if dbg.resume_skip {
                                // Keep skipping while on the same line as last stop.
                                // This prevents re-triggering breakpoints on multi-opcode lines.
                                let same_line = dbg
                                    .last_stop_line
                                    .as_ref()
                                    .is_some_and(|(_, last_line)| line == *last_line);
                                if !same_line {
                                    dbg.resume_skip = false;
                                }
                            }
                            if !dbg.resume_skip {
                                let frame_depth = self.frames.len();
                                if dbg.should_stop(file.as_ref(), line, frame_depth)
                                    && self.debug_condition_allows_stop(
                                        file.as_ref(),
                                        line,
                                        dbg,
                                        ctx,
                                    )
                                {
                                    self.frames[fi].pc = pc - 1;
                                    let reason = if dbg.pause_requested {
                                        crate::debug::StopReason::Pause
                                    } else if dbg.step_mode != crate::debug::StepMode::Continue {
                                        crate::debug::StopReason::Step
                                    } else {
                                        crate::debug::StopReason::Breakpoint
                                    };
                                    dbg.last_stop_line = file.as_ref().map(|f| (f.clone(), line));
                                    dbg.pause_requested = false;
                                    dbg.resume_skip = true;
                                    return Ok(crate::debug::VmExecResult::Stopped(
                                        crate::debug::StopInfo {
                                            reason,
                                            file: file.clone(),
                                            line,
                                        },
                                    ));
                                }
                            }
                        }

                        // Instruction budget yield check (for cooperative WASM execution).
                        // Checked every 128 instructions, after breakpoints so they take priority.
                        if debug_poll_counter & 127 == 0 && dbg.instructions_remaining > 0 {
                            dbg.instructions_remaining =
                                dbg.instructions_remaining.saturating_sub(128);
                            if dbg.instructions_remaining == 0 {
                                self.frames[fi].pc = pc - 1;
                                return Ok(crate::debug::VmExecResult::Yielded);
                            }
                        }
                    }
                }

                match op {
                    // --- Constants & stack ---
                    op::CONST => {
                        let idx = read_u16!(code, pc) as usize;
                        let val = unsafe { (&(*consts)).get_unchecked(idx) }.clone();
                        self.stack.push(val);
                    }
                    op::NIL => {
                        self.stack.push(Value::nil());
                    }
                    op::TRUE => {
                        self.stack.push(Value::bool(true));
                    }
                    op::FALSE => {
                        self.stack.push(Value::bool(false));
                    }
                    op::POP => {
                        unsafe { pop_unchecked(&mut self.stack) };
                    }
                    op::DUP => {
                        // `len() - 1` underflows to usize::MAX on an empty stack and
                        // reads far out of bounds (UB). A crafted .semac can declare a
                        // `max_stack` that doesn't match actual stack effects and lead
                        // with a bare DUP, so guard rather than trust the verifier.
                        let val = match self.stack.last() {
                            Some(v) => v.clone(),
                            None => {
                                return Err(SemaError::eval(
                                    "DUP on empty stack (corrupt or malicious bytecode)",
                                ))
                            }
                        };
                        self.stack.push(val);
                    }

                    // --- Locals ---
                    op::LOAD_LOCAL => {
                        let slot = read_u16!(code, pc) as usize;
                        self.stack.push(self.stack[base + slot].clone());
                    }
                    op::TAKE_LOCAL => {
                        // Moving load: the compiler proved this is the statically
                        // last use of a never-captured slot (takelocal.rs), so the
                        // slot ref is dead — move it instead of bumping the
                        // refcount, leaving nil behind. This is what lets the
                        // stdlib's strong_count==1 in-place fast paths fire.
                        let slot = read_u16!(code, pc) as usize;
                        let val = std::mem::replace(&mut self.stack[base + slot], Value::nil());
                        self.stack.push(val);
                    }
                    op::STORE_LOCAL => {
                        let slot = read_u16!(code, pc) as usize;
                        let val = unsafe { pop_unchecked(&mut self.stack) };
                        self.propagate_local_store_to_tracked(fi, slot, &val);
                        self.stack[base + slot] = val;
                    }

                    // --- Upvalues ---
                    op::LOAD_UPVALUE => {
                        let idx = read_u16!(code, pc) as usize;
                        let resolved = {
                            let state = self.frames[fi].closure.upvalues[idx].state.borrow();
                            match &*state {
                                UpvalueState::Closed(v) => Ok(v.clone()),
                                // Detached-but-live: read the owned value — safe
                                // on any VM stack (it no longer indexes a stack).
                                UpvalueState::Tracked { value, .. } => Ok(value.clone()),
                                UpvalueState::Open { frame_base, slot } => Err(*frame_base + *slot),
                            }
                        };
                        let val = match resolved {
                            Ok(v) => v,
                            // An Open cell indexes the stack of the VM that
                            // created it. Out of bounds means a closure with
                            // open upvalues escaped onto a foreign stack
                            // without being snapshotted — error, don't panic
                            // the process.
                            Err(stack_idx) => match self.stack.get(stack_idx) {
                                Some(v) => v.clone(),
                                None => {
                                    let err = foreign_upvalue_error();
                                    let saved_pc = pc - op::SIZE_OP_U16;
                                    handle_err!(self, fi, pc, err, saved_pc, 'dispatch)
                                }
                            },
                        };
                        self.stack.push(val);
                    }
                    op::STORE_UPVALUE => {
                        let idx = read_u16!(code, pc) as usize;
                        let val = unsafe { pop_unchecked(&mut self.stack) };
                        let open_target = {
                            let mut state =
                                self.frames[fi].closure.upvalues[idx].state.borrow_mut();
                            match &mut *state {
                                UpvalueState::Closed(v) => {
                                    *v = val;
                                    None
                                }
                                // Detached-but-live: write the owned value in
                                // place — no stack slot to touch on this VM.
                                UpvalueState::Tracked { value, .. } => {
                                    *value = val;
                                    None
                                }
                                UpvalueState::Open { frame_base, slot } => {
                                    Some((*frame_base + *slot, val))
                                }
                            }
                        };
                        if let Some((stack_idx, val)) = open_target {
                            // See LOAD_UPVALUE: an out-of-bounds Open cell
                            // means the closure escaped its owning VM.
                            match self.stack.get_mut(stack_idx) {
                                Some(slot) => *slot = val,
                                None => {
                                    let err = foreign_upvalue_error();
                                    let saved_pc = pc - op::SIZE_OP_U16;
                                    handle_err!(self, fi, pc, err, saved_pc, 'dispatch)
                                }
                            }
                        }
                    }

                    // --- Globals ---
                    op::LOAD_GLOBAL => {
                        let bits = read_u32!(code, pc);
                        let cache_slot = read_u16!(code, pc) as usize;
                        let cache_idx = self.frames[fi].cache_base + cache_slot;
                        let version = self.globals.version.get();
                        let entry = &self.inline_cache[cache_idx];
                        if entry.0 == bits && entry.1 == version {
                            self.stack.push(entry.2.value().clone());
                        } else {
                            let spur = bits_to_spur(bits);
                            match self.globals.get(spur) {
                                Some(val) => {
                                    self.inline_cache[cache_idx] =
                                        (bits, version, CachedGlobal::Plain(val.clone()));
                                    self.stack.push(val);
                                }
                                None => {
                                    let err = unbound_global_error(spur, &self.globals);
                                    handle_err!(self, fi, pc, err, pc - op::SIZE_LOAD_GLOBAL, 'dispatch);
                                }
                            }
                        }
                    }
                    op::STORE_GLOBAL => {
                        let bits = read_u32!(code, pc);
                        let spur = bits_to_spur(bits);
                        let val = unsafe { pop_unchecked(&mut self.stack) };
                        if !self.globals.set_existing(spur, val.clone()) {
                            self.globals.set(spur, val);
                        }
                    }
                    op::DEFINE_GLOBAL => {
                        let bits = read_u32!(code, pc);
                        let spur = bits_to_spur(bits);
                        let val = unsafe { pop_unchecked(&mut self.stack) };
                        self.globals.set(spur, val);
                    }

                    // --- Control flow ---
                    op::JUMP => {
                        let offset = read_i32!(code, pc);
                        pc = (pc as i64 + offset as i64) as usize;
                        // Backward jump = loop iteration boundary: run the
                        // step-limit / deadline / cancellation guard so tight
                        // loops like `(while #t)` are bounded and interruptible.
                        if offset < 0 {
                            ctx.check_loop_interrupt()?;
                        }
                    }
                    op::JUMP_IF_FALSE => {
                        let offset = read_i32!(code, pc);
                        let val = unsafe { pop_unchecked(&mut self.stack) };
                        if !val.is_truthy() {
                            pc = (pc as i64 + offset as i64) as usize;
                            if offset < 0 {
                                ctx.check_loop_interrupt()?;
                            }
                        }
                    }
                    op::JUMP_IF_TRUE => {
                        let offset = read_i32!(code, pc);
                        let val = unsafe { pop_unchecked(&mut self.stack) };
                        if val.is_truthy() {
                            pc = (pc as i64 + offset as i64) as usize;
                            if offset < 0 {
                                ctx.check_loop_interrupt()?;
                            }
                        }
                    }

                    // --- Function calls ---
                    op::CALL => {
                        let argc = read_u16!(code, pc) as usize;
                        self.frames[fi].pc = pc;
                        let saved_pc = pc - op::SIZE_OP_U16;
                        if let Err(err) = self.call_value(argc, ctx) {
                            match self.handle_exception(err, saved_pc)? {
                                ExceptionAction::Handled => {}
                                ExceptionAction::Propagate(e) => return Err(e),
                            }
                        }
                        // A native dispatched via call_value suspended structurally.
                        if let Some(signal) = self.native_signal.take() {
                            if let Some(top) = self.stack.last_mut() {
                                *top = Value::nil();
                            }
                            self.frames[fi].pc = pc;
                            return Ok(signal.into_exec_result());
                        }
                        continue 'dispatch;
                    }
                    op::TAIL_CALL => {
                        let argc = read_u16!(code, pc) as usize;
                        self.frames[fi].pc = pc;
                        let saved_pc = pc - op::SIZE_OP_U16;
                        if let Err(err) = self.tail_call_value(argc, ctx) {
                            match self.handle_exception(err, saved_pc)? {
                                ExceptionAction::Handled => {}
                                ExceptionAction::Propagate(e) => return Err(e),
                            }
                        }
                        // A native dispatched via tail_call_value → call_value
                        // suspended structurally.
                        if let Some(signal) = self.native_signal.take() {
                            if let Some(top) = self.stack.last_mut() {
                                *top = Value::nil();
                            }
                            self.frames[fi].pc = pc;
                            return Ok(signal.into_exec_result());
                        }
                        continue 'dispatch;
                    }
                    op::SELF_TAIL_CALL => {
                        // Self-recursive tail call: the callee is the current
                        // frame's own closure, so no callee value is on the stack.
                        // Cannot dispatch a native, so no async-yield path is
                        // possible; only an arity mismatch can raise.
                        let argc = read_u16!(code, pc) as usize;
                        self.frames[fi].pc = pc;
                        let saved_pc = pc - op::SIZE_OP_U16;
                        if let Err(err) = self.self_tail_call(argc) {
                            match self.handle_exception(err, saved_pc)? {
                                ExceptionAction::Handled => {}
                                ExceptionAction::Propagate(e) => return Err(e),
                            }
                        }
                        continue 'dispatch;
                    }
                    op::CALL_SELF => {
                        // Direct (non-tail) self-call: the callee is the current
                        // frame's own closure — no global lookup, no callable
                        // dispatch, and no callee value on the stack (contrast
                        // Call / CallGlobal); only the argc args are. Cannot
                        // dispatch a native, so no async-yield path is possible;
                        // only an arity mismatch or frame overflow can raise.
                        let argc = read_u16!(code, pc) as usize;
                        self.frames[fi].pc = pc;
                        let saved_pc = pc - op::SIZE_OP_U16;
                        let closure = self.frames[fi].closure.clone();
                        if let Err(err) = self.call_vm_closure_direct(closure, argc) {
                            match self.handle_exception(err, saved_pc)? {
                                ExceptionAction::Handled => {}
                                ExceptionAction::Propagate(e) => return Err(e),
                            }
                        }
                        continue 'dispatch;
                    }
                    op::RETURN => {
                        let result = if !self.stack.is_empty() {
                            unsafe { pop_unchecked(&mut self.stack) }
                        } else {
                            Value::nil()
                        };
                        // Close open upvalues before popping
                        let base = self.frames.last().unwrap().base;
                        if let Some(ref mut open) = self.frames.last_mut().unwrap().open_upvalues {
                            close_open_upvalues(open, &self.stack, base);
                        }
                        let frame = self.frames.pop().unwrap();
                        self.stack.truncate(frame.base);
                        if self.frames.is_empty() {
                            return Ok(crate::debug::VmExecResult::Finished(result));
                        }
                        self.stack.push(result);
                        continue 'dispatch;
                    }

                    // --- Closures ---
                    op::MAKE_CLOSURE => {
                        self.frames[fi].pc = pc - op::SIZE_OP; // make_closure reads from frame.pc (the opcode position)
                        self.make_closure()?;
                        continue 'dispatch;
                    }

                    op::CALL_NATIVE => {
                        let native_id = read_u16!(code, pc) as usize;
                        let argc = read_u16!(code, pc) as usize;
                        self.frames[fi].pc = pc;
                        let saved_pc = pc - op::SIZE_CALL_NATIVE;

                        // Direct dispatch: index into pre-resolved native function table.
                        // No env lookup, no cache — resolved at VM creation.
                        // Real bounds check (not debug_assert!): a crafted .semac can
                        // carry an out-of-range native_id that passes load-time
                        // validation, and the index below would panic in release.
                        if native_id >= self.native_fns.len() {
                            return Err(SemaError::eval(format!(
                                "CallNative: native_id {} out of range (table has {} entries)",
                                native_id,
                                self.native_fns.len()
                            )));
                        }

                        // Keep open upvalues open across the call. Explicit-owner
                        // handoffs snapshot closures before they cross onto a
                        // fresh callback or async task VM.
                        let native = self.native_fns[native_id].clone();
                        let args_start = self.stack.len() - argc;
                        // Move args into an owned buffer and drop them from the
                        // stack before the call so no borrow of self.stack is held
                        // while the native runs. SmallVec keeps argc <= 8 off the
                        // heap; drain moves without refcount traffic.
                        let call_args: SmallVec<[Value; 8]> =
                            self.stack.drain(args_start..).collect();
                        match self.dispatch_native(&native, &call_args, ctx) {
                            Ok(NativeDispatchResult::Value(val)) => {
                                self.stack.push(val);
                            }
                            // Args are already truncated. Push nil as a placeholder
                            // for the call result slot; on resume the runtime replaces
                            // it with the actual resume value before continuing.
                            Ok(NativeDispatchResult::Pending(outcome)) => {
                                self.stack.push(Value::nil());
                                self.frames[fi].pc = pc; // PC already past CALL_NATIVE
                                return Ok(crate::debug::VmExecResult::Pending(outcome));
                            }
                            Err(err) => {
                                handle_err!(self, fi, pc, err, saved_pc, 'dispatch);
                            }
                        }
                        // The native completed on this frame, so stay in the
                        // inner loop instead of re-entering 'dispatch. The
                        // length check guards frame topology; DEBUG re-enters
                        // so stepping/breakpoint semantics are unchanged.
                        if DEBUG || self.frames.len() != fi + 1 {
                            continue 'dispatch;
                        }
                        // Re-derive the frame-cached locals from the unchanged
                        // frame rather than keeping them live across the native
                        // call (frames[fi].pc was synced to the post-operand pc
                        // above).
                        let frame = &self.frames[fi];
                        code = frame.closure.func.chunk.code.as_ptr();
                        consts = frame.closure.func.chunk.consts.as_slice();
                        base = frame.base;
                        pc = frame.pc;
                        code_len = frame.closure.func.chunk.code.len();
                    }

                    // --- Data constructors ---
                    op::MAKE_LIST => {
                        let n = read_u16!(code, pc) as usize;
                        let start = self.stack.len() - n;
                        let items = self.stack.split_off(start);
                        self.stack.push(Value::list(items));
                    }
                    op::MAKE_VECTOR => {
                        let n = read_u16!(code, pc) as usize;
                        let start = self.stack.len() - n;
                        let items = self.stack.split_off(start);
                        self.stack.push(Value::vector(items));
                    }
                    op::MAKE_MAP => {
                        let n = read_u16!(code, pc) as usize;
                        let start = self.stack.len() - n * 2;
                        let items: Vec<Value> = self.stack.drain(start..).collect();
                        if let Err(err) = check_literal_map_keys(&items) {
                            handle_err!(self, fi, pc, err, pc - op::SIZE_OP_U16, 'dispatch);
                        }
                        let mut map = BTreeMap::new();
                        for pair in items.chunks(2) {
                            map.insert(pair[0].clone(), pair[1].clone());
                        }
                        self.stack.push(Value::map(map));
                    }
                    op::MAKE_HASH_MAP => {
                        let n = read_u16!(code, pc) as usize;
                        let start = self.stack.len() - n * 2;
                        let items: Vec<Value> = self.stack.drain(start..).collect();
                        if let Err(err) = check_literal_map_keys(&items) {
                            handle_err!(self, fi, pc, err, pc - op::SIZE_OP_U16, 'dispatch);
                        }
                        let mut map = hashbrown::HashMap::new();
                        for pair in items.chunks(2) {
                            map.insert(pair[0].clone(), pair[1].clone());
                        }
                        self.stack.push(Value::hashmap_from_rc(Rc::new(map)));
                    }

                    // --- Exceptions ---
                    op::THROW => {
                        let val = unsafe { pop_unchecked(&mut self.stack) };
                        // A caught condition map re-raises as itself; anything
                        // else raises as a fresh user exception.
                        let err = SemaError::from_thrown(val);
                        handle_err!(self, fi, pc, err, pc - op::SIZE_OP, 'dispatch);
                    }

                    // --- Arithmetic ---
                    op::ADD => {
                        let b = unsafe { pop_unchecked(&mut self.stack) };
                        let a = unsafe { pop_unchecked(&mut self.stack) };
                        match vm_add(&a, &b) {
                            Ok(v) => self.stack.push(v),
                            Err(err) => handle_err!(self, fi, pc, err, pc - op::SIZE_OP, 'dispatch),
                        }
                    }
                    op::SUB => {
                        let b = unsafe { pop_unchecked(&mut self.stack) };
                        let a = unsafe { pop_unchecked(&mut self.stack) };
                        match vm_sub(&a, &b) {
                            Ok(v) => self.stack.push(v),
                            Err(err) => handle_err!(self, fi, pc, err, pc - op::SIZE_OP, 'dispatch),
                        }
                    }
                    op::MUL => {
                        let b = unsafe { pop_unchecked(&mut self.stack) };
                        let a = unsafe { pop_unchecked(&mut self.stack) };
                        match vm_mul(&a, &b) {
                            Ok(v) => self.stack.push(v),
                            Err(err) => handle_err!(self, fi, pc, err, pc - op::SIZE_OP, 'dispatch),
                        }
                    }
                    op::DIV => {
                        let b = unsafe { pop_unchecked(&mut self.stack) };
                        let a = unsafe { pop_unchecked(&mut self.stack) };
                        match vm_div(&a, &b) {
                            Ok(v) => self.stack.push(v),
                            Err(err) => handle_err!(self, fi, pc, err, pc - op::SIZE_OP, 'dispatch),
                        }
                    }
                    op::NEGATE => {
                        let a = unsafe { pop_unchecked(&mut self.stack) };
                        if let Some(n) = a.as_int() {
                            match n.checked_neg() {
                                Some(v) => self.stack.push(Value::int(v)),
                                // i64::MIN negation overflows i64; promote to a bignum
                                // instead of raising or silently wrapping (matches the
                                // stdlib `-` unary case in sema-stdlib/src/arithmetic.rs).
                                None => self
                                    .stack
                                    .push(Value::from_number(SemaNumber::from_i64(n).neg())),
                            }
                        } else if let Some(f) = a.as_float() {
                            self.stack.push(Value::float(-f));
                        } else if let Some(n) = a.as_number() {
                            // Bignum/rational/complex operand: fold through the tower
                            // instead of raising (mirrors vm_add/vm_sub/vm_mul above).
                            self.stack.push(Value::from_number(n.neg()));
                        } else {
                            let err = SemaError::type_error("number", a.type_name());
                            handle_err!(self, fi, pc, err, pc - op::SIZE_OP, 'dispatch);
                        }
                    }
                    op::NOT => {
                        let a = unsafe { pop_unchecked(&mut self.stack) };
                        self.stack.push(Value::bool(!a.is_truthy()));
                    }
                    op::EQ => {
                        let b = unsafe { pop_unchecked(&mut self.stack) };
                        let a = unsafe { pop_unchecked(&mut self.stack) };
                        self.stack.push(Value::bool(vm_eq(&a, &b)));
                    }
                    op::LT => {
                        let b = unsafe { pop_unchecked(&mut self.stack) };
                        let a = unsafe { pop_unchecked(&mut self.stack) };
                        match vm_lt(&a, &b) {
                            Ok(v) => self.stack.push(Value::bool(v)),
                            Err(err) => handle_err!(self, fi, pc, err, pc - op::SIZE_OP, 'dispatch),
                        }
                    }
                    op::GT => {
                        let b = unsafe { pop_unchecked(&mut self.stack) };
                        let a = unsafe { pop_unchecked(&mut self.stack) };
                        match vm_lt(&b, &a) {
                            Ok(v) => self.stack.push(Value::bool(v)),
                            Err(err) => handle_err!(self, fi, pc, err, pc - op::SIZE_OP, 'dispatch),
                        }
                    }
                    op::LE => {
                        let b = unsafe { pop_unchecked(&mut self.stack) };
                        let a = unsafe { pop_unchecked(&mut self.stack) };
                        match vm_lt(&b, &a) {
                            Ok(v) => self.stack.push(Value::bool(!v)),
                            Err(err) => handle_err!(self, fi, pc, err, pc - op::SIZE_OP, 'dispatch),
                        }
                    }
                    op::GE => {
                        let b = unsafe { pop_unchecked(&mut self.stack) };
                        let a = unsafe { pop_unchecked(&mut self.stack) };
                        match vm_lt(&a, &b) {
                            Ok(v) => self.stack.push(Value::bool(!v)),
                            Err(err) => handle_err!(self, fi, pc, err, pc - op::SIZE_OP, 'dispatch),
                        }
                    }

                    // --- Specialized int fast paths ---
                    // These operate directly on raw u64 bits to avoid Clone/Drop overhead.
                    // Small ints are immediates (no heap pointer), so we can safely
                    // overwrite stack slots and adjust length without running destructors.
                    //
                    // SAFETY: *_INT opcodes are emitted only when the compiler has placed
                    // two values on the stack (lower.rs guarantees this). The NAN_TAG_MASK
                    // check confirms both slots hold small-int immediates — pure bit patterns
                    // with no Rc to leak — so ptr::write skipping Drop is safe. The result
                    // bits encode a valid small-int Value (NAN_INT_SMALL_PATTERN | 45-bit payload).
                    op::ADD_INT => {
                        let len = self.stack.len();
                        let a_bits = unsafe { (*self.stack.as_ptr().add(len - 2)).raw_bits() };
                        let b_bits = unsafe { (*self.stack.as_ptr().add(len - 1)).raw_bits() };
                        if (a_bits & NAN_TAG_MASK) == NAN_INT_SMALL_PATTERN
                            && (b_bits & NAN_TAG_MASK) == NAN_INT_SMALL_PATTERN
                        {
                            // Sign-extend to i64 and add through Value::int, which boxes
                            // the result when it overflows the 45-bit small-int range.
                            // (The old raw-bit `& NAN_PAYLOAD_MASK` trick silently
                            // truncated sums past ±2^44 — see the dual-eval
                            // big_int_add_* regression tests.)
                            let ax =
                                (((a_bits & NAN_PAYLOAD_MASK) << SIGN_SHIFT) as i64) >> SIGN_SHIFT;
                            let bx =
                                (((b_bits & NAN_PAYLOAD_MASK) << SIGN_SHIFT) as i64) >> SIGN_SHIFT;
                            unsafe {
                                std::ptr::write(
                                    self.stack.as_mut_ptr().add(len - 2),
                                    Value::int(ax.wrapping_add(bx)),
                                );
                                self.stack.set_len(len - 1);
                            }
                        } else {
                            let b = unsafe { pop_unchecked(&mut self.stack) };
                            let a = unsafe { pop_unchecked(&mut self.stack) };
                            match vm_add(&a, &b) {
                                Ok(v) => self.stack.push(v),
                                Err(err) => {
                                    handle_err!(self, fi, pc, err, pc - op::SIZE_OP, 'dispatch)
                                }
                            }
                        }
                    }
                    op::SUB_INT => {
                        let len = self.stack.len();
                        let a_bits = unsafe { (*self.stack.as_ptr().add(len - 2)).raw_bits() };
                        let b_bits = unsafe { (*self.stack.as_ptr().add(len - 1)).raw_bits() };
                        if (a_bits & NAN_TAG_MASK) == NAN_INT_SMALL_PATTERN
                            && (b_bits & NAN_TAG_MASK) == NAN_INT_SMALL_PATTERN
                        {
                            // Sign-extend to i64 and subtract through Value::int, which
                            // boxes the result when it overflows the 45-bit small-int
                            // range (a raw-bit subtract would truncate past ±2^44).
                            let ax =
                                (((a_bits & NAN_PAYLOAD_MASK) << SIGN_SHIFT) as i64) >> SIGN_SHIFT;
                            let bx =
                                (((b_bits & NAN_PAYLOAD_MASK) << SIGN_SHIFT) as i64) >> SIGN_SHIFT;
                            unsafe {
                                std::ptr::write(
                                    self.stack.as_mut_ptr().add(len - 2),
                                    Value::int(ax.wrapping_sub(bx)),
                                );
                                self.stack.set_len(len - 1);
                            }
                        } else {
                            let b = unsafe { pop_unchecked(&mut self.stack) };
                            let a = unsafe { pop_unchecked(&mut self.stack) };
                            match vm_sub(&a, &b) {
                                Ok(v) => self.stack.push(v),
                                Err(err) => {
                                    handle_err!(self, fi, pc, err, pc - op::SIZE_OP, 'dispatch)
                                }
                            }
                        }
                    }
                    op::MUL_INT => {
                        let len = self.stack.len();
                        let a_bits = unsafe { (*self.stack.as_ptr().add(len - 2)).raw_bits() };
                        let b_bits = unsafe { (*self.stack.as_ptr().add(len - 1)).raw_bits() };
                        if (a_bits & NAN_TAG_MASK) == NAN_INT_SMALL_PATTERN
                            && (b_bits & NAN_TAG_MASK) == NAN_INT_SMALL_PATTERN
                        {
                            // Branchless sign-extension to i64
                            let ax =
                                (((a_bits & NAN_PAYLOAD_MASK) << SIGN_SHIFT) as i64) >> SIGN_SHIFT;
                            let bx =
                                (((b_bits & NAN_PAYLOAD_MASK) << SIGN_SHIFT) as i64) >> SIGN_SHIFT;
                            // Two 45-bit smalls can multiply past i64 (up to ~2^88).
                            // Value::int boxes an in-range result that overflows the
                            // 45-bit small range; a genuine i64 overflow promotes to a
                            // bignum through the tower rather than raising.
                            match ax.checked_mul(bx) {
                                Some(p) => unsafe {
                                    std::ptr::write(
                                        self.stack.as_mut_ptr().add(len - 2),
                                        Value::int(p),
                                    );
                                    self.stack.set_len(len - 1);
                                },
                                None => unsafe {
                                    std::ptr::write(
                                        self.stack.as_mut_ptr().add(len - 2),
                                        Value::from_number(
                                            SemaNumber::from_i64(ax).mul(SemaNumber::from_i64(bx)),
                                        ),
                                    );
                                    self.stack.set_len(len - 1);
                                },
                            }
                        } else {
                            let b = unsafe { pop_unchecked(&mut self.stack) };
                            let a = unsafe { pop_unchecked(&mut self.stack) };
                            match vm_mul(&a, &b) {
                                Ok(v) => self.stack.push(v),
                                Err(err) => {
                                    handle_err!(self, fi, pc, err, pc - op::SIZE_OP, 'dispatch)
                                }
                            }
                        }
                    }
                    op::LT_INT => {
                        let len = self.stack.len();
                        let a_bits = unsafe { (*self.stack.as_ptr().add(len - 2)).raw_bits() };
                        let b_bits = unsafe { (*self.stack.as_ptr().add(len - 1)).raw_bits() };
                        if (a_bits & NAN_TAG_MASK) == NAN_INT_SMALL_PATTERN
                            && (b_bits & NAN_TAG_MASK) == NAN_INT_SMALL_PATTERN
                        {
                            // Branchless sign-extension and compare
                            let ax =
                                (((a_bits & NAN_PAYLOAD_MASK) << SIGN_SHIFT) as i64) >> SIGN_SHIFT;
                            let bx =
                                (((b_bits & NAN_PAYLOAD_MASK) << SIGN_SHIFT) as i64) >> SIGN_SHIFT;
                            unsafe {
                                std::ptr::write(
                                    self.stack.as_mut_ptr().add(len - 2),
                                    Value::bool(ax < bx),
                                );
                                self.stack.set_len(len - 1);
                            }
                        } else {
                            let b = unsafe { pop_unchecked(&mut self.stack) };
                            let a = unsafe { pop_unchecked(&mut self.stack) };
                            match vm_lt(&a, &b) {
                                Ok(v) => self.stack.push(Value::bool(v)),
                                Err(err) => {
                                    handle_err!(self, fi, pc, err, pc - op::SIZE_OP, 'dispatch)
                                }
                            }
                        }
                    }
                    op::EQ_INT => {
                        let len = self.stack.len();
                        let a_bits = unsafe { (*self.stack.as_ptr().add(len - 2)).raw_bits() };
                        let b_bits = unsafe { (*self.stack.as_ptr().add(len - 1)).raw_bits() };
                        if (a_bits & NAN_TAG_MASK) == NAN_INT_SMALL_PATTERN
                            && (b_bits & NAN_TAG_MASK) == NAN_INT_SMALL_PATTERN
                        {
                            // Small ints: equal exactly when their bits match
                            unsafe {
                                std::ptr::write(
                                    self.stack.as_mut_ptr().add(len - 2),
                                    Value::bool(a_bits == b_bits),
                                );
                                self.stack.set_len(len - 1);
                            }
                        } else {
                            let b = unsafe { pop_unchecked(&mut self.stack) };
                            let a = unsafe { pop_unchecked(&mut self.stack) };
                            self.stack.push(Value::bool(vm_eq(&a, &b)));
                        }
                    }

                    op::LOAD_LOCAL0 => {
                        self.stack.push(self.stack[base].clone());
                    }
                    op::LOAD_LOCAL1 => {
                        self.stack.push(self.stack[base + 1].clone());
                    }
                    op::LOAD_LOCAL2 => {
                        self.stack.push(self.stack[base + 2].clone());
                    }
                    op::LOAD_LOCAL3 => {
                        self.stack.push(self.stack[base + 3].clone());
                    }

                    // Fused LOAD_GLOBAL + CALL: look up global, call without
                    // pushing the function value onto the stack.
                    op::CALL_GLOBAL => {
                        let bits = read_u32!(code, pc);
                        let argc = read_u16!(code, pc) as usize;
                        let cache_slot = read_u16!(code, pc) as usize;
                        self.frames[fi].pc = pc;
                        let saved_pc = pc - op::SIZE_CALL_GLOBAL;

                        // Look up the global; a miss decodes the callee into
                        // the inline cache so every hit dispatches from the
                        // pre-decoded entry (no Value clone, no downcast).
                        let cache_idx = self.frames[fi].cache_base + cache_slot;
                        let version = self.globals.version.get();
                        let entry = &self.inline_cache[cache_idx];
                        if entry.0 != bits || entry.1 != version {
                            let spur = bits_to_spur(bits);
                            match self.globals.get(spur) {
                                Some(val) => {
                                    self.inline_cache[cache_idx] =
                                        (bits, version, CachedGlobal::decode(val));
                                }
                                None => {
                                    let err = unbound_global_error(spur, &self.globals);
                                    handle_err!(self, fi, pc, err, saved_pc, 'dispatch);
                                }
                            }
                        }

                        match &self.inline_cache[cache_idx].2 {
                            // Fast path: VM closure — direct call without a
                            // function slot on the stack.
                            CachedGlobal::VmClosure {
                                closure, functions, ..
                            } => {
                                let closure = closure.clone();
                                if !Rc::ptr_eq(functions, &self.functions) {
                                    let f = functions.clone();
                                    self.functions = f;
                                }
                                if let Err(err) = self.call_vm_closure_direct(closure, argc) {
                                    match self.handle_exception(err, saved_pc)? {
                                        ExceptionAction::Handled => {}
                                        ExceptionAction::Propagate(e) => return Err(e),
                                    }
                                }
                                continue 'dispatch;
                            }
                            CachedGlobal::Native { func, .. } => {
                                let func = func.clone();
                                match self.call_native_with(&func, argc, ctx) {
                                    Ok(()) => {
                                        // The native suspended structurally. The call
                                        // pushed a nil placeholder; on resume the
                                        // runtime substitutes the actual resume value.
                                        if let Some(signal) = self.native_signal.take() {
                                            self.frames[fi].pc = pc; // PC already past CALL_GLOBAL
                                            return Ok(signal.into_exec_result());
                                        }
                                        // The native completed on this frame, so
                                        // stay in the inner loop instead of
                                        // re-entering 'dispatch. The length check
                                        // guards frame topology; DEBUG re-enters
                                        // so stepping and breakpoint semantics are
                                        // unchanged.
                                        if DEBUG || self.frames.len() != fi + 1 {
                                            continue 'dispatch;
                                        }
                                        // Re-derive the frame-cached locals from
                                        // the unchanged frame rather than keeping
                                        // them live across the native call
                                        // (frames[fi].pc was synced to the
                                        // post-operand pc above).
                                        let frame = &self.frames[fi];
                                        code = frame.closure.func.chunk.code.as_ptr();
                                        consts = frame.closure.func.chunk.consts.as_slice();
                                        base = frame.base;
                                        pc = frame.pc;
                                        code_len = frame.closure.func.chunk.code.len();
                                    }
                                    Err(err) => {
                                        handle_err!(self, fi, pc, err, saved_pc, 'dispatch);
                                    }
                                }
                            }
                            // Slow path: non-native callable — use call_value_with
                            CachedGlobal::Plain(value) => {
                                let func_val = value.clone();
                                if let Err(err) = self.call_value_with(func_val, argc, ctx) {
                                    match self.handle_exception(err, saved_pc)? {
                                        ExceptionAction::Handled => {}
                                        ExceptionAction::Propagate(e) => return Err(e),
                                    }
                                }
                                // A native callable suspended structurally; the
                                // placeholder is on top.
                                if let Some(signal) = self.native_signal.take() {
                                    self.frames[fi].pc = pc; // PC already past CALL_GLOBAL
                                    return Ok(signal.into_exec_result());
                                }
                                continue 'dispatch;
                            }
                        }
                    }

                    op::STORE_LOCAL0 => {
                        let val = unsafe { pop_unchecked(&mut self.stack) };
                        self.propagate_local_store_to_tracked(fi, 0, &val);
                        self.stack[base] = val;
                    }
                    op::STORE_LOCAL1 => {
                        let val = unsafe { pop_unchecked(&mut self.stack) };
                        self.propagate_local_store_to_tracked(fi, 1, &val);
                        self.stack[base + 1] = val;
                    }
                    op::STORE_LOCAL2 => {
                        let val = unsafe { pop_unchecked(&mut self.stack) };
                        self.propagate_local_store_to_tracked(fi, 2, &val);
                        self.stack[base + 2] = val;
                    }
                    op::STORE_LOCAL3 => {
                        let val = unsafe { pop_unchecked(&mut self.stack) };
                        self.propagate_local_store_to_tracked(fi, 3, &val);
                        self.stack[base + 3] = val;
                    }

                    // --- Inline stdlib intrinsics ---
                    op::CAR => {
                        let val = unsafe { pop_unchecked(&mut self.stack) };
                        if let Some(l) = val.as_list() {
                            self.stack.push(if l.is_empty() {
                                Value::nil()
                            } else {
                                l[0].clone()
                            });
                        } else if let Some(v) = val.as_vector() {
                            self.stack.push(if v.is_empty() {
                                Value::nil()
                            } else {
                                v[0].clone()
                            });
                        } else {
                            let err = SemaError::type_error("list or vector", val.type_name());
                            handle_err!(self, fi, pc, err, pc - op::SIZE_OP, 'dispatch);
                        }
                    }
                    op::CDR => {
                        let val = unsafe { pop_unchecked(&mut self.stack) };
                        if let Some(l) = val.as_list() {
                            self.stack.push(if l.len() <= 1 {
                                Value::list(vec![])
                            } else {
                                Value::list(l[1..].to_vec())
                            });
                        } else if let Some(v) = val.as_vector() {
                            self.stack.push(if v.len() <= 1 {
                                Value::vector(vec![])
                            } else {
                                Value::vector(v[1..].to_vec())
                            });
                        } else {
                            let err = SemaError::type_error("list or vector", val.type_name());
                            handle_err!(self, fi, pc, err, pc - op::SIZE_OP, 'dispatch);
                        }
                    }
                    op::CONS => {
                        let tail = unsafe { pop_unchecked(&mut self.stack) };
                        let head = unsafe { pop_unchecked(&mut self.stack) };
                        if tail.is_nil() {
                            self.stack.push(Value::list(vec![head]));
                        } else if let Some(list) = tail.as_list() {
                            let mut new = Vec::with_capacity(1 + list.len());
                            new.push(head);
                            new.extend(list.iter().cloned());
                            self.stack.push(Value::list(new));
                        } else {
                            self.stack.push(Value::list(vec![head, tail]));
                        }
                    }
                    op::IS_NULL => {
                        let val = unsafe { pop_unchecked(&mut self.stack) };
                        let result = val.is_nil() || val.as_list().is_some_and(|l| l.is_empty());
                        self.stack.push(Value::bool(result));
                    }
                    op::IS_PAIR => {
                        let val = unsafe { pop_unchecked(&mut self.stack) };
                        let result = val.as_list().is_some_and(|l| !l.is_empty());
                        self.stack.push(Value::bool(result));
                    }
                    op::IS_LIST => {
                        let val = unsafe { pop_unchecked(&mut self.stack) };
                        self.stack.push(Value::bool(val.is_list()));
                    }
                    op::IS_NUMBER => {
                        let val = unsafe { pop_unchecked(&mut self.stack) };
                        // Matches stdlib `number?`: any tower number (fixnum,
                        // bignum, float, and later rational/complex).
                        self.stack.push(Value::bool(val.as_number().is_some()));
                    }
                    op::IS_STRING => {
                        let val = unsafe { pop_unchecked(&mut self.stack) };
                        self.stack.push(Value::bool(val.is_string()));
                    }
                    op::IS_SYMBOL => {
                        let val = unsafe { pop_unchecked(&mut self.stack) };
                        self.stack.push(Value::bool(val.is_symbol()));
                    }
                    op::LENGTH => {
                        let val = unsafe { pop_unchecked(&mut self.stack) };
                        if let Some(l) = val.as_list() {
                            self.stack.push(Value::int(l.len() as i64));
                        } else if let Some(v) = val.as_vector() {
                            self.stack.push(Value::int(v.len() as i64));
                        } else if let Some(s) = val.as_str() {
                            self.stack.push(Value::int(s.chars().count() as i64));
                        } else if let Some(m) = val.as_map_rc() {
                            self.stack.push(Value::int(m.len() as i64));
                        } else if let Some(m) = val.as_hashmap_rc() {
                            self.stack.push(Value::int(m.len() as i64));
                        } else if let Some(arr) = val.as_mutable_array() {
                            self.stack.push(Value::int(arr.items.borrow().len() as i64));
                        } else if let Some(bv) = val.as_bytevector() {
                            self.stack.push(Value::int(bv.len() as i64));
                        } else if let Some(arr) = val.as_f64_array() {
                            self.stack.push(Value::int(arr.len() as i64));
                        } else if let Some(arr) = val.as_i64_array() {
                            self.stack.push(Value::int(arr.len() as i64));
                        } else {
                            let err = SemaError::type_error(
                                "list, vector, string, map, hashmap, bytevector, typed array, or mutable-array",
                                val.type_name(),
                            );
                            handle_err!(self, fi, pc, err, pc - op::SIZE_OP, 'dispatch);
                        }
                    }

                    op::APPEND => {
                        let b = unsafe { pop_unchecked(&mut self.stack) };
                        let a = unsafe { pop_unchecked(&mut self.stack) };
                        if let (Some(la), Some(lb)) = (a.as_list(), b.as_list()) {
                            let mut result = Vec::with_capacity(la.len() + lb.len());
                            result.extend(la.iter().cloned());
                            result.extend(lb.iter().cloned());
                            self.stack.push(Value::list(result));
                        } else {
                            let mut result = Vec::new();
                            if let Some(l) = a.as_list() {
                                result.extend(l.iter().cloned());
                            } else if let Some(v) = a.as_vector() {
                                result.extend(v.iter().cloned());
                            } else {
                                let err = SemaError::type_error("list or vector", a.type_name());
                                handle_err!(self, fi, pc, err, pc - op::SIZE_OP, 'dispatch);
                            }
                            if let Some(l) = b.as_list() {
                                result.extend(l.iter().cloned());
                            } else if let Some(v) = b.as_vector() {
                                result.extend(v.iter().cloned());
                            } else {
                                let err = SemaError::type_error("list or vector", b.type_name());
                                handle_err!(self, fi, pc, err, pc - op::SIZE_OP, 'dispatch);
                            }
                            self.stack.push(Value::list(result));
                        }
                    }
                    op::GET => {
                        let key = unsafe { pop_unchecked(&mut self.stack) };
                        let coll = unsafe { pop_unchecked(&mut self.stack) };
                        if let Some(map) = coll.as_hashmap_ref() {
                            self.stack
                                .push(map.get(&key).cloned().unwrap_or(Value::nil()));
                        } else if let Some(map) = coll.as_map_ref() {
                            self.stack
                                .push(map.get(&key).cloned().unwrap_or(Value::nil()));
                        } else {
                            let err = SemaError::type_error("map or hashmap", coll.type_name())
                                .with_hint(map_access_hint("get", &coll));
                            handle_err!(self, fi, pc, err, pc - op::SIZE_OP, 'dispatch);
                        }
                    }
                    op::CONTAINS_Q => {
                        let key = unsafe { pop_unchecked(&mut self.stack) };
                        let coll = unsafe { pop_unchecked(&mut self.stack) };
                        if let Some(map) = coll.as_hashmap_ref() {
                            self.stack.push(Value::bool(map.contains_key(&key)));
                        } else if let Some(map) = coll.as_map_ref() {
                            self.stack.push(Value::bool(map.contains_key(&key)));
                        } else {
                            let err = SemaError::type_error("map or hashmap", coll.type_name())
                                .with_hint(map_access_hint("contains?", &coll));
                            handle_err!(self, fi, pc, err, pc - op::SIZE_OP, 'dispatch);
                        }
                    }

                    op::MOD => {
                        let b = unsafe { pop_unchecked(&mut self.stack) };
                        let a = unsafe { pop_unchecked(&mut self.stack) };
                        match vm_mod(&a, &b) {
                            Ok(v) => self.stack.push(v),
                            Err(err) => {
                                handle_err!(self, fi, pc, err, pc - op::SIZE_OP, 'dispatch)
                            }
                        }
                    }
                    op::NTH => {
                        let idx_val = unsafe { pop_unchecked(&mut self.stack) };
                        let coll = unsafe { pop_unchecked(&mut self.stack) };
                        let idx = match idx_val.as_int() {
                            Some(i) if i >= 0 => i as usize,
                            Some(i) => {
                                let err = SemaError::eval(format!(
                                    "nth: index must be non-negative, got {i}"
                                ));
                                handle_err!(self, fi, pc, err, pc - op::SIZE_OP, 'dispatch);
                            }
                            None => {
                                // A collection in the index slot almost always
                                // means the args are swapped — nth is (nth coll idx).
                                let swapped =
                                    idx_val.as_list().is_some() || idx_val.as_vector().is_some();
                                let hint = if swapped {
                                    "nth: argument order is (nth collection index) — looks like the arguments are swapped"
                                } else {
                                    "nth: argument order is (nth collection index); the index must be an integer"
                                };
                                let err = SemaError::type_error("int", idx_val.type_name())
                                    .with_hint(hint);
                                handle_err!(self, fi, pc, err, pc - op::SIZE_OP, 'dispatch);
                            }
                        };
                        if let Some(l) = coll.as_list() {
                            match l.get(idx) {
                                Some(v) => self.stack.push(v.clone()),
                                None => {
                                    let err = SemaError::eval(format!(
                                        "index {} out of bounds (length {})",
                                        idx,
                                        l.len()
                                    ));
                                    handle_err!(self, fi, pc, err, pc - op::SIZE_OP, 'dispatch);
                                }
                            }
                        } else if let Some(v) = coll.as_vector() {
                            match v.get(idx) {
                                Some(v) => self.stack.push(v.clone()),
                                None => {
                                    let err = SemaError::eval(format!(
                                        "index {} out of bounds (length {})",
                                        idx,
                                        v.len()
                                    ));
                                    handle_err!(self, fi, pc, err, pc - op::SIZE_OP, 'dispatch);
                                }
                            }
                        } else if let Some(arr) = coll.as_mutable_array() {
                            let item = arr.items.borrow().get(idx).cloned();
                            match item {
                                Some(v) => self.stack.push(v),
                                None => {
                                    let err = SemaError::eval(format!(
                                        "index {} out of bounds (length {})",
                                        idx,
                                        arr.items.borrow().len()
                                    ));
                                    handle_err!(self, fi, pc, err, pc - op::SIZE_OP, 'dispatch);
                                }
                            }
                        } else {
                            let err = SemaError::type_error("list or vector", coll.type_name());
                            handle_err!(self, fi, pc, err, pc - op::SIZE_OP, 'dispatch);
                        }
                    }

                    // --- String intrinsics (semantics mirror sema-stdlib/src/string.rs) ---
                    op::STRING_LENGTH => {
                        let val = unsafe { pop_unchecked(&mut self.stack) };
                        if let Some(s) = val.as_str() {
                            self.stack.push(Value::int(s.chars().count() as i64));
                        } else {
                            let err = SemaError::type_error("string", val.type_name());
                            handle_err!(self, fi, pc, err, pc - op::SIZE_OP, 'dispatch);
                        }
                    }
                    op::STRING_REF => {
                        let idx_val = unsafe { pop_unchecked(&mut self.stack) };
                        let str_val = unsafe { pop_unchecked(&mut self.stack) };
                        let Some(s) = str_val.as_str() else {
                            let err = SemaError::type_error("string", str_val.type_name());
                            handle_err!(self, fi, pc, err, pc - op::SIZE_OP, 'dispatch);
                        };
                        let Some(idx_signed) = idx_val.as_int() else {
                            let err = SemaError::type_error("int", idx_val.type_name());
                            handle_err!(self, fi, pc, err, pc - op::SIZE_OP, 'dispatch);
                        };
                        if idx_signed < 0 {
                            let err = SemaError::eval(format!(
                                "string-ref: index {idx_signed} must be non-negative"
                            ));
                            handle_err!(self, fi, pc, err, pc - op::SIZE_OP, 'dispatch);
                        }
                        let idx = idx_signed as usize;
                        match s.chars().nth(idx) {
                            Some(c) => self.stack.push(Value::char(c)),
                            None => {
                                let len = s.chars().count();
                                let err = SemaError::eval(format!(
                                    "string-ref: index {idx} out of bounds (string length {len})"
                                ))
                                .with_hint("indices are 0-based");
                                handle_err!(self, fi, pc, err, pc - op::SIZE_OP, 'dispatch);
                            }
                        }
                    }
                    op::STRING_APPEND => {
                        let b = unsafe { pop_unchecked(&mut self.stack) };
                        let a = unsafe { pop_unchecked(&mut self.stack) };
                        let result = if let (Some(x), Some(y)) = (a.as_str(), b.as_str()) {
                            // Both strings: one exact-capacity allocation.
                            let mut s = String::with_capacity(x.len() + y.len());
                            s.push_str(x);
                            s.push_str(y);
                            s
                        } else {
                            use std::fmt::Write;
                            let mut s = String::new();
                            for arg in [&a, &b] {
                                if let Some(x) = arg.as_str() {
                                    s.push_str(x);
                                } else {
                                    write!(&mut s, "{}", arg).unwrap();
                                }
                            }
                            s
                        };
                        self.stack.push(Value::string_owned(result));
                    }

                    // --- Mutable-array intrinsics (implementation shared with
                    //     sema-stdlib/src/mutable.rs via sema_core::mutable_ops,
                    //     so errors are byte-identical across dispatch paths) ---
                    op::MUT_ARR_GET => {
                        let idx = unsafe { pop_unchecked(&mut self.stack) };
                        let arr = unsafe { pop_unchecked(&mut self.stack) };
                        match sema_core::mutable_array_get(&arr, &idx, None) {
                            Ok(v) => self.stack.push(v),
                            Err(err) => {
                                handle_err!(self, fi, pc, err, pc - op::SIZE_OP, 'dispatch)
                            }
                        }
                    }
                    op::MUT_ARR_SET => {
                        let val = unsafe { pop_unchecked(&mut self.stack) };
                        let idx = unsafe { pop_unchecked(&mut self.stack) };
                        let arr = unsafe { pop_unchecked(&mut self.stack) };
                        if NodePtr::of_value(&val).is_some() {
                            EscapingValueWalker::with_owner(self).visit_value(&val);
                        }
                        match sema_core::mutable_array_set(&arr, &idx, val) {
                            // The Sema-level contract returns the array itself;
                            // the popped handle goes straight back — no clone.
                            Ok(()) => self.stack.push(arr),
                            Err(err) => {
                                handle_err!(self, fi, pc, err, pc - op::SIZE_OP, 'dispatch)
                            }
                        }
                    }

                    _ => {
                        return Err(SemaError::eval(format!("VM: invalid opcode {}", op)));
                    }
                }
            }
        }
    }

    // --- Function call implementation ---

    fn call_value(&mut self, argc: usize, ctx: &EvalContext) -> Result<(), SemaError> {
        let func_idx = self.stack.len() - 1 - argc;

        // Fast path: peek at tag without Rc refcount bump.
        if self.stack[func_idx].raw_tag() == Some(TAG_NATIVE_FN) {
            // Check for VM closure payload without holding a borrow across mutation.
            // Extract closure (cloned Rc) in a block; only clone functions if different.
            let vm_closure_data = {
                let native = self.stack[func_idx].as_native_fn_ref().unwrap();
                native.payload.as_ref().and_then(|p| {
                    p.downcast_ref::<VmClosurePayload>().map(|vmc| {
                        let closure = vmc.closure.clone();
                        // Only clone functions Rc if it's a different table
                        let functions = if Rc::ptr_eq(&vmc.functions, &self.functions) {
                            None
                        } else {
                            Some(vmc.functions.clone())
                        };
                        (closure, functions)
                    })
                })
            };
            if let Some((closure, functions)) = vm_closure_data {
                if let Some(f) = functions {
                    self.functions = f;
                }
                return self.call_vm_closure(closure, argc);
            }
            // Keep open upvalues live across the native call so structural
            // in-VM HOF callbacks can write back through them. Move args into
            // an owned buffer to release the stack borrow during dispatch.
            let func_rc = self.stack[func_idx].as_native_fn_rc().unwrap();
            let call_args: SmallVec<[Value; 8]> = self.stack.drain(func_idx + 1..).collect();
            self.stack.pop(); // pop the native fn value
            let dispatch = self.dispatch_native(&func_rc, &call_args, ctx)?;
            self.stash_native_dispatch(dispatch);
            Ok(())
        } else if let Some(kw) = self.stack[func_idx].as_keyword_spur() {
            // Keyword as function: (kw map) -> map[kw]
            if argc != 1 {
                return Err(SemaError::arity(resolve_spur(kw), "1", argc));
            }
            let arg = self.stack.pop().unwrap();
            self.stack.pop(); // pop keyword
            let kw_val = Value::keyword_from_spur(kw);
            let result = if let Some(m) = arg.as_map_rc() {
                m.get(&kw_val).cloned().unwrap_or(Value::nil())
            } else if let Some(m) = arg.as_hashmap_rc() {
                m.get(&kw_val).cloned().unwrap_or(Value::nil())
            } else {
                return Err(SemaError::type_error("map or hashmap", arg.type_name()));
            };
            self.stack.push(result);
            Ok(())
        } else {
            // Keep upvalues open across structural multimethod dispatch. Move
            // args into an owned buffer so no stack borrow is held while the
            // selected handler is invoked.
            let func_val = self.stack[func_idx].clone();
            let call_args: SmallVec<[Value; 8]> = self.stack.drain(func_idx + 1..).collect();
            self.stack.pop(); // pop the callable value
            self.call_non_native(func_val, call_args, ctx)
        }
    }

    /// Dispatch a callable that is neither a native fn nor a keyword. During a
    /// runtime quantum a multimethod becomes two structural calls: first its
    /// dispatch function, then the selected handler. Outside a runtime quantum
    /// this retains the host-only synchronous callback path.
    fn call_non_native(
        &mut self,
        func_val: Value,
        call_args: SmallVec<[Value; 8]>,
        ctx: &EvalContext,
    ) -> Result<(), SemaError> {
        if ctx.runtime_quantum_active() && func_val.as_multimethod_rc().is_some() {
            let outcome = NativeOutcome::Call(multimethod_call(
                func_val,
                call_args.into_vec(),
                Box::new(MultimethodCallContinuation),
            )?);
            self.stash_native_dispatch(NativeDispatchResult::Pending(
                VmPendingOutcome::from_outcome(outcome),
            ));
            return Ok(());
        }
        if ctx.runtime_quantum_active() {
            return Err(SemaError::eval(format!(
                "not callable: {} ({})",
                func_val,
                func_val.type_name()
            ))
            .with_hint("expected a function, lambda, or keyword"));
        }
        snapshot_escaping_call_with_owner(self, &func_val, &call_args);
        let result = sema_core::call_callback(ctx, &func_val, &call_args);
        self.sync_tracked_upvalues_to_stack();
        let result = result?;
        self.stack.push(result);
        Ok(())
    }

    fn tail_call_value(&mut self, argc: usize, ctx: &EvalContext) -> Result<(), SemaError> {
        let func_idx = self.stack.len() - 1 - argc;

        // Fast path: peek at tag without Rc refcount bump
        if self.stack[func_idx].raw_tag() == Some(TAG_NATIVE_FN) {
            let vm_closure_data = {
                let native = self.stack[func_idx].as_native_fn_ref().unwrap();
                native.payload.as_ref().and_then(|p| {
                    p.downcast_ref::<VmClosurePayload>().map(|vmc| {
                        let closure = vmc.closure.clone();
                        let functions = if Rc::ptr_eq(&vmc.functions, &self.functions) {
                            None
                        } else {
                            Some(vmc.functions.clone())
                        };
                        (closure, functions)
                    })
                })
            };
            if let Some((closure, functions)) = vm_closure_data {
                if let Some(f) = functions {
                    self.functions = f;
                }
                return self.tail_call_vm_closure(closure, argc);
            }
        }

        // Non-VM callables: regular call (no TCO possible)
        self.call_value(argc, ctx)
    }

    /// Call a function value that's NOT on the stack (for CALL_GLOBAL slow path).
    /// The args are on top of the stack.
    fn call_value_with(
        &mut self,
        func_val: Value,
        argc: usize,
        ctx: &EvalContext,
    ) -> Result<(), SemaError> {
        if func_val.raw_tag() == Some(TAG_NATIVE_FN) {
            // Keep upvalues open and move args off the stack so native dispatch
            // holds no outstanding stack borrow.
            let func_rc = func_val.as_native_fn_rc().unwrap();
            let args_start = self.stack.len() - argc;
            let call_args: SmallVec<[Value; 8]> = self.stack.drain(args_start..).collect();
            let dispatch = self.dispatch_native(&func_rc, &call_args, ctx)?;
            self.stash_native_dispatch(dispatch);
            Ok(())
        } else if let Some(kw) = func_val.as_keyword_spur() {
            if argc != 1 {
                return Err(SemaError::arity(resolve_spur(kw), "1", argc));
            }
            let arg = self.stack.pop().unwrap();
            let kw_val = Value::keyword_from_spur(kw);
            let result = if let Some(m) = arg.as_map_rc() {
                m.get(&kw_val).cloned().unwrap_or(Value::nil())
            } else if let Some(m) = arg.as_hashmap_rc() {
                m.get(&kw_val).cloned().unwrap_or(Value::nil())
            } else {
                return Err(SemaError::type_error("map or hashmap", arg.type_name()));
            };
            self.stack.push(result);
            Ok(())
        } else {
            // C1: keep upvalues open; move args off the stack so a re-entrant
            // callback can run in-VM without an outstanding stack borrow.
            // Closures crossing onto a foreign stack are snapshotted at the
            // crossing point.
            let args_start = self.stack.len() - argc;
            let call_args: SmallVec<[Value; 8]> = self.stack.drain(args_start..).collect();
            self.call_non_native(func_val, call_args, ctx)
        }
    }

    /// Call a plain native fn (no VM-closure payload) whose `Rc` was
    /// pre-decoded by the CALL_GLOBAL inline cache. Args are on top of the
    /// stack; no function value is on the stack.
    fn call_native_with(
        &mut self,
        func: &Rc<NativeFn>,
        argc: usize,
        ctx: &EvalContext,
    ) -> Result<(), SemaError> {
        // Keep upvalues open and move args off the stack so native dispatch
        // holds no outstanding stack borrow.
        let args_start = self.stack.len() - argc;
        let call_args: SmallVec<[Value; 8]> = self.stack.drain(args_start..).collect();
        let dispatch = self.dispatch_native(func, &call_args, ctx)?;
        self.stash_native_dispatch(dispatch);
        Ok(())
    }

    /// Push a new CallFrame for a VM closure called via CALL_GLOBAL.
    /// No function value is on the stack — only args. `base = stack.len() - argc`.
    /// Args are already in place; we just extend the stack for remaining locals.
    fn call_vm_closure_direct(
        &mut self,
        closure: Rc<Closure>,
        argc: usize,
    ) -> Result<(), SemaError> {
        if self.frames.len() >= MAX_FRAMES {
            return Err(SemaError::eval(
                "stack overflow: maximum call depth exceeded",
            )
            .with_hint(
                "this usually means unbounded recursion; ensure recursive calls are in tail position for TCO, or use 'do' for iteration",
            ));
        }
        self.ensure_cache_space(&closure.func);
        let func = &closure.func;
        let arity = func.arity as usize;
        let has_rest = func.has_rest;
        let n_locals = func.chunk.n_locals as usize;

        // Arity check
        if has_rest {
            if argc < arity {
                return Err(SemaError::arity(
                    func.name
                        .map(resolve_spur)
                        .unwrap_or_else(|| "<lambda>".to_string()),
                    format!("{}+", arity),
                    argc,
                ));
            }
        } else if argc != arity {
            return Err(SemaError::arity(
                func.name
                    .map(resolve_spur)
                    .unwrap_or_else(|| "<lambda>".to_string()),
                arity.to_string(),
                argc,
            ));
        }

        // Args are already at stack[base..base+argc] in the right order.
        // base = stack.len() - argc
        let base = self.stack.len() - argc;

        if has_rest {
            // Collect extra args into a rest list
            let rest: Vec<Value> = self.stack[base + arity..base + argc].to_vec();
            self.stack.truncate(base + arity);
            self.stack.push(Value::list(rest));
        }

        // Resize to exact local count (pads with nil or truncates)
        self.stack.resize(base + n_locals, Value::nil());

        self.frames.push(CallFrame {
            cache_base: closure.func.cache_offset,
            closure,
            pc: 0,
            base,
            open_upvalues: None,
        });

        Ok(())
    }

    /// Push a new CallFrame for a VM closure (no Rust recursion).
    /// Caller must set `self.functions` before calling this.
    /// Takes ownership of the Rc to avoid an extra clone.
    fn call_vm_closure(&mut self, closure: Rc<Closure>, argc: usize) -> Result<(), SemaError> {
        if self.frames.len() >= MAX_FRAMES {
            return Err(SemaError::eval(
                "stack overflow: maximum call depth exceeded",
            )
            .with_hint(
                "this usually means unbounded recursion; ensure recursive calls are in tail position for TCO, or use 'do' for iteration",
            ));
        }
        self.ensure_cache_space(&closure.func);
        let func = &closure.func;
        let arity = func.arity as usize;
        let has_rest = func.has_rest;
        let n_locals = func.chunk.n_locals as usize;

        // Arity check
        if has_rest {
            if argc < arity {
                return Err(SemaError::arity(
                    func.name
                        .map(resolve_spur)
                        .unwrap_or_else(|| "<lambda>".to_string()),
                    format!("{}+", arity),
                    argc,
                ));
            }
        } else if argc != arity {
            return Err(SemaError::arity(
                func.name
                    .map(resolve_spur)
                    .unwrap_or_else(|| "<lambda>".to_string()),
                arity.to_string(),
                argc,
            ));
        }

        // Copy args directly from stack into new locals — no Vec allocation
        let func_idx = self.stack.len() - 1 - argc;
        let base = func_idx; // reuse the callee's slot as new frame base

        // Copy params: clone each arg into its local slot.
        // dest (base+i) < src (func_idx+1+i) so forward copy is safe.
        Self::copy_args_to_locals(&mut self.stack, base, func_idx + 1, arity, argc, has_rest);

        // Now resize to exact local count (pads with nil or truncates excess args)
        self.stack.resize(base + n_locals, Value::nil());

        // Push frame
        self.frames.push(CallFrame {
            cache_base: closure.func.cache_offset,
            closure,
            pc: 0,
            base,
            open_upvalues: None,
        });

        Ok(())
    }

    /// Tail-call a VM closure: reuse the current frame's stack space.
    /// Caller must set `self.functions` before calling this.
    /// Takes ownership of the Rc to avoid an extra clone.
    fn tail_call_vm_closure(&mut self, closure: Rc<Closure>, argc: usize) -> Result<(), SemaError> {
        let func = &closure.func;
        let arity = func.arity as usize;
        let has_rest = func.has_rest;
        let n_locals = func.chunk.n_locals as usize;

        // Arity check
        if has_rest {
            if argc < arity {
                return Err(SemaError::arity(
                    func.name
                        .map(resolve_spur)
                        .unwrap_or_else(|| "<lambda>".to_string()),
                    format!("{}+", arity),
                    argc,
                ));
            }
        } else if argc != arity {
            return Err(SemaError::arity(
                func.name
                    .map(resolve_spur)
                    .unwrap_or_else(|| "<lambda>".to_string()),
                arity.to_string(),
                argc,
            ));
        }

        // Copy args directly into current frame's base — no Vec allocation
        let func_idx = self.stack.len() - 1 - argc;
        let base = self.frames.last().unwrap().base;

        // Close open upvalues before overwriting stack slots
        if let Some(ref mut open) = self.frames.last_mut().unwrap().open_upvalues {
            close_open_upvalues(open, &self.stack, base);
        }

        // Copy args into base slots (args are above base, no overlap issues)
        Self::copy_args_to_locals(&mut self.stack, base, func_idx + 1, arity, argc, has_rest);

        // Resize to exact local count (pads with nil or truncates excess)
        self.stack.resize(base + n_locals, Value::nil());

        // Ensure inline cache has space for the target function
        self.ensure_cache_space(&closure.func);

        // Replace current frame (reuse slot)
        let frame = self.frames.last_mut().unwrap();
        frame.cache_base = closure.func.cache_offset;
        frame.closure = closure;
        frame.pc = 0;
        // base stays the same
        frame.open_upvalues = None;

        Ok(())
    }

    /// Self-recursive tail call (`Op::SelfTailCall`): the callee is the current
    /// frame's own closure, so no callee value is on the stack — only `argc`
    /// args. Reuses the frame in place (rebind args, jump to entry) reading the
    /// closure straight off the frame instead of a `LoadUpvalue`.
    ///
    /// Emitted only inside a loop lambda's own compiled `Function`, which runs
    /// solely as that closure's frame, so `frame.closure` is always the correct
    /// self. Because the self upvalue was elided by the resolver
    /// (`VarResolution::SelfFn`), no self cell is ever captured and no cycle
    /// forms.
    fn self_tail_call(&mut self, argc: usize) -> Result<(), SemaError> {
        let closure = self.frames.last().unwrap().closure.clone();
        let func = &closure.func;
        let arity = func.arity as usize;
        let has_rest = func.has_rest;
        let n_locals = func.chunk.n_locals as usize;

        // Arity check — a self-call can still be written with the wrong argument
        // count, e.g. `(let loop ((a 1) (b 2)) (loop 1))`.
        if has_rest {
            if argc < arity {
                return Err(SemaError::arity(
                    func.name
                        .map(resolve_spur)
                        .unwrap_or_else(|| "<lambda>".to_string()),
                    format!("{}+", arity),
                    argc,
                ));
            }
        } else if argc != arity {
            return Err(SemaError::arity(
                func.name
                    .map(resolve_spur)
                    .unwrap_or_else(|| "<lambda>".to_string()),
                arity.to_string(),
                argc,
            ));
        }

        // Args sit directly on top of this frame's locals (no callee slot).
        let src = self.stack.len() - argc;
        let base = self.frames.last().unwrap().base;

        // Close any open upvalue cells over this frame's locals before the args
        // overwrite them. The self upvalue is never captured, but the loop body
        // may still have made closures capturing OTHER loop locals.
        if let Some(ref mut open) = self.frames.last_mut().unwrap().open_upvalues {
            close_open_upvalues(open, &self.stack, base);
        }

        Self::copy_args_to_locals(&mut self.stack, base, src, arity, argc, has_rest);
        self.stack.resize(base + n_locals, Value::nil());

        // Reuse the frame: same closure, base and cache — just jump to entry.
        let frame = self.frames.last_mut().unwrap();
        frame.pc = 0;
        frame.open_upvalues = None;

        Ok(())
    }

    /// Copy args from the stack into local slots, handling rest params.
    /// `dst` is the base index for destination, `src` is the start of args.
    #[inline(always)]
    fn copy_args_to_locals(
        stack: &mut [Value],
        dst: usize,
        src: usize,
        arity: usize,
        argc: usize,
        has_rest: bool,
    ) {
        if has_rest {
            let rest: Vec<Value> = stack[src + arity..src + argc].to_vec();
            for i in 0..arity {
                stack[dst + i] = stack[src + i].clone();
            }
            stack[dst + arity] = Value::list(rest);
        } else {
            for i in 0..arity {
                stack[dst + i] = stack[src + i].clone();
            }
        }
    }

    /// Mirror a captured local's write into a detached-but-live `Tracked`
    /// upvalue cell (issue #104). When a closure escaped this frame onto a
    /// foreign stack (an `async/spawn` task, a fresh fallback VM, an inline-task
    /// HOF), an explicit-owner snapshot detached the shared cell to `Tracked`
    /// but left it registered in this frame's `open_upvalues`; the
    /// cell no longer reads this stack slot, so a plain stack write would be
    /// invisible to the task. Propagate the write into the cell's owned value so
    /// the task observes post-spawn mutations. Cheap no-op for the common cases:
    /// non-capturing frames (`open_upvalues == None`), uncaptured slots, and
    /// ordinary `Open`/`Closed` cells.
    #[inline]
    fn propagate_local_store_to_tracked(&self, fi: usize, slot: usize, val: &Value) {
        if let Some(open) = &self.frames[fi].open_upvalues {
            if let Some(Some(cell)) = open.get(slot) {
                if let UpvalueState::Tracked { value, .. } = &mut *cell.state.borrow_mut() {
                    *value = val.clone();
                }
            }
        }
    }

    // --- MakeClosure ---

    fn make_closure(&mut self) -> Result<(), SemaError> {
        // Read instruction operands from the current frame's bytecode.
        // We extract everything we need first, then release the borrow.
        let frame = self.frames.last().unwrap();
        let code = &frame.closure.func.chunk.code;
        let pc = frame.pc + 1;
        let func_id = u16::from_le_bytes([code[pc], code[pc + 1]]) as usize;
        let n_upvalues = u16::from_le_bytes([code[pc + 2], code[pc + 3]]) as usize;

        // Collect upvalue descriptors
        let mut uv_descs = Vec::with_capacity(n_upvalues);
        let mut uv_pc = pc + 4;
        for _ in 0..n_upvalues {
            let is_local = u16::from_le_bytes([code[uv_pc], code[uv_pc + 1]]);
            let idx = u16::from_le_bytes([code[uv_pc + 2], code[uv_pc + 3]]) as usize;
            uv_pc += 4;
            uv_descs.push((is_local != 0, idx));
        }

        let base = frame.base;
        let parent_upvalues = frame.closure.upvalues.clone();
        // Release the immutable borrow before mutating
        let _ = frame;

        let func = self.functions[func_id].clone();
        let mut upvalues = Vec::with_capacity(n_upvalues);

        for (is_local, idx) in &uv_descs {
            if *is_local {
                // Capture from current frame's local slot using a shared UpvalueCell.
                // Lazily allocate open_upvalues on first capture.
                let frame = self.frames.last_mut().unwrap();
                let n_locals = frame.closure.func.chunk.n_locals as usize;
                let open = frame
                    .open_upvalues
                    .get_or_insert_with(|| vec![None; n_locals]);
                let cell = if let Some(existing) = &open[*idx] {
                    existing.clone()
                } else {
                    // Create an OPEN cell pointing to the stack slot
                    let cell = Rc::new(UpvalueCell::new_open(base, *idx));
                    open[*idx] = Some(cell.clone());
                    cell
                };
                upvalues.push(cell);
            } else {
                // Capture from current frame's upvalue
                upvalues.push(parent_upvalues[*idx].clone());
            }
        }

        // Update pc past the entire instruction
        self.frames.last_mut().unwrap().pc = uv_pc;

        // Concretize the closure's home globals: the env the defining frame
        // resolves globals against. The defining frame's closure carries
        // `Some(home)` if it was itself a MakeClosure result, or `None` if it
        // is the top-level main closure — in which case the home is the VM's
        // own globals. Recording a concrete `Some(home)` here keeps the closure
        // correct if it is later exported and run inside a different VM (M1).
        let home_globals = match &self.frames.last().unwrap().closure.globals {
            Some(g) => g.clone(),
            None => self.globals.clone(),
        };

        // Cycle-collector candidates (CORE-2), registered after construction
        // below: the home env on first adoption, the new closure wrapper.
        // Home-adoption 1-entry cache: only clone (and later probe the
        // collector's seen-set for) the home when it isn't the one this VM
        // last adopted. The `Weak` guards address reuse — a dead entry
        // (strong count 0) never matches, even if a fresh env landed on the
        // same address.
        let home_for_gc: Option<Rc<Env>> = {
            let cached = self.gc_adopted_home.borrow();
            let hit = std::ptr::eq(cached.as_ptr(), Rc::as_ptr(&home_globals))
                && cached.strong_count() > 0;
            drop(cached);
            if hit {
                None
            } else {
                *self.gc_adopted_home.borrow_mut() = Rc::downgrade(&home_globals);
                Some(home_globals.clone())
            }
        };

        let closure = Rc::new(Closure {
            func,
            upvalues,
            globals: Some(home_globals),
            // The new closure's func-ids index the table the defining frame is
            // running against (set per frame activation in the dispatch loop).
            functions: Some(self.functions.clone()),
        });
        let payload = Rc::new(VmClosurePayload {
            closure,
            functions: self.functions.clone(),
            native_fns: self.native_fns.clone(),
        });
        // The fallback box captures ONLY this payload Rc (invariant I2): the
        // wrapper's strong edges into the closure graph are exactly payload ×2
        // (the `payload` field + the box), both traced through the registered
        // payload tracer. Closure/functions/globals are derived from it at call
        // time.
        let payload_for_box = Rc::clone(&payload);
        // Complete only when every fixed param has a name (a `.semac` load may
        // reconstruct an empty list); a partial list would misbind named args.
        let param_names = {
            let func = &payload.closure.func;
            (func.param_names.len() == func.arity as usize).then(|| Rc::clone(&func.param_names))
        };
        let name = payload
            .closure
            .func
            .name
            .map(resolve_spur)
            .unwrap_or_else(|| "<vm-closure>".to_string());

        // The NativeFn wrapper is used as a fallback when called from outside the VM
        // (e.g., from stdlib HOFs like map/filter). Inside the VM, call_value detects
        // the payload and pushes a CallFrame instead — no Rust recursion.
        let mut native_fn = sema_core::NativeFn::with_payload(
            name,
            payload as Rc<dyn std::any::Any>,
            move |ctx, args| {
                ensure_legacy_vm_entry_allowed(ctx)?;
                let closure = &payload_for_box.closure;
                let functions = &payload_for_box.functions;
                let globals = closure
                    .globals
                    .as_ref()
                    .expect("MakeClosure closures always carry Some(home)");

                // The explicit caller snapshots reachable Open cells before
                // this ownerless foreign-VM fallback is entered.
                close_closure_upvalues_for_foreign_run(closure);
                let mut vm = VM::new_with_rc_functions(
                    globals.clone(),
                    functions.clone(),
                    payload_for_box.native_fns.clone(),
                );
                vm.setup_for_call(closure.clone(), args)?;
                vm.run(ctx)
            },
        );
        // Mark this wrapper so `type`/`type_name` report `:lambda`, not `:native-fn`.
        native_fn.is_closure = true;
        native_fn.param_names = param_names;
        let native_rc = Rc::new(native_fn);

        // Zero-upvalue exemption: a closure that captured NO upvalues is not
        // registered as a cycle candidate (its home env still is). Sound
        // because it owns no upvalue cells, so it cannot carry the severable
        // link of any cycle (invariant I1) — every cycle it sits on closes
        // through a cell owned by something else that is independently
        // registered and reaches the whole cycle:
        //   - env bindings on its home's parent chain (the Env⇄Closure
        //     shapes): the home WRAPPER candidate registered by this very
        //     call traces `parent` edges through every ancestor env;
        //   - upvalue cells: owned by closures WITH upvalues (registered);
        //   - data cells (thunk/promise/channel/multimethod): their cold
        //     constructors register their own candidates, so even a data
        //     cell smuggled into chunk consts by a macro (reachable only
        //     through the shared function table) carries a covering
        //     candidate.
        // This keeps every top-level `(define (f ...))` — the common case —
        // out of the registry: live ones aren't re-traced each pass, garbage
        // ones die by plain Rc drop without a registry entry to prune.
        let candidate = (n_upvalues > 0).then_some(&native_rc);
        let should_collect = sema_core::register_closure_birth(home_for_gc.as_ref(), candidate);
        drop(home_for_gc);
        self.stack.push(Value::native_fn_from_rc(native_rc));

        // Threshold-gated safe point: closure creation is the hot site where
        // the candidate registry grows (cold data births run the same trigger
        // inside register_candidate), so churn workloads that never return
        // to a top-level safe point still collect. Mid-VM collection is safe —
        // live objects are protected by external strong counts (VM stack,
        // frames, open-upvalue refs) and any outstanding `RefCell` borrow
        // aborts the pass cleanly. Pins skip descent into the executing VM's
        // live global namespace; computed only when a pass will actually run.
        if should_collect {
            let pins = sema_core::gc_env_chain_pins(&self.globals);
            sema_core::gc_threshold_collect(&pins, sema_core::GcTrigger::Threshold);
        }
        Ok(())
    }

    // --- Exception handling ---

    #[cold]
    #[inline(never)]
    /// Capture the current VM call stack as a `StackTrace` for error reporting.
    ///
    /// Walks `self.frames` top-to-bottom (innermost first), mirroring
    /// `debug_stack_trace` but producing `sema_core::CallFrame` instead of
    /// `DapStackFrame`. For the innermost frame, decodes the opcode at
    /// `failing_pc` to synthesize a leading intrinsic frame (e.g. `+`, `car`)
    /// when the error originated from an inline opcode rather than a function
    /// call.
    fn capture_vm_stack_trace(&self, failing_pc: usize) -> StackTrace {
        let mut frames: Vec<CoreCallFrame> = Vec::new();

        // Try to synthesize an intrinsic frame for the innermost opcode.
        if let Some(top) = self.frames.last() {
            let code = &top.closure.func.chunk.code;
            if failing_pc < code.len() {
                let opcode = Op::from_u8(code[failing_pc]);
                if let Some(name) = opcode.and_then(intrinsic_name) {
                    let span = self.span_at_pc_raw(top, failing_pc);
                    frames.push(CoreCallFrame {
                        name: name.to_string(),
                        file: top.closure.func.source_file.clone(),
                        span,
                    });
                }
            }
        }

        // Walk frames innermost-to-outermost.
        for (i, frame) in self.frames.iter().rev().enumerate() {
            let func = &frame.closure.func;
            let name = func
                .name
                .map(resolve_spur)
                .unwrap_or_else(|| "<lambda>".to_string());
            // Use failing_pc for the innermost frame, frame.pc for the rest.
            let pc = if i == 0 { failing_pc } else { frame.pc };
            let span = self.span_at_pc_raw(frame, pc);
            frames.push(CoreCallFrame {
                name,
                file: func.source_file.clone(),
                span,
            });
        }

        StackTrace(frames)
    }

    fn handle_exception(
        &mut self,
        mut err: SemaError,
        failing_pc: usize,
    ) -> Result<ExceptionAction, SemaError> {
        // Capture the stack trace before unwinding frames.
        let trace = self.capture_vm_stack_trace(failing_pc);
        err = err.with_stack_trace(trace);

        let mut pc_for_lookup = failing_pc as u32;
        // Walk frames from top looking for a handler.
        while !self.frames.is_empty() {
            let frame = self.frames.last().unwrap();
            let chunk = &frame.closure.func.chunk;

            // Check exception table for this frame
            let mut found = None;
            for entry in &chunk.exception_table {
                if pc_for_lookup >= entry.try_start && pc_for_lookup < entry.try_end {
                    found = Some(entry.clone());
                    break;
                }
            }

            if let Some(entry) = found {
                // Close open upvalues above the handler's stack depth
                let base = frame.base;
                if let Some(ref mut open) = self.frames.last_mut().unwrap().open_upvalues {
                    close_open_upvalues_above(open, &self.stack, base, entry.stack_depth as usize);
                }
                // Restore stack to handler state
                self.stack.truncate(base + entry.stack_depth as usize);

                // Push error value as a map
                let error_val = error_to_value(&err);
                self.stack.push(error_val);

                // Jump to handler
                let frame = self.frames.last_mut().unwrap();
                frame.pc = entry.handler_pc as usize;
                return Ok(ExceptionAction::Handled);
            }

            // Close all open upvalues before popping this frame
            let base = self.frames.last().unwrap().base;
            if let Some(ref mut open) = self.frames.last_mut().unwrap().open_upvalues {
                close_open_upvalues(open, &self.stack, base);
            }
            // No handler in this frame, pop it and try parent
            let frame = self.frames.pop().unwrap();
            self.stack.truncate(frame.base);
            // Parent frames use their own pc for lookup.
            // parent.pc is the *resume* PC (the byte after the CALL instruction).
            // Exception table intervals are half-open [try_start, try_end), so if the
            // CALL was the last instruction in the try body, parent.pc == try_end and
            // the lookup would miss. Subtract 1 to land inside the CALL instruction.
            if let Some(parent) = self.frames.last() {
                pc_for_lookup = parent.pc.saturating_sub(1) as u32;
            }
        }

        // No handler found anywhere
        Ok(ExceptionAction::Propagate(err))
    }

    // --- Debug inspection methods ---

    pub fn debug_frame_count(&self) -> usize {
        self.frames.len()
    }

    pub fn debug_stack_trace(&self) -> Vec<crate::debug::DapStackFrame> {
        self.frames
            .iter()
            .enumerate()
            .rev()
            .map(|(i, frame)| {
                let func = &frame.closure.func;
                let name = func
                    .name
                    .map(sema_core::resolve)
                    .unwrap_or_else(|| "<main>".to_string());
                let (line, col) = self.span_at_pc(frame);
                crate::debug::DapStackFrame {
                    id: i as u64,
                    name,
                    line,
                    column: col,
                    source_file: func.source_file.clone(),
                }
            })
            .collect()
    }

    /// Locals in scope at the frame's current pc, as `(slot, name-spur)`, with
    /// the innermost binding chosen when a name is shadowed by nested blocks.
    ///
    /// A slot with recorded block scopes (`let`/`do` bindings) is in scope only
    /// while pc lies within one of them — hiding not-yet-bound and already-exited
    /// block locals. Params and slots with no recorded scope (e.g. functions
    /// loaded from bytecode, which carry no `local_scopes`) are always in scope.
    /// This is the single source of truth used by the locals display and by the
    /// `setVariable` / `set!` write-back and `evaluate` read paths, so all three
    /// resolve a shadowed name to the same slot.
    fn in_scope_locals(&self, frame_id: usize) -> Vec<(u16, Spur)> {
        let Some(frame) = self.frames.get(frame_id) else {
            return Vec::new();
        };
        let pc = frame.pc as u32;
        let func = &frame.closure.func;
        // name -> (slot, spur, priority); higher priority = innermost block.
        let mut chosen: hashbrown::HashMap<String, (u16, Spur, u32)> = hashbrown::HashMap::new();
        for &(slot, spur) in &func.local_names {
            let mut scopes = func
                .local_scopes
                .iter()
                .filter(|(s, _, _)| *s == slot)
                .peekable();
            let priority = if scopes.peek().is_none() {
                0 // param / no scope info: always in scope, lowest priority
            } else {
                match scopes
                    .filter(|(_, start, end)| pc >= *start && pc < *end)
                    .map(|(_, start, _)| *start)
                    .max()
                {
                    Some(start) => start.saturating_add(1),
                    None => continue, // out of scope at this pc
                }
            };
            let name = sema_core::resolve(spur);
            match chosen.get(&name) {
                Some((_, _, p)) if *p >= priority => {}
                _ => {
                    chosen.insert(name, (slot, spur, priority));
                }
            }
        }
        let mut result: Vec<(u16, Spur)> = chosen
            .into_values()
            .map(|(slot, spur, _)| (slot, spur))
            .collect();
        result.sort_by_key(|(slot, _)| *slot);
        result
    }

    pub fn debug_locals(&mut self, frame_idx: usize) -> Vec<crate::debug::DapVariable> {
        let Some(base) = self.frames.get(frame_idx).map(|f| f.base) else {
            return Vec::new();
        };
        let in_scope = self.in_scope_locals(frame_idx);
        let mut vars = Vec::new();
        for (slot, spur) in in_scope {
            let idx = base + slot as usize;
            let val = self.stack.get(idx).cloned().unwrap_or(Value::nil());
            vars.push(self.debug_value_to_variable(&sema_core::resolve(spur), val));
        }
        vars
    }

    pub fn debug_upvalues(&mut self, frame_idx: usize) -> Vec<crate::debug::DapVariable> {
        let Some(frame) = self.frames.get(frame_idx) else {
            return Vec::new();
        };
        let upvalues = frame.closure.upvalues.clone();
        let names = frame.closure.func.upvalue_names.clone();
        upvalues
            .iter()
            .enumerate()
            .map(|(i, uv)| {
                let val = match &*uv.state.borrow() {
                    UpvalueState::Closed(v) => v.clone(),
                    UpvalueState::Tracked { value, .. } => value.clone(),
                    UpvalueState::Open { frame_base, slot } => {
                        self.stack[*frame_base + *slot].clone()
                    }
                };
                let name = names
                    .get(i)
                    .map(|spur| sema_core::resolve(*spur))
                    .unwrap_or_else(|| format!("upvalue_{i}"));
                self.debug_value_to_variable(&name, val)
            })
            .collect()
    }

    pub fn debug_scopes(&mut self, frame_id: usize) -> Vec<crate::debug::DapScope> {
        let mut scopes = vec![crate::debug::DapScope {
            name: "Locals".to_string(),
            variables_reference: crate::debug::scope_locals_ref(frame_id),
            expensive: false,
        }];
        if !self.debug_upvalues(frame_id).is_empty() {
            scopes.push(crate::debug::DapScope {
                name: "Closure".to_string(),
                variables_reference: crate::debug::scope_upvalues_ref(frame_id),
                expensive: false,
            });
        }
        scopes
    }

    pub fn debug_variables(&mut self, reference: u64) -> Vec<crate::debug::DapVariable> {
        if let Some(value) = self.debug_values.get(&reference).cloned() {
            return self.debug_children(value);
        }
        match crate::debug::decode_scope_ref(reference) {
            None => Vec::new(),
            Some(crate::debug::ScopeKind::Locals(frame_id)) => self.debug_locals(frame_id),
            Some(crate::debug::ScopeKind::Upvalues(frame_id)) => self.debug_upvalues(frame_id),
        }
    }

    /// Evaluate a debugger expression, writing through any top-level
    /// `(set! <local-or-upvalue> <value>)` to the real frame.
    ///
    /// Plain `debug_evaluate` runs in a throwaway env that copies locals/upvalues
    /// by value, so a `set!` on a local would only mutate that scratch env and
    /// silently fail to persist. This mut variant detects that case and routes
    /// the assignment through the same write-back path as `setVariable`, keeping
    /// the two requests consistent.
    ///
    /// Precedence rules for the write-back short-circuit (all must hold; otherwise
    /// the expression is handed to the normal evaluator unchanged):
    ///
    /// 1. The expression must be syntactically a builtin `set!` form — exactly
    ///    `(set! <symbol> <value-expr>)`. Anything else (wrong arity, non-symbol
    ///    target, head not the `set!` symbol) is evaluated normally.
    /// 2. The head `set!` must NOT be shadowed by an in-scope local or upvalue in
    ///    this frame. If the user rebound `set!` (e.g. `(let ((set! ...)) ...)`),
    ///    the form is an ordinary call to that binding, not the assignment special
    ///    form, so we must not hijack it.
    /// 3. The assignment target must name an in-scope frame binding (local
    ///    preferred over upvalue, matching the locals display). If it names a
    ///    global or an unknown symbol, the normal evaluator handles it so global
    ///    `set!` semantics are preserved.
    pub fn debug_evaluate_mut(
        &mut self,
        frame_id: usize,
        expr: &Value,
        ctx: &EvalContext,
        debug: &crate::debug::DebugState,
    ) -> Result<Value, SemaError> {
        if let Some((target, value_expr)) = Self::as_local_set(expr) {
            // Rule 2: don't hijack a `set!` that the user has rebound in-frame.
            let set_shadowed = self.frame_has_binding(frame_id, "set!");
            let name = sema_core::resolve(target);
            // Rule 3: only write back to an actual frame binding.
            if !set_shadowed && self.frame_has_binding(frame_id, &name) {
                let value = self.debug_evaluate(frame_id, &value_expr, ctx, debug)?;
                self.debug_set_named(frame_id, &name, value.clone())?;
                return Ok(value);
            }
        }
        self.debug_evaluate(frame_id, expr, ctx, debug)
    }

    /// If `expr` is a builtin `set!` form `(set! <symbol> <value-expr>)`, return
    /// the target symbol and the value expression. Returns `None` for anything
    /// that is not syntactically that exact shape — including a head symbol other
    /// than `set!`, the wrong number of arguments, or a non-symbol target. The
    /// caller is responsible for confirming that the head `set!` is the builtin
    /// special form and not a shadowing in-scope binding (see `debug_evaluate_mut`).
    fn as_local_set(expr: &Value) -> Option<(Spur, Value)> {
        let items = expr.as_list()?;
        if items.len() != 3 {
            return None;
        }
        let head = items[0].as_symbol_spur()?;
        if sema_core::resolve(head) != "set!" {
            return None;
        }
        let target = items[1].as_symbol_spur()?;
        Some((target, items[2].clone()))
    }

    /// True if `name` is a local or upvalue of the given frame.
    /// The slot of the in-scope local named `name` at the frame's current pc, if
    /// any (innermost when shadowed). Shared by the write/read paths so they
    /// agree with the locals display on which binding a name refers to.
    fn in_scope_local_slot(&self, frame_id: usize, name: &str) -> Option<u16> {
        self.in_scope_locals(frame_id)
            .into_iter()
            .find(|(_, spur)| sema_core::resolve(*spur) == name)
            .map(|(slot, _)| slot)
    }

    fn frame_has_binding(&self, frame_id: usize, name: &str) -> bool {
        if self.in_scope_local_slot(frame_id, name).is_some() {
            return true;
        }
        self.frames.get(frame_id).is_some_and(|frame| {
            frame
                .closure
                .func
                .upvalue_names
                .iter()
                .any(|spur| sema_core::resolve(*spur) == name)
        })
    }

    /// Write `value` back to the local (preferred) or upvalue named `name`.
    fn debug_set_named(
        &mut self,
        frame_id: usize,
        name: &str,
        value: Value,
    ) -> Result<crate::debug::DapVariable, SemaError> {
        if let Some(slot) = self.in_scope_local_slot(frame_id, name) {
            self.debug_set_local_slot(frame_id, slot, name, value)
        } else {
            self.debug_set_upvalue(frame_id, name, value)
        }
    }

    /// Decide whether a debug stop should actually fire, applying any
    /// conditional-breakpoint expression. Returns `true` to stop.
    ///
    /// Only *pure* breakpoint stops (no pending pause, no step that would land
    /// here on its own) are gated: a stop that is also a step/pause stop always
    /// fires. The condition is evaluated against the topmost (innermost) frame.
    /// If the condition fails to parse or evaluate we fail open and stop, so a
    /// bad condition surfaces to the user rather than silently swallowing the
    /// breakpoint.
    fn debug_condition_allows_stop(
        &mut self,
        file: Option<&std::path::PathBuf>,
        line: u32,
        debug: &crate::debug::DebugState,
        ctx: &EvalContext,
    ) -> bool {
        let frame_depth = self.frames.len();
        if !debug.is_pure_breakpoint_stop(file, line, frame_depth) {
            return true;
        }
        let Some(condition) = debug.condition_at(file, line) else {
            return true;
        };
        if self.frames.is_empty() {
            return true;
        }
        let frame_id = self.frames.len() - 1;
        match sema_reader::read(condition) {
            Ok(expr) => match self.debug_evaluate(frame_id, &expr, ctx, debug) {
                Ok(value) => value.is_truthy(),
                Err(_) => true,
            },
            Err(_) => true,
        }
    }

    pub fn debug_evaluate(
        &mut self,
        frame_id: usize,
        expr: &Value,
        ctx: &EvalContext,
        _debug: &crate::debug::DebugState,
    ) -> Result<Value, SemaError> {
        let env = self.debug_env_for_frame(frame_id)?;
        let cancellation = self.quantum_cancellation.clone();
        let deadline = ctx.eval_deadline.get();
        let task_context = ctx.task_context().unwrap_or_default();
        let mut traversal_budget = DebugTraversalBudget::new(cancellation.clone(), deadline);
        let mut rollback = DebugUpvalueRollback::new();
        DebugValueGraphWalker::capture_stopped_bindings(
            self,
            &env,
            &mut rollback,
            &mut traversal_budget,
        )?;
        DebugValueGraphWalker::snapshot_reachable(self, Rc::clone(&env), &mut traversal_budget)?;
        rollback.capture_owner_frames(self, &mut traversal_budget)?;
        traversal_budget.check_boundary()?;
        let native_context = NativeCallContext {
            eval_context: ctx,
            task_context: task_context.clone(),
            call_env: Some(Rc::clone(&env)),
            cancellation: cancellation.clone(),
        };
        let result = (|| {
            let expanded = sema_core::try_macro_expand_callback(&native_context, expr, &env)
                .transpose()?
                .unwrap_or_else(|| expr.clone());
            if expanded.is_nil() {
                return Ok(Value::nil());
            }
            let program = compile_program(std::slice::from_ref(&expanded), None)?;
            run_program_restricted(
                ctx,
                task_context,
                program,
                env,
                RestrictedRunPolicy {
                    operation: "debug evaluation",
                    suspension_error: "debug evaluation cannot suspend",
                    instruction_limit: NonZeroUsize::new(DEBUG_EVALUATION_INSTRUCTION_LIMIT)
                        .expect("debug evaluation instruction limit is nonzero"),
                    transition_limit: NonZeroUsize::new(DEBUG_EVALUATION_TRANSITION_LIMIT)
                        .expect("debug evaluation transition limit is nonzero"),
                    deadline,
                    cancellation,
                },
            )
        })();
        match result {
            Ok(value) => {
                self.sync_tracked_upvalues_to_stack();
                Ok(value)
            }
            Err(error) => {
                rollback.restore();
                Err(error)
            }
        }
    }

    pub fn debug_set_variable(
        &mut self,
        variables_reference: u64,
        name: &str,
        value: Value,
    ) -> Result<crate::debug::DapVariable, SemaError> {
        let target = self.resolve_debug_variable_target(variables_reference, name)?;
        self.write_debug_variable_target(target, name, value)
    }

    fn debug_set_variable_expression(
        &mut self,
        variables_reference: u64,
        name: &str,
        value_expression: &str,
        ctx: &EvalContext,
        debug: &crate::debug::DebugState,
    ) -> Result<crate::debug::DapVariable, String> {
        let target = self
            .resolve_debug_variable_target(variables_reference, name)
            .map_err(|error| error.to_string())?;
        let expression = sema_reader::read(value_expression).map_err(|error| error.to_string())?;
        let value = self
            .debug_evaluate(target.frame_id(), &expression, ctx, debug)
            .map_err(|error| error.to_string())?;
        self.write_debug_variable_target(target, name, value)
            .map_err(|error| error.to_string())
    }

    fn resolve_debug_variable_target(
        &self,
        variables_reference: u64,
        name: &str,
    ) -> Result<DebugVariableTarget, SemaError> {
        match crate::debug::decode_scope_ref(variables_reference) {
            Some(crate::debug::ScopeKind::Locals(frame_id)) => {
                let Some(slot) = self.in_scope_local_slot(frame_id, name) else {
                    return Err(SemaError::eval(format!(
                        "setVariable: local '{name}' not found"
                    )));
                };
                let frame = self.frames.get(frame_id).ok_or_else(|| {
                    SemaError::eval(format!("setVariable: invalid frame id {frame_id}"))
                })?;
                let stack_index = frame.base + usize::from(slot);
                if self.stack.get(stack_index).is_none() {
                    return Err(SemaError::eval(format!(
                        "setVariable: local '{name}' is out of range"
                    )));
                }
                Ok(DebugVariableTarget::Local {
                    frame_id,
                    slot: usize::from(slot),
                    stack_index,
                })
            }
            Some(crate::debug::ScopeKind::Upvalues(frame_id)) => {
                let frame = self.frames.get(frame_id).ok_or_else(|| {
                    SemaError::eval(format!("setVariable: invalid frame id {frame_id}"))
                })?;
                let index = if let Some(index) = name
                    .strip_prefix("upvalue_")
                    .and_then(|suffix| suffix.parse::<usize>().ok())
                {
                    index
                } else if let Some(index) = frame
                    .closure
                    .func
                    .upvalue_names
                    .iter()
                    .position(|spur| sema_core::resolve(*spur) == name)
                {
                    index
                } else {
                    return Err(SemaError::eval(format!(
                        "setVariable: upvalue '{name}' not found"
                    )));
                };
                let Some(cell) = frame.closure.upvalues.get(index).cloned() else {
                    return Err(SemaError::eval(format!(
                        "setVariable: upvalue '{name}' not found"
                    )));
                };
                if let UpvalueState::Open { frame_base, slot } = &*cell.state.borrow() {
                    if self.stack.get(*frame_base + *slot).is_none() {
                        return Err(SemaError::eval(format!(
                            "setVariable: upvalue '{name}' is out of range"
                        )));
                    }
                }
                Ok(DebugVariableTarget::Upvalue { frame_id, cell })
            }
            None => Err(SemaError::eval(
                "setVariable: invalid variablesReference".to_string(),
            )),
        }
    }

    fn write_debug_variable_target(
        &mut self,
        target: DebugVariableTarget,
        name: &str,
        value: Value,
    ) -> Result<crate::debug::DapVariable, SemaError> {
        match target {
            DebugVariableTarget::Local {
                frame_id,
                slot,
                stack_index,
            } => {
                let Some(slot_value) = self.stack.get_mut(stack_index) else {
                    return Err(SemaError::eval(format!(
                        "setVariable: local '{name}' is out of range"
                    )));
                };
                *slot_value = value.clone();
                self.propagate_local_store_to_tracked(frame_id, slot, &value);
            }
            DebugVariableTarget::Upvalue { cell, .. } => {
                let wrote_tracked = {
                    let mut state = cell.state.borrow_mut();
                    match &mut *state {
                        UpvalueState::Closed(slot_value) => {
                            *slot_value = value.clone();
                            false
                        }
                        UpvalueState::Tracked {
                            value: tracked_value,
                            ..
                        } => {
                            *tracked_value = value.clone();
                            true
                        }
                        UpvalueState::Open { frame_base, slot } => {
                            let Some(slot_value) = self.stack.get_mut(*frame_base + *slot) else {
                                return Err(SemaError::eval(format!(
                                    "setVariable: upvalue '{name}' is out of range"
                                )));
                            };
                            *slot_value = value.clone();
                            false
                        }
                    }
                };
                if wrote_tracked {
                    self.sync_tracked_upvalues_to_stack();
                }
            }
        }
        Ok(self.debug_value_to_variable(name, value))
    }

    fn span_at_pc(&self, frame: &CallFrame) -> (u64, u64) {
        match self.span_at_pc_raw(frame, frame.pc) {
            Some(span) => (span.line as u64, span.col as u64 + 1),
            None => (0, 0),
        }
    }

    /// Resolve a `Span` from the chunk's span table at a given PC.
    fn span_at_pc_raw(&self, frame: &CallFrame, pc: usize) -> Option<sema_core::error::Span> {
        let pc32 = pc as u32;
        let spans = &frame.closure.func.chunk.spans;
        match spans.binary_search_by_key(&pc32, |(p, _)| *p) {
            Ok(idx) => Some(spans[idx].1),
            Err(idx) if idx > 0 => Some(spans[idx - 1].1),
            _ => None,
        }
    }

    fn debug_env_for_frame(&self, frame_id: usize) -> Result<Rc<Env>, SemaError> {
        let frame = self.frames.get(frame_id).ok_or_else(|| {
            SemaError::eval(format!("debug evaluate: invalid frame id {frame_id}"))
        })?;
        let env = Rc::new(Env::with_parent(self.globals.clone()));

        for (i, upvalue) in frame.closure.upvalues.iter().enumerate() {
            let value = self.debug_upvalue_value(upvalue);
            env.set(sema_core::intern(&format!("upvalue_{i}")), value.clone());
            if let Some(name) = frame.closure.func.upvalue_names.get(i) {
                env.set(*name, value);
            }
        }

        // Inject only the locals in scope at the current pc (innermost binding
        // for shadowed names), so an evaluated expression sees the same binding
        // the locals display and setVariable resolve to.
        for (slot, spur) in self.in_scope_locals(frame_id) {
            let idx = frame.base + slot as usize;
            if let Some(value) = self.stack.get(idx) {
                env.set(spur, value.clone());
            }
        }

        Ok(env)
    }

    fn debug_upvalue_value(&self, upvalue: &UpvalueCell) -> Value {
        match &*upvalue.state.borrow() {
            UpvalueState::Closed(value) => value.clone(),
            UpvalueState::Tracked { value, .. } => value.clone(),
            UpvalueState::Open { frame_base, slot } => self
                .stack
                .get(*frame_base + *slot)
                .cloned()
                .unwrap_or_else(Value::nil),
        }
    }

    /// Write `value` to a specific local `slot` of the frame, resolving the
    /// stack index from the frame base. The caller has already mapped the name
    /// to the pc-active slot (so shadowed locals write the binding actually in
    /// scope, matching the locals display).
    fn debug_set_local_slot(
        &mut self,
        frame_id: usize,
        slot: u16,
        name: &str,
        value: Value,
    ) -> Result<crate::debug::DapVariable, SemaError> {
        let base = self
            .frames
            .get(frame_id)
            .ok_or_else(|| SemaError::eval(format!("setVariable: invalid frame id {frame_id}")))?
            .base;
        let idx = base + slot as usize;
        let Some(slot_value) = self.stack.get_mut(idx) else {
            return Err(SemaError::eval(format!(
                "setVariable: local '{name}' is out of range"
            )));
        };
        *slot_value = value.clone();
        self.propagate_local_store_to_tracked(frame_id, slot as usize, &value);
        Ok(self.debug_value_to_variable(name, value))
    }

    fn debug_set_upvalue(
        &mut self,
        frame_id: usize,
        name: &str,
        value: Value,
    ) -> Result<crate::debug::DapVariable, SemaError> {
        let frame = self
            .frames
            .get(frame_id)
            .ok_or_else(|| SemaError::eval(format!("setVariable: invalid frame id {frame_id}")))?;
        let index = if let Some(index) = name
            .strip_prefix("upvalue_")
            .and_then(|suffix| suffix.parse::<usize>().ok())
        {
            index
        } else if let Some(index) = frame
            .closure
            .func
            .upvalue_names
            .iter()
            .position(|spur| sema_core::resolve(*spur) == name)
        {
            index
        } else {
            return Err(SemaError::eval(format!(
                "setVariable: upvalue '{name}' not found"
            )));
        };
        let Some(upvalue) = frame.closure.upvalues.get(index) else {
            return Err(SemaError::eval(format!(
                "setVariable: upvalue '{name}' not found"
            )));
        };

        let wrote_tracked = {
            let mut state = upvalue.state.borrow_mut();
            match &mut *state {
                UpvalueState::Closed(slot_value) => {
                    *slot_value = value.clone();
                    false
                }
                UpvalueState::Tracked {
                    value: tracked_value,
                    ..
                } => {
                    *tracked_value = value.clone();
                    true
                }
                UpvalueState::Open { frame_base, slot } => {
                    let Some(slot_value) = self.stack.get_mut(*frame_base + *slot) else {
                        return Err(SemaError::eval(format!(
                            "setVariable: upvalue '{name}' is out of range"
                        )));
                    };
                    *slot_value = value.clone();
                    false
                }
            }
        };
        if wrote_tracked {
            self.sync_tracked_upvalues_to_stack();
        }

        Ok(self.debug_value_to_variable(name, value))
    }

    fn debug_value_to_variable(&mut self, name: &str, value: Value) -> crate::debug::DapVariable {
        let variables_reference = self.debug_expandable_ref(&value);
        crate::debug::DapVariable {
            name: name.to_string(),
            value: sema_core::pretty_print(&value, 80),
            type_name: value.type_name().to_string(),
            variables_reference,
        }
    }

    fn debug_expandable_ref(&mut self, value: &Value) -> u64 {
        if !Self::is_debug_expandable(value) {
            return 0;
        }
        let reference = self.next_debug_value_ref;
        self.next_debug_value_ref += 1;
        self.debug_values.insert(reference, value.clone());
        reference
    }

    fn is_debug_expandable(value: &Value) -> bool {
        matches!(
            value.view_ref(),
            ValueViewRef::List(_)
                | ValueViewRef::Vector(_)
                | ValueViewRef::Map(_)
                | ValueViewRef::HashMap(_)
                | ValueViewRef::Record(_)
                | ValueViewRef::Bytevector(_)
        )
    }

    fn debug_children(&mut self, value: Value) -> Vec<crate::debug::DapVariable> {
        match value.view_ref() {
            ValueViewRef::List(items) | ValueViewRef::Vector(items) => items
                .iter()
                .enumerate()
                .map(|(i, child)| self.debug_value_to_variable(&format!("[{i}]"), child.clone()))
                .collect(),
            ValueViewRef::Map(map) => map
                .iter()
                .map(|(key, child)| {
                    self.debug_value_to_variable(&sema_core::pretty_print(key, 80), child.clone())
                })
                .collect(),
            ValueViewRef::HashMap(map) => {
                let mut entries: Vec<_> = map.iter().collect();
                entries.sort_by_key(|(key, _)| (*key).clone());
                entries
                    .into_iter()
                    .map(|(key, child)| {
                        self.debug_value_to_variable(
                            &sema_core::pretty_print(key, 80),
                            child.clone(),
                        )
                    })
                    .collect()
            }
            ValueViewRef::Record(record) => record
                .fields
                .iter()
                .enumerate()
                .map(|(i, child)| {
                    let name = if record.field_names.len() == record.fields.len() {
                        sema_core::resolve(record.field_names[i])
                    } else {
                        format!("field_{i}")
                    };
                    self.debug_value_to_variable(&name, child.clone())
                })
                .collect(),
            ValueViewRef::Bytevector(bytes) => bytes
                .iter()
                .enumerate()
                .map(|(i, byte)| {
                    self.debug_value_to_variable(&format!("[{i}]"), Value::int(*byte as i64))
                })
                .collect(),
            _ => Vec::new(),
        }
    }
}

enum ExceptionAction {
    Handled,
    Propagate(SemaError),
}

/// Convert a SemaError into a Sema map value.
fn error_to_value(err: &SemaError) -> Value {
    let inner = err.inner();
    // A re-raised condition IS its map already — hand it back unchanged so
    // every catch layer binds the same value (its :stack-trace stays the
    // original error's; the re-throw site adds nothing).
    if let SemaError::Condition(condition) = inner {
        return condition.clone();
    }
    let mut map = BTreeMap::new();
    match inner {
        SemaError::Eval(msg) => {
            map.insert(Value::keyword("type"), Value::keyword("eval"));
            map.insert(Value::keyword("message"), Value::string(msg));
        }
        SemaError::Type { expected, got, .. } => {
            map.insert(Value::keyword("type"), Value::keyword("type-error"));
            map.insert(
                Value::keyword("message"),
                Value::string(&format!("expected {expected}, got {got}")),
            );
            map.insert(Value::keyword("expected"), Value::string(expected));
            map.insert(Value::keyword("got"), Value::string(got));
        }
        SemaError::Arity {
            name,
            expected,
            got,
        } => {
            map.insert(Value::keyword("type"), Value::keyword("arity"));
            map.insert(
                Value::keyword("message"),
                Value::string(&format!("{name} expects {expected} args, got {got}")),
            );
        }
        SemaError::Unbound(name) => {
            map.insert(Value::keyword("type"), Value::keyword("unbound"));
            map.insert(
                Value::keyword("message"),
                Value::string(&format!("Unbound variable: {name}")),
            );
            map.insert(Value::keyword("name"), Value::string(name));
        }
        SemaError::UserException(val) => {
            map.insert(Value::keyword("type"), Value::keyword("user"));
            map.insert(Value::keyword("message"), Value::string(&val.to_string()));
            map.insert(Value::keyword("value"), val.clone());
        }
        SemaError::Io(msg) => {
            map.insert(Value::keyword("type"), Value::keyword("io"));
            map.insert(Value::keyword("message"), Value::string(msg));
        }
        SemaError::Llm(msg) => {
            map.insert(Value::keyword("type"), Value::keyword("llm"));
            map.insert(Value::keyword("message"), Value::string(msg));
        }
        SemaError::Reader { message, span } => {
            map.insert(Value::keyword("type"), Value::keyword("reader"));
            map.insert(
                Value::keyword("message"),
                Value::string(&format!("{message} at {span}")),
            );
        }
        SemaError::PermissionDenied {
            function,
            capability,
        } => {
            map.insert(Value::keyword("type"), Value::keyword("permission-denied"));
            map.insert(
                Value::keyword("message"),
                Value::string(&format!(
                    "Permission denied: {function} requires '{capability}' capability"
                )),
            );
            map.insert(Value::keyword("function"), Value::string(function));
            map.insert(Value::keyword("capability"), Value::string(capability));
        }
        SemaError::PathDenied { function, path } => {
            map.insert(Value::keyword("type"), Value::keyword("permission-denied"));
            map.insert(
                Value::keyword("message"),
                Value::string(&format!(
                    "Permission denied: {function} — path '{path}' is outside allowed directories"
                )),
            );
            map.insert(Value::keyword("function"), Value::string(function));
            map.insert(Value::keyword("path"), Value::string(path));
        }
        SemaError::WithTrace { .. } | SemaError::WithContext { .. } => {
            unreachable!("inner() already unwraps these")
        }
        SemaError::Condition(_) => {
            unreachable!("handled by the early return above")
        }
    }

    // Serialize stack trace if present
    if let Some(trace) = err.stack_trace() {
        let frames: Vec<Value> = trace
            .0
            .iter()
            .map(|frame| {
                let mut fm = BTreeMap::new();
                fm.insert(Value::keyword("name"), Value::string(&frame.name));
                if let Some(file) = &frame.file {
                    fm.insert(
                        Value::keyword("file"),
                        Value::string(&file.display().to_string()),
                    );
                }
                if let Some(span) = &frame.span {
                    fm.insert(Value::keyword("line"), Value::int(span.line as i64));
                    fm.insert(Value::keyword("col"), Value::int(span.col as i64));
                }
                Value::map(fm)
            })
            .collect();
        map.insert(Value::keyword("stack-trace"), Value::list(frames));
    }

    Value::map(map)
}

// --- Stack trace intrinsic name mapping ---

/// Map an inline opcode to its Sema-level name for stack trace frames.
/// Returns `None` for opcodes that don't correspond to a user-visible
/// operation (e.g. `Const`, `Pop`, `Jump`).
fn intrinsic_name(opcode: Op) -> Option<&'static str> {
    match opcode {
        Op::Add | Op::AddInt => Some("+"),
        Op::Sub | Op::SubInt => Some("-"),
        Op::Mul | Op::MulInt => Some("*"),
        Op::Div => Some("/"),
        Op::Mod => Some("mod"),
        Op::Negate => Some("-"),
        Op::Not => Some("not"),
        Op::Eq | Op::EqInt => Some("="),
        Op::Lt | Op::LtInt => Some("<"),
        Op::Gt => Some(">"),
        Op::Le => Some("<="),
        Op::Ge => Some(">="),
        Op::Car => Some("car"),
        Op::Cdr => Some("cdr"),
        Op::Cons => Some("cons"),
        Op::IsNull => Some("null?"),
        Op::IsPair => Some("pair?"),
        Op::IsList => Some("list?"),
        Op::IsNumber => Some("number?"),
        Op::IsString => Some("string?"),
        Op::IsSymbol => Some("symbol?"),
        Op::Length => Some("length"),
        Op::Append => Some("append"),
        Op::Get => Some("get"),
        Op::ContainsQ => Some("contains?"),
        Op::Nth => Some("nth"),
        Op::StringLength => Some("string-length"),
        Op::StringRef => Some("string-ref"),
        Op::StringAppend => Some("string-append"),
        Op::MutArrGet => Some("mutable-array/get"),
        Op::MutArrSet => Some("mutable-array/set!"),
        Op::Throw => Some("throw"),
        _ => None,
    }
}

// --- Arithmetic helpers ---

#[inline(always)]
/// Hint for `get`/`contains?` called on the wrong collection type. These work
/// on maps only; users from Clojure expect them to index vectors too, so when
/// the collection is a list/vector we redirect them to `nth`.
fn map_access_hint(func: &str, coll: &Value) -> String {
    if coll.as_list().is_some() || coll.as_vector().is_some() {
        format!("{func} works on maps; use (nth coll i) to index a list or vector")
    } else {
        format!("{func}: expected a map as the first argument")
    }
}

fn vm_add(a: &Value, b: &Value) -> Result<Value, SemaError> {
    match (a.view_ref(), b.view_ref()) {
        (ValueViewRef::Int(x), ValueViewRef::Int(y)) => match x.checked_add(y) {
            Some(s) => Ok(Value::int(s)),
            // i64 overflow promotes to a bignum instead of raising.
            None => Ok(Value::from_number(
                SemaNumber::from_i64(x).add(SemaNumber::from_i64(y)),
            )),
        },
        (ValueViewRef::Float(x), ValueViewRef::Float(y)) => Ok(Value::float(x + y)),
        (ValueViewRef::Int(x), ValueViewRef::Float(y)) => Ok(Value::float(x as f64 + y)),
        (ValueViewRef::Float(x), ValueViewRef::Int(y)) => Ok(Value::float(x + y as f64)),
        (ValueViewRef::String(x), ValueViewRef::String(y)) => {
            let mut s = String::with_capacity(x.len() + y.len());
            s.push_str(x);
            s.push_str(y);
            Ok(Value::string_owned(s))
        }
        _ => {
            // Non-fixnum numeric operands (bignum now; rational/complex in later
            // phases) fold through the tower.
            if let (Some(x), Some(y)) = (a.as_number(), b.as_number()) {
                return Ok(Value::from_number(x.add(y)));
            }
            let err = SemaError::type_error(
                "number or string",
                format!("{} and {}", a.type_name(), b.type_name()),
            );
            let mixing_string = matches!(a.view_ref(), ValueViewRef::String(_))
                || matches!(b.view_ref(), ValueViewRef::String(_));
            Err(if mixing_string {
                err.with_hint(
                    "+: cannot mix strings with other types; use (str a b ...) to build a string",
                )
            } else {
                err
            })
        }
    }
}

#[inline(always)]
fn vm_sub(a: &Value, b: &Value) -> Result<Value, SemaError> {
    match (a.view_ref(), b.view_ref()) {
        (ValueViewRef::Int(x), ValueViewRef::Int(y)) => match x.checked_sub(y) {
            Some(s) => Ok(Value::int(s)),
            // i64 overflow promotes to a bignum instead of raising.
            None => Ok(Value::from_number(
                SemaNumber::from_i64(x).sub(SemaNumber::from_i64(y)),
            )),
        },
        (ValueViewRef::Float(x), ValueViewRef::Float(y)) => Ok(Value::float(x - y)),
        (ValueViewRef::Int(x), ValueViewRef::Float(y)) => Ok(Value::float(x as f64 - y)),
        (ValueViewRef::Float(x), ValueViewRef::Int(y)) => Ok(Value::float(x - y as f64)),
        _ => match (a.as_number(), b.as_number()) {
            (Some(x), Some(y)) => Ok(Value::from_number(x.sub(y))),
            _ => Err(SemaError::type_error(
                "number",
                format!("{} and {}", a.type_name(), b.type_name()),
            )),
        },
    }
}

#[inline(always)]
fn vm_mul(a: &Value, b: &Value) -> Result<Value, SemaError> {
    match (a.view_ref(), b.view_ref()) {
        (ValueViewRef::Int(x), ValueViewRef::Int(y)) => match x.checked_mul(y) {
            Some(p) => Ok(Value::int(p)),
            // i64 overflow promotes to a bignum instead of raising.
            None => Ok(Value::from_number(
                SemaNumber::from_i64(x).mul(SemaNumber::from_i64(y)),
            )),
        },
        (ValueViewRef::Float(x), ValueViewRef::Float(y)) => Ok(Value::float(x * y)),
        (ValueViewRef::Int(x), ValueViewRef::Float(y)) => Ok(Value::float(x as f64 * y)),
        (ValueViewRef::Float(x), ValueViewRef::Int(y)) => Ok(Value::float(x * y as f64)),
        _ => match (a.as_number(), b.as_number()) {
            (Some(x), Some(y)) => Ok(Value::from_number(x.mul(y))),
            _ => Err(SemaError::type_error(
                "number",
                format!("{} and {}", a.type_name(), b.type_name()),
            )),
        },
    }
}

#[inline(always)]
fn vm_div(a: &Value, b: &Value) -> Result<Value, SemaError> {
    match (a.view_ref(), b.view_ref()) {
        (ValueViewRef::Int(_), ValueViewRef::Int(0)) => Err(SemaError::eval("division by zero")
            .with_hint("/: guard with (if (zero? d) ... (/ n d))")),
        (ValueViewRef::Int(x), ValueViewRef::Int(y)) => {
            if x.checked_rem(y) == Some(0) {
                // checked_rem rules out the i64::MIN / -1 overflow pair (it
                // yields None there), so the quotient always fits a fixnum.
                Ok(Value::int(x / y))
            } else {
                // Not evenly divisible (exact rational, not a lossy float),
                // or i64::MIN / -1: the whole-valued rational normalizes to
                // an integer, promoting the 2^63 quotient to a bignum.
                Ok(Value::from_number(
                    SemaNumber::from_i64(x)
                        .div(SemaNumber::from_i64(y))
                        .unwrap(),
                ))
            }
        }
        (ValueViewRef::Float(x), ValueViewRef::Float(y)) => Ok(Value::float(x / y)),
        (ValueViewRef::Int(x), ValueViewRef::Float(y)) => Ok(Value::float(x as f64 / y)),
        (ValueViewRef::Float(x), ValueViewRef::Int(y)) => Ok(Value::float(x / y as f64)),
        // Any other numeric combination (bignum, rational) divides exactly
        // through the tower; an exact-zero divisor signals, matching the
        // stdlib `/` native fn.
        _ => match (a.as_number(), b.as_number()) {
            (Some(x), Some(y)) => x.div(y).map(Value::from_number).map_err(|_| {
                SemaError::eval("/: division by zero")
                    .with_hint("/: guard with (if (zero? d) ... (/ n d))")
            }),
            _ => Err(SemaError::type_error(
                "number",
                format!("{} and {}", a.type_name(), b.type_name()),
            )),
        },
    }
}

/// Numeric-coercing equality: matches stdlib `=` semantics. Mixed int/float
/// compares exactly (no lossy `as f64` cast that would collapse integers above
/// 2^53 onto the same float).
#[inline(always)]
fn vm_eq(a: &Value, b: &Value) -> bool {
    match (a.view_ref(), b.view_ref()) {
        (ValueViewRef::Int(x), ValueViewRef::Int(y)) => x == y,
        (ValueViewRef::Float(x), ValueViewRef::Float(y)) => x == y,
        (ValueViewRef::Int(x), ValueViewRef::Float(y))
        | (ValueViewRef::Float(y), ValueViewRef::Int(x)) => {
            sema_core::num::cmp_int_float(x, y) == Some(std::cmp::Ordering::Equal)
        }
        _ => match (a.as_number(), b.as_number()) {
            (Some(x), Some(y)) => x.num_eq(&y),
            // Non-numbers fall back to structural equality.
            _ => a == b,
        },
    }
}

fn vm_lt(a: &Value, b: &Value) -> Result<bool, SemaError> {
    match (a.view_ref(), b.view_ref()) {
        (ValueViewRef::Int(x), ValueViewRef::Int(y)) => Ok(x < y),
        (ValueViewRef::Float(x), ValueViewRef::Float(y)) => Ok(x < y),
        (ValueViewRef::Int(x), ValueViewRef::Float(y)) => {
            Ok(sema_core::num::cmp_int_float(x, y) == Some(std::cmp::Ordering::Less))
        }
        (ValueViewRef::Float(x), ValueViewRef::Int(y)) => {
            Ok(sema_core::num::cmp_int_float(y, x) == Some(std::cmp::Ordering::Greater))
        }
        (ValueViewRef::String(x), ValueViewRef::String(y)) => Ok(x < y),
        _ => match (a.as_number(), b.as_number()) {
            (Some(x), Some(y)) if !x.is_real() || !y.is_real() => {
                Err(SemaError::eval("cannot order complex numbers")
                    .with_hint("complex numbers have no ordering; use = or zero? instead"))
            }
            (Some(x), Some(y)) => Ok(x.cmp_real(&y) == Some(std::cmp::Ordering::Less)),
            _ => Err(SemaError::type_error(
                "comparable values",
                format!("{} and {}", a.type_name(), b.type_name()),
            )),
        },
    }
}

/// `mod`/`modulo` intrinsic: floored division (result takes the sign of the
/// divisor) over any exact integer (fixnum or bignum), matching the stdlib
/// `mod` native fn. Float operands keep the existing `%` (IEEE truncated
/// remainder) behavior.
fn vm_mod(a: &Value, b: &Value) -> Result<Value, SemaError> {
    use num_integer::Integer;
    match (a.view_ref(), b.view_ref()) {
        (ValueViewRef::Float(x), ValueViewRef::Float(y)) => Ok(Value::float(x % y)),
        (ValueViewRef::Int(x), ValueViewRef::Float(y)) => Ok(Value::float(x as f64 % y)),
        (ValueViewRef::Float(x), ValueViewRef::Int(y)) => Ok(Value::float(x % y as f64)),
        _ => {
            let n = a
                .as_bigint()
                .ok_or_else(|| SemaError::type_error("integer", a.type_name()))?;
            let d = b
                .as_bigint()
                .ok_or_else(|| SemaError::type_error("integer", b.type_name()))?;
            if d == num_bigint::BigInt::from(0) {
                return Err(SemaError::eval("modulo by zero"));
            }
            Ok(Value::from_bigint(n.mod_floor(&d)))
        }
    }
}

/// Compile Value ASTs with span/source info for debug support (DAP breakpoints).
pub fn compile_program_with_spans(
    vals: &[Value],
    span_map: &sema_core::SpanMap,
    source_file: Option<std::path::PathBuf>,
) -> Result<CompiledProgram, SemaError> {
    compile_program_with_spans_and_natives(vals, span_map, source_file, None)
}

pub fn compile_program_with_spans_and_natives(
    vals: &[Value],
    span_map: &sema_core::SpanMap,
    source_file: Option<std::path::PathBuf>,
    known_natives: Option<std::collections::HashSet<Spur>>,
) -> Result<CompiledProgram, SemaError> {
    let source_file = source_file.map(|p| std::fs::canonicalize(&p).unwrap_or(p));
    let mut cores = Vec::with_capacity(vals.len());
    for val in vals {
        cores.push(crate::lower::lower(val, Some(span_map))?);
    }
    // Lower everything first: a sibling top-level form can redefine a
    // foldable builtin, and the folder must see the whole program (the
    // compiler's redefined_globals scan is likewise program-wide).
    let redefined = crate::optimize::redefined_foldable_names(&cores);
    let mut resolved = Vec::new();
    let mut total_locals: u16 = 0;
    for core in cores {
        let core = crate::optimize::optimize_with_redefined(core, &redefined);
        let (res, n) = crate::resolve::resolve_with_locals(&core)?;
        total_locals = total_locals.max(n);
        resolved.push(res);
    }
    let result = crate::compiler::compile(&resolved, total_locals, known_natives)?;

    let functions: Vec<Rc<Function>> = result
        .functions
        .into_iter()
        .map(|mut f| {
            if f.source_file.is_none() {
                f.source_file = source_file.clone();
            }
            Rc::new(f)
        })
        .collect();
    let main_cache_slots = result.chunk.n_global_cache_slots;
    let closure = Rc::new(Closure {
        func: Rc::new(Function {
            name: None,
            chunk: result.chunk,
            upvalue_descs: Vec::new(),
            upvalue_names: Vec::new(),
            arity: 0,
            has_rest: false,
            param_names: Vec::new().into(),
            local_names: Vec::new(),
            source_file,
            local_scopes: Vec::new(),
            cache_offset: 0,
        }),
        upvalues: Vec::new(),
        // Top-level main closure: uses the VM's own globals and function table.
        globals: None,
        functions: None,
    });

    Ok(CompiledProgram {
        closure,
        functions,
        native_table: result.native_table,
        main_cache_slots,
    })
}

/// Extract the set of source lines that have bytecode spans (valid breakpoint locations).
/// Includes spans from the main chunk and all sub-functions.
pub fn valid_breakpoint_lines(closure: &Closure, functions: &[Rc<Function>]) -> Vec<u32> {
    let mut lines = std::collections::BTreeSet::new();
    for file_lines in valid_breakpoint_lines_by_file(closure, functions).values() {
        lines.extend(file_lines.iter().copied());
    }
    for (_, s) in &closure.func.chunk.spans {
        lines.insert(s.line as u32);
    }
    for f in functions {
        for (_, s) in &f.chunk.spans {
            lines.insert(s.line as u32);
        }
    }
    lines.into_iter().collect()
}

/// Extract executable source lines grouped by canonical source file.
pub fn valid_breakpoint_lines_by_file(
    closure: &Closure,
    functions: &[Rc<Function>],
) -> BTreeMap<std::path::PathBuf, Vec<u32>> {
    let mut lines_by_file: BTreeMap<std::path::PathBuf, std::collections::BTreeSet<u32>> =
        BTreeMap::new();
    collect_function_breakpoint_lines(&closure.func, &mut lines_by_file);
    for function in functions {
        collect_function_breakpoint_lines(function, &mut lines_by_file);
    }
    lines_by_file
        .into_iter()
        .map(|(file, lines)| (file, lines.into_iter().collect()))
        .collect()
}

fn collect_function_breakpoint_lines(
    function: &Function,
    lines_by_file: &mut BTreeMap<std::path::PathBuf, std::collections::BTreeSet<u32>>,
) {
    let Some(source_file) = &function.source_file else {
        return;
    };
    let lines = lines_by_file.entry(source_file.clone()).or_default();
    for (_, span) in &function.chunk.spans {
        lines.insert(span.line as u32);
    }
}

/// Snap a requested breakpoint line to the nearest valid line with bytecode spans.
/// Prefers the same line, then searches forward, then backward.
/// Returns None if no valid lines exist.
pub fn snap_breakpoint_line(requested: u32, valid_lines: &[u32]) -> Option<u32> {
    if valid_lines.is_empty() {
        return None;
    }
    if valid_lines.contains(&requested) {
        return Some(requested);
    }
    // Binary search for insertion point
    let idx = valid_lines.partition_point(|&l| l < requested);
    let forward = valid_lines.get(idx).copied();
    let backward = if idx > 0 {
        valid_lines.get(idx - 1).copied()
    } else {
        None
    };
    match (forward, backward) {
        (Some(f), Some(b)) => {
            if (f - requested) <= (requested - b) {
                Some(f)
            } else {
                Some(b)
            }
        }
        (Some(f), None) => Some(f),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

/// Result of compiling a program, ready for VM execution.
#[derive(Debug)]
pub struct CompiledProgram {
    pub closure: Rc<Closure>,
    pub functions: Vec<Rc<Function>>,
    pub native_table: Vec<Spur>,
    /// Number of inline cache slots used by the main (top-level) chunk.
    pub main_cache_slots: u16,
}

/// Compile a sequence of Value ASTs through the full pipeline.
/// If `known_natives` is provided, global calls to those names emit CallNative
/// for direct dispatch without env lookup at runtime.
pub fn compile_program(
    vals: &[Value],
    known_natives: Option<std::collections::HashSet<Spur>>,
) -> Result<CompiledProgram, SemaError> {
    let mut cores = Vec::with_capacity(vals.len());
    for val in vals {
        cores.push(crate::lower::lower(val, None)?);
    }
    // Sibling top-level redefinitions of foldable builtins suppress folding
    // program-wide (see compile_program_with_spans_and_natives).
    let redefined = crate::optimize::redefined_foldable_names(&cores);
    let mut resolved = Vec::new();
    let mut total_locals: u16 = 0;
    for core in cores {
        let core = crate::optimize::optimize_with_redefined(core, &redefined);
        let (res, n) = crate::resolve::resolve_with_locals(&core)?;
        total_locals = total_locals.max(n);
        resolved.push(res);
    }
    let result = crate::compiler::compile(&resolved, total_locals, known_natives)?;

    let functions: Vec<Rc<Function>> = result.functions.into_iter().map(Rc::new).collect();
    let main_cache_slots = result.chunk.n_global_cache_slots;
    let closure = Rc::new(Closure {
        func: Rc::new(Function {
            name: None,
            chunk: result.chunk,
            upvalue_descs: Vec::new(),
            upvalue_names: Vec::new(),
            arity: 0,
            has_rest: false,
            param_names: Vec::new().into(),
            local_names: Vec::new(),
            source_file: None,
            local_scopes: Vec::new(),
            cache_offset: 0,
        }),
        upvalues: Vec::new(),
        // Top-level main closure: uses the VM's own globals and function table.
        globals: None,
        functions: None,
    });

    Ok(CompiledProgram {
        closure,
        functions,
        native_table: result.native_table,
        main_cache_slots,
    })
}

/// Wrap a [`compile_program`] result as a zero-arg callable `Value`,
/// concretizing its home globals as `home` and pre-resolving its native
/// table against it.
///
/// `compile_program`'s main closure carries `globals: None` / `functions:
/// None` ("run me on whichever VM owns me") because it is normally driven
/// directly by `VM::execute` on a fresh, throwaway VM. Nested `eval` of an
/// async form (Step G) instead needs the eval'd program to run as an
/// ordinary callee under the runtime's cooperative `NativeOutcome::Call`
/// ABI — `invoke_vm_callback_loop` retargets a scratch VM at a callee
/// closure's `globals`/`functions`/native table, so those fields must be
/// concrete `Some(..)`, exactly like a closure produced by `MakeClosure`
/// (mirrors that construction in `VM::make_closure`).
///
/// Nested function cache offsets are assigned here the same way `VM::new`
/// assigns them for a freshly loaded program (starting after the main
/// chunk's own `main_cache_slots`): `compile_program`'s `functions` come
/// straight from the compiler with default (colliding) offsets, since
/// offset assignment normally only happens once, in `VM::new`. Skipping this
/// would alias the eval'd program's inline-cache slots with a nested
/// closure's, corrupting global lookups inside it.
pub fn program_as_callable(prog: CompiledProgram, home: Rc<Env>) -> Result<Value, SemaError> {
    let native_fns = Rc::new(VM::resolve_native_table(&home, &prog.native_table)?);
    let mut functions = prog.functions;
    let mut total_cache_slots = prog.main_cache_slots as usize;
    for func_rc in &mut functions {
        let func = Rc::make_mut(func_rc);
        func.cache_offset = total_cache_slots;
        total_cache_slots += func.chunk.n_global_cache_slots as usize;
    }
    let functions: Rc<Vec<Rc<Function>>> = Rc::new(functions);
    let closure = Rc::new(Closure {
        func: prog.closure.func.clone(),
        upvalues: Vec::new(),
        globals: Some(home),
        functions: Some(functions.clone()),
    });
    let payload = Rc::new(VmClosurePayload {
        closure: closure.clone(),
        functions: functions.clone(),
        native_fns,
    });
    let payload_for_box = Rc::clone(&payload);
    ensure_cycle_gc_wired();
    let mut native_fn = sema_core::NativeFn::with_payload(
        "<eval-program>",
        payload as Rc<dyn std::any::Any>,
        move |ctx, args| {
            ensure_legacy_vm_entry_allowed(ctx)?;
            let closure = &payload_for_box.closure;
            let functions = &payload_for_box.functions;
            let globals = closure
                .globals
                .as_ref()
                .expect("program_as_callable closures always carry Some(home)");
            close_closure_upvalues_for_foreign_run(closure);
            let mut vm = VM::new_with_rc_functions(
                globals.clone(),
                functions.clone(),
                payload_for_box.native_fns.clone(),
            );
            vm.setup_for_call(closure.clone(), args)?;
            vm.run(ctx)
        },
    );
    // Zero-arg, no upvalues: not a cycle candidate for the same reason a
    // zero-upvalue `MakeClosure` result is skipped (see `VM::make_closure`'s
    // "Zero-upvalue exemption" comment) — it owns no upvalue cells, so it
    // cannot sit on a severable cycle edge; its home env is independently a
    // registered candidate (or reachable from one) already.
    native_fn.is_closure = true;
    native_fn.param_names = Some(Rc::from(Vec::new()));
    Ok(Value::native_fn_from_rc(Rc::new(native_fn)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::{Chunk, Function};
    use sema_core::runtime::{
        NativeCall, NativeSuspend, TaskContextHandle, TaskLocalValue, WaitKind,
    };
    use sema_core::{intern, MultiMethod, NativeFn};
    use std::any::Any;
    use std::cell::Cell;
    use std::time::Duration;

    struct DebugIdentityContinuation;

    impl Trace for DebugIdentityContinuation {
        fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
            true
        }
    }

    impl NativeContinuation for DebugIdentityContinuation {
        fn resume(
            self: Box<Self>,
            _context: &mut NativeCallContext<'_>,
            input: ResumeInput,
        ) -> NativeResult {
            match input {
                ResumeInput::Returned(value) => Ok(NativeOutcome::Return(value)),
                ResumeInput::Failed(error) => Err(error),
                ResumeInput::Cancelled(reason) => Err(SemaError::eval(format!(
                    "debug identity continuation was cancelled ({reason:?})"
                ))),
                ResumeInput::Runtime(_) => Err(SemaError::eval(
                    "debug identity continuation received an unexpected runtime response",
                )),
            }
        }
    }

    struct DebugHandlerPayload {
        handler: RefCell<Value>,
    }

    fn debug_handler_payload_tracer(
        payload: &Rc<dyn Any>,
        sink: &mut dyn FnMut(GcEdge<'_>),
    ) -> bool {
        sink(GcEdge::Opaque {
            ptr: NodePtr::of_rc(payload),
            strong_count: Rc::strong_count(payload),
            trace: trace_debug_handler_payload,
            sever: sever_debug_handler_payload,
        });
        true
    }

    fn trace_debug_handler_payload(ptr: NodePtr, sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        // SAFETY: the NativeFn Value retained by the debugger work item owns
        // this payload allocation for the complete opaque traversal.
        let payload = unsafe { &*(ptr.raw() as *const DebugHandlerPayload) };
        let Ok(handler) = payload.handler.try_borrow() else {
            return false;
        };
        sink(GcEdge::Value(&handler));
        true
    }

    fn sever_debug_handler_payload(ptr: NodePtr) -> Option<Value> {
        // SAFETY: see `trace_debug_handler_payload`; this hook is present only
        // to satisfy the collector edge contract and is not called by the
        // debugger walker.
        let payload = unsafe { &*(ptr.raw() as *const DebugHandlerPayload) };
        payload
            .handler
            .try_borrow_mut()
            .ok()
            .map(|mut handler| std::mem::replace(&mut *handler, Value::nil()))
    }

    fn invoke_debug_handler(
        payload: &DebugHandlerPayload,
        _context: &mut NativeCallContext<'_>,
        args: &[Value],
    ) -> NativeResult {
        Ok(NativeOutcome::Call(NativeCall {
            callable: payload.handler.borrow().clone(),
            args: args.to_vec(),
            continuation: Box::new(DebugIdentityContinuation),
        }))
    }

    fn debug_handler_value(handler: Value) -> Value {
        sema_core::register_payload_tracer(
            std::any::TypeId::of::<DebugHandlerPayload>(),
            debug_handler_payload_tracer,
        );
        Value::native_fn(NativeFn::with_payload_result(
            "debug-handler",
            Rc::new(DebugHandlerPayload {
                handler: RefCell::new(handler),
            }),
            invoke_debug_handler,
        ))
    }

    fn debug_macro_probe(
        _context: &NativeCallContext<'_>,
        expression: &Value,
        env: &Env,
    ) -> Result<Value, SemaError> {
        let Some(items) = expression.as_list() else {
            return Ok(expression.clone());
        };
        let Some(head) = items.first().and_then(Value::as_symbol_spur) else {
            return Ok(expression.clone());
        };
        match sema_core::resolve(head).as_str() {
            "debug-expand-value" => Ok(Value::int(42)),
            "debug-expand-fail" => {
                mutate_debug_macro_owner_cell(env)?;
                Err(SemaError::eval("debug macro expansion failed"))
            }
            "debug-expand-compile-fail" => {
                mutate_debug_macro_owner_cell(env)?;
                sema_reader::read("(if)")
            }
            _ => Ok(expression.clone()),
        }
    }

    fn mutate_debug_macro_owner_cell(env: &Env) -> Result<(), SemaError> {
        let mutator = env
            .get(intern("mutate-captured"))
            .ok_or_else(|| SemaError::eval("debug macro mutator is missing"))?;
        let (closure, _, _) = extract_vm_closure(&mutator)
            .ok_or_else(|| SemaError::eval("debug macro mutator is not a VM closure"))?;
        let cell = closure
            .upvalues
            .first()
            .ok_or_else(|| SemaError::eval("debug macro mutator has no upvalue"))?;
        let mut state = cell.state.borrow_mut();
        let UpvalueState::Tracked { value, .. } = &mut *state else {
            return Err(SemaError::eval(
                "debug macro expansion ran before paused-owner snapshot",
            ));
        };
        *value = Value::int(99);
        Ok(())
    }

    struct DebugContextMarker(i64);

    impl Trace for DebugContextMarker {
        fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
            true
        }
    }

    impl TaskLocalValue for DebugContextMarker {
        fn inherit(&self) -> Rc<dyn TaskLocalValue> {
            Rc::new(Self(self.0))
        }

        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    #[test]
    fn native_table_is_traced_as_one_shared_opaque_node() {
        let globals = make_test_env();
        let native = Rc::new(NativeFn::simple("gc-probe", |_| Ok(Value::nil())));
        let native_table = Rc::new(vec![native]);
        let vm = VM::new_for_task_with_native_fns(
            globals,
            Rc::new(Vec::new()),
            Rc::clone(&native_table),
        );
        let table_ptr = sema_core::NodePtr::of_rc(&native_table);
        let mut table_edges = 0;

        sema_core::runtime::Trace::trace(&vm, &mut |edge| {
            if matches!(edge, sema_core::GcEdge::Opaque { ptr, .. } if ptr == table_ptr) {
                table_edges += 1;
            }
        });

        assert_eq!(table_edges, 1, "VM must own one edge to the shared table");
    }

    #[test]
    fn forced_collection_preserves_native_table_shared_by_suspended_vm_and_payloads() {
        let globals = make_test_env();
        let ctx = EvalContext::new();
        let callbacks = eval_str(
            "((lambda (captured) (list (lambda () captured) (lambda () captured))) 7)",
            &globals,
            &ctx,
        )
        .unwrap();
        let callbacks = callbacks.as_list().expect("callback list");
        let (first, second) = (&callbacks[0], &callbacks[1]);
        let (first_closure, functions, native_table) =
            extract_vm_closure(first).expect("first VM closure");
        let (_, _, second_native_table) = extract_vm_closure(second).expect("second VM closure");
        assert!(Rc::ptr_eq(&native_table, &second_native_table));

        let mut suspended = VM::new_for_task_with_native_fns(
            Rc::clone(first_closure.globals.as_ref().expect("closure home")),
            functions,
            Rc::clone(&native_table),
        );
        suspended.setup_for_call(first_closure, &[]).unwrap();
        let pins = sema_core::gc_env_chain_pins(&globals);
        sema_core::gc_collect(&pins, sema_core::GcTrigger::Explicit);

        assert_eq!(
            (second.as_native_fn_ref().unwrap().func)(&ctx, &[]).unwrap(),
            Value::int(7)
        );
        assert!(Rc::strong_count(&native_table) >= 3);
        assert_eq!(suspended.frame_count(), 1);
    }

    #[test]
    fn closure_fallback_rejects_during_runtime_quantum_before_running_bytecode() {
        let globals = make_test_env();
        let calls = Rc::new(std::cell::Cell::new(0));
        let calls_for_native = Rc::clone(&calls);
        globals.set(
            intern("quantum-probe"),
            Value::native_fn(NativeFn::simple("quantum-probe", move |_| {
                calls_for_native.set(calls_for_native.get() + 1);
                Ok(Value::int(9))
            })),
        );
        let ctx = EvalContext::new();
        let callback = eval_str("(lambda () (quantum-probe))", &globals, &ctx).unwrap();
        let native = callback.as_native_fn_ref().expect("VM closure wrapper");
        let _quantum = ctx.enter_runtime_quantum().unwrap();

        let error = (native.func)(&ctx, &[])
            .expect_err("legacy closure fallback must not re-enter during a runtime quantum");

        assert_eq!(
            calls.get(),
            0,
            "rejection must happen before callback bytecode executes"
        );
        assert!(error
            .to_string()
            .contains("legacy native callback cannot re-enter a VM"));
        assert!(
            ctx.runtime_quantum_active(),
            "rejection must leave the active quantum installed"
        );
    }

    #[test]
    fn owned_closure_fallback_rejects_before_moving_arguments() {
        let globals = make_test_env();
        let ctx = EvalContext::new();
        let callback = eval_str("(lambda (value) value)", &globals, &ctx).unwrap();
        let mut args = [Value::int(42)];
        let _quantum = ctx.enter_runtime_quantum().unwrap();

        let error = call_closure_owned(&callback, &ctx, &mut args)
            .expect("value is a VM closure")
            .expect_err("owned legacy fallback must reject active runtime re-entry");

        assert!(error
            .to_string()
            .contains("legacy native callback cannot re-enter a VM"));
        assert_eq!(
            args[0],
            Value::int(42),
            "rejected owned calls must leave the caller's buffer intact"
        );
    }

    #[test]
    fn explicit_escape_owner_does_not_snapshot_a_colliding_interpreter() {
        let (mut first_owner, first_closure, first_cell) = open_upvalue_fixture(Value::int(11));
        let (second_owner, second_closure, second_cell) = open_upvalue_fixture(Value::int(22));
        let first = vm_closure_value(&first_owner, first_closure);
        let second = vm_closure_value(&second_owner, second_closure);
        let graph = Value::list(vec![first, second]);

        snapshot_escaping_call_with_owner(&mut first_owner, &graph, &[]);

        assert!(matches!(
            &*first_cell.state.borrow(),
            UpvalueState::Tracked { value, .. } if value == &Value::int(11)
        ));
        assert!(
            matches!(&*second_cell.state.borrow(), UpvalueState::Open { .. }),
            "an explicit owner must not inspect a colliding interpreter"
        );
    }

    #[test]
    fn legacy_native_dispatch_snapshots_all_callback_arguments() {
        let globals = make_test_env();
        globals.set(
            intern("callback-is-tracked?"),
            Value::native_fn(NativeFn::simple("callback-is-tracked?", |args| {
                let (closure, _, _) =
                    extract_vm_closure(&args[0]).expect("probe argument must be a VM closure");
                let is_tracked = matches!(
                    &*closure.upvalues[0].state.borrow(),
                    UpvalueState::Tracked { .. }
                );
                Ok(Value::bool(is_tracked))
            })),
        );
        let ctx = EvalContext::new();

        let value = eval_str(
            "(let ((captured 7)) (callback-is-tracked? (fn () captured)))",
            &globals,
            &ctx,
        )
        .expect("probe executes");

        assert_eq!(
            value,
            Value::bool(true),
            "legacy value-ABI calls must snapshot every reachable callback before entry"
        );
    }

    #[test]
    fn nested_legacy_callback_rejects_during_runtime_quantum() {
        let globals = make_test_env();
        let ctx = EvalContext::new();
        globals.set(
            intern("invoke-callback"),
            Value::native_fn(NativeFn::with_ctx("invoke-callback", |ctx, args| {
                let native = args[0].as_native_fn_ref().expect("VM closure wrapper");
                (native.func)(ctx, &[])
            })),
        );
        let forms = sema_reader::read_many(
            r#"
            (define (loop n)
              (if (= n 0) n (loop (- n 1))))
            (invoke-callback (lambda () (loop 100)))
            "#,
        )
        .unwrap();
        let program = compile_program(&forms, None).unwrap();
        let mut vm = VM::new(
            globals,
            program.functions,
            &program.native_table,
            program.main_cache_slots,
        )
        .unwrap();
        vm.setup_for_call(program.closure, &[]).unwrap();
        let _quantum = ctx.enter_runtime_quantum().unwrap();

        let quantum = vm.run_quantum(&ctx, 100, CancellationView::default());
        let error = quantum
            .outcome
            .expect_err("legacy callback must not start a nested VM dispatch loop");

        assert!(error
            .to_string()
            .contains("legacy native callback cannot re-enter a VM"));
    }

    #[test]
    fn runtime_invalid_call_reports_not_callable_without_callback_fallback() {
        let globals = make_test_env();
        let ctx = EvalContext::new();
        let forms = sema_reader::read_many("(42)").unwrap();
        let program = compile_program(&forms, None).unwrap();
        let mut vm = VM::new(
            globals,
            program.functions,
            &program.native_table,
            program.main_cache_slots,
        )
        .unwrap();
        vm.setup_for_call(program.closure, &[]).unwrap();
        let _quantum = ctx.enter_runtime_quantum().unwrap();

        let error = vm
            .run_quantum(&ctx, 100, CancellationView::default())
            .outcome
            .expect_err("a raw value is not callable");

        assert!(
            error.to_string().contains("not callable: 42 (int)"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn legacy_vm_bridge_symbols_are_absent_from_sources() {
        let source = [include_str!("vm.rs"), include_str!("lib.rs")].concat();
        let source_without_line_comments = source
            .lines()
            .map(|line| line.split("//").next().unwrap_or_default())
            .collect::<Vec<_>>()
            .join("\n");
        let forbidden = [
            ["CURRENT", "_VM"].concat(),
            ["Current", "VmGuard"].concat(),
            ["try_run_on", "_current_vm"].concat(),
            ["run_nested", "_closure_args"].concat(),
            ["current_vm", "_globals"].concat(),
            ["suspend_runtime", "_quantum"].concat(),
            ["Quantum", "SuspendGuard"].concat(),
            ["snapshot_escaping", "_closure"].concat(),
            ["snapshot_escaping", "_value"].concat(),
            ["snapshot_native_escaping_args", "_for_current_vm"].concat(),
        ];

        for symbol in forbidden {
            assert!(
                !source_without_line_comments.contains(&symbol),
                "legacy bridge symbol remains in production source: {symbol}"
            );
        }
    }

    /// Convenience: compile and run a string expression in the VM.
    fn eval_str(input: &str, globals: &Rc<Env>, ctx: &EvalContext) -> Result<Value, SemaError> {
        let vals = sema_reader::read_many(input)
            .map_err(|e| SemaError::eval(format!("parse error: {e}")))?;
        let prog = compile_program(&vals, None)?;
        let mut vm = VM::new(globals.clone(), prog.functions, &[], prog.main_cache_slots)?;
        vm.execute(prog.closure, ctx)
    }

    fn make_test_env() -> Rc<Env> {
        let env = Rc::new(Env::new());
        env.set(
            intern("+"),
            Value::native_fn(NativeFn::simple("+", |args| vm_add(&args[0], &args[1]))),
        );
        env.set(
            intern("-"),
            Value::native_fn(NativeFn::simple("-", |args| vm_sub(&args[0], &args[1]))),
        );
        env.set(
            intern("*"),
            Value::native_fn(NativeFn::simple("*", |args| vm_mul(&args[0], &args[1]))),
        );
        env.set(
            intern("/"),
            Value::native_fn(NativeFn::simple("/", |args| vm_div(&args[0], &args[1]))),
        );
        env.set(
            intern("="),
            Value::native_fn(NativeFn::simple("=", |args| {
                Ok(Value::bool(vm_eq(&args[0], &args[1])))
            })),
        );
        env.set(
            intern("<"),
            Value::native_fn(NativeFn::simple("<", |args| {
                Ok(Value::bool(vm_lt(&args[0], &args[1])?))
            })),
        );
        env.set(
            intern(">"),
            Value::native_fn(NativeFn::simple(">", |args| {
                Ok(Value::bool(vm_lt(&args[1], &args[0])?))
            })),
        );
        env.set(
            intern("not"),
            Value::native_fn(NativeFn::simple("not", |args| {
                Ok(Value::bool(!args[0].is_truthy()))
            })),
        );
        env.set(
            intern("list"),
            Value::native_fn(NativeFn::simple("list", |args| {
                Ok(Value::list(args.to_vec()))
            })),
        );
        env
    }

    fn open_upvalue_fixture(value: Value) -> (VM, Rc<Closure>, Rc<UpvalueCell>) {
        let globals = make_test_env();
        let forms = sema_reader::read_many("nil").expect("fixture source parses");
        let program = compile_program(&forms, None).expect("fixture source compiles");
        let mut owner = VM::new(
            globals.clone(),
            program.functions,
            &program.native_table,
            program.main_cache_slots,
        )
        .expect("fixture VM constructs");
        let cell = Rc::new(UpvalueCell::new_open(0, 0));
        owner.stack.push(value);
        owner.frames.push(CallFrame {
            closure: program.closure.clone(),
            pc: 0,
            base: 0,
            open_upvalues: Some(vec![Some(cell.clone())]),
            cache_base: 0,
        });
        let closure = Rc::new(Closure {
            func: program.closure.func.clone(),
            upvalues: vec![cell.clone()],
            globals: Some(globals),
            functions: Some(owner.functions.clone()),
        });
        (owner, closure, cell)
    }

    fn paused_debug_owner_with_open_mutator(value: Value) -> (VM, Rc<UpvalueCell>) {
        let globals = make_test_env();
        let ctx = EvalContext::new();
        let template = eval_str(
            "(let ((captured 0))\
               (lambda (value) (set! captured value) captured))",
            &globals,
            &ctx,
        )
        .expect("mutator template evaluates");
        let (template_closure, functions, native_fns) =
            extract_vm_closure(&template).expect("mutator template is a VM closure");
        assert_eq!(template_closure.upvalues.len(), 1);

        let forms = sema_reader::read_many("nil").expect("owner source parses");
        let program = compile_program(&forms, None).expect("owner source compiles");
        let mut owner_func = (*program.closure.func).clone();
        owner_func.chunk.n_locals = 1;
        owner_func.local_names = vec![(0, intern("captured"))];
        let owner_closure = Rc::new(Closure {
            func: Rc::new(owner_func),
            upvalues: Vec::new(),
            globals: None,
            functions: None,
        });
        let mut owner = VM::new(
            Rc::clone(&globals),
            program.functions,
            &program.native_table,
            program.main_cache_slots,
        )
        .expect("owner VM constructs");
        owner.seed_main_frame(owner_closure);
        owner.stack[0] = value;
        let cell = Rc::new(UpvalueCell::new_open(0, 0));
        owner.frames[0].open_upvalues = Some(vec![Some(Rc::clone(&cell))]);

        let closure = Rc::new(Closure {
            func: Rc::clone(&template_closure.func),
            upvalues: vec![Rc::clone(&cell)],
            globals: Some(Rc::clone(&globals)),
            functions: Some(Rc::clone(&functions)),
        });
        let payload = Rc::new(VmClosurePayload {
            closure,
            functions,
            native_fns,
        });
        let mut native = NativeFn::with_payload(
            "mutate-captured",
            payload as Rc<dyn std::any::Any>,
            |_, _| Ok(Value::nil()),
        );
        native.is_closure = true;
        globals.set(intern("mutate-captured"), Value::native_fn(native));

        (owner, cell)
    }

    fn paused_debug_owner_with_closed_upvalue_mutator(value: Value) -> (VM, Rc<UpvalueCell>) {
        let (mut owner, cell) = paused_debug_owner_with_open_mutator(value.clone());
        *cell.state.borrow_mut() = UpvalueState::Closed(value);
        let mut owner_func = (*owner.frames[0].closure.func).clone();
        owner_func.upvalue_names = vec![intern("captured-upvalue")];
        owner.frames[0].closure = Rc::new(Closure {
            func: Rc::new(owner_func),
            upvalues: vec![Rc::clone(&cell)],
            globals: None,
            functions: None,
        });
        owner.frames[0].open_upvalues = None;
        (owner, cell)
    }

    fn vm_closure_value(owner: &VM, closure: Rc<Closure>) -> Value {
        let payload = Rc::new(VmClosurePayload {
            closure,
            functions: owner.functions.clone(),
            native_fns: owner.native_fns.clone(),
        });
        let mut native = NativeFn::with_payload(
            "<escaping-owner-fixture>",
            payload as Rc<dyn std::any::Any>,
            |_, _| Ok(Value::nil()),
        );
        native.is_closure = true;
        Value::native_fn(native)
    }

    fn eval(input: &str) -> Result<Value, SemaError> {
        let globals = make_test_env();
        let ctx = EvalContext::new();
        eval_str(input, &globals, &ctx)
    }

    #[test]
    fn closure_home_globals_resolve_against_defining_env() {
        // M1: a closure carries its home globals env. When run on a VM whose
        // own globals differ, GetGlobal must resolve against the closure's home
        // env (the env it was *defined* in), not the executing VM's globals.
        // This is the keystone enabler for module-isolated `import` on the VM.
        let ctx = EvalContext::new();

        // A trivial program whose body just loads the global `x`.
        let vals = sema_reader::read_many("x").unwrap();
        let prog = compile_program(&vals, None).unwrap();

        // Home env G1 defines x = 999; the executing VM's own env G2 does NOT.
        let g1 = Rc::new(Env::new());
        g1.set(intern("x"), Value::int(999));
        let g2 = Rc::new(Env::new()); // no `x`

        // Closure whose home globals = G1, executed by a VM whose globals = G2.
        let closure = Rc::new(Closure {
            func: prog.closure.func.clone(),
            upvalues: vec![],
            globals: Some(g1.clone()),
            functions: None,
        });
        let mut vm = VM::new(
            g2.clone(),
            prog.functions.clone(),
            &[],
            prog.main_cache_slots,
        )
        .unwrap();
        let result = vm.execute(closure, &ctx).unwrap();
        assert_eq!(
            result,
            Value::int(999),
            "GetGlobal must resolve `x` against the closure's home env G1, not the VM's G2"
        );

        // Negative control: with no home globals (`None`), the same func
        // resolves against the executing VM's own env G2, where x is unbound.
        let closure_no_home = Rc::new(Closure {
            func: prog.closure.func.clone(),
            upvalues: vec![],
            globals: None,
            functions: None,
        });
        let mut vm2 = VM::new(g2, prog.functions, &[], prog.main_cache_slots).unwrap();
        let err = vm2
            .execute(closure_no_home, &ctx)
            .expect_err("x must be unbound against the VM's own globals (G2)");
        let msg = err.to_string().to_lowercase();
        assert!(
            msg.contains("unbound") && msg.contains('x'),
            "expected an unbound-`x` error against G2, got: {err}"
        );
    }

    #[test]
    fn dup_on_empty_stack_errors_instead_of_ub() {
        // A crafted/corrupt .semac can declare a generous `max_stack` but lead
        // with a bare DUP. Before the guard this read `stack[usize::MAX]` (UB);
        // now it must return a clean error.
        let mut chunk = Chunk::new();
        chunk.code = vec![op::DUP, op::RETURN];
        chunk.max_stack = 8;
        let func = Rc::new(Function {
            name: None,
            chunk,
            upvalue_descs: vec![],
            upvalue_names: vec![],
            arity: 0,
            has_rest: false,
            param_names: Vec::new().into(),
            local_names: vec![],
            source_file: None,
            local_scopes: Vec::new(),
            cache_offset: 0,
        });
        let closure = Rc::new(Closure {
            func: func.clone(),
            upvalues: vec![],
            globals: None,
            functions: None,
        });
        let globals = make_test_env();
        let ctx = EvalContext::new();
        let mut vm = VM::new(globals, vec![func], &[], 0).unwrap();
        let res = vm.execute(closure, &ctx);
        let err = res.expect_err("DUP on empty stack must error, not panic/UB");
        assert!(
            err.to_string().contains("DUP on empty stack"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn self_tail_call_opcode_runs_a_countdown_loop() {
        // Hand-assemble a self-recursive countdown that loops via SELF_TAIL_CALL
        // (no callee on the stack — the VM reuses the current frame's closure):
        //   (fn loop (n) (if (= n 0) n (loop (- n 1))))
        use crate::emit::Emitter;
        let mut e = Emitter::new();
        e.emit_op(Op::LoadLocal0); // n
        e.emit_const(Value::int(0)).unwrap();
        e.emit_op(Op::Eq); // n == 0
        let jf = e.emit_jump(Op::JumpIfFalse);
        e.emit_op(Op::LoadLocal0); // n (== 0)
        e.emit_op(Op::Return);
        e.patch_jump(jf); // recursive branch
        e.emit_op(Op::LoadLocal0); // n
        e.emit_const(Value::int(1)).unwrap();
        e.emit_op(Op::Sub); // n - 1
        e.emit_op(Op::SelfTailCall);
        e.emit_u16(1); // argc = 1
        let mut chunk = e.into_chunk();
        chunk.max_stack = 8;
        chunk.n_locals = 1;
        let func = Rc::new(Function {
            name: Some(intern("loop")),
            chunk,
            upvalue_descs: vec![],
            upvalue_names: vec![],
            arity: 1,
            has_rest: false,
            param_names: Vec::new().into(),
            local_names: vec![],
            source_file: None,
            local_scopes: Vec::new(),
            cache_offset: 0,
        });
        let closure = Rc::new(Closure {
            func: func.clone(),
            upvalues: vec![],
            globals: None,
            functions: None,
        });
        let globals = make_test_env();
        let ctx = EvalContext::new();
        let mut vm = VM::new(globals, vec![func], &[], 0).unwrap();
        vm.setup_for_call(closure, &[Value::int(5)]).unwrap();
        let result = vm.run(&ctx).unwrap();
        assert_eq!(result, Value::int(0));
    }

    #[test]
    fn quantum_expiry_preserves_vm_state_across_repeated_resumes() {
        let env = make_test_env();
        let ctx = EvalContext::new();
        let forms = sema_reader::read_many("(let loop ((n 100)) (if (= n 0) n (loop (- n 1))))")
            .expect("read");
        let program = compile_program(&forms, None).expect("compile");
        let mut vm = VM::new(
            env,
            program.functions,
            &program.native_table,
            program.main_cache_slots,
        )
        .expect("vm");
        vm.setup_for_call(program.closure, &[]).expect("prepare");

        let mut expiries = 0;
        loop {
            let quantum = vm.run_quantum(&ctx, 7, CancellationView::default());
            match quantum.outcome.expect("quantum") {
                crate::debug::VmExecResult::QuantumExpired { instructions } => {
                    assert_eq!(instructions, 7);
                    assert_eq!(quantum.instructions, 7);
                    expiries += 1;
                }
                crate::debug::VmExecResult::Finished(value) => {
                    assert_eq!(value.as_int(), Some(0));
                    assert!(quantum.instructions <= 7);
                    break;
                }
                other => panic!("unexpected quantum result: {other:?}"),
            }
        }
        assert!(expiries > 1);
    }

    #[test]
    fn test_vm_int_literal() {
        assert_eq!(eval("42").unwrap(), Value::int(42));
    }

    #[test]
    fn test_vm_nil() {
        assert_eq!(eval("nil").unwrap(), Value::nil());
    }

    #[test]
    fn test_vm_bool() {
        assert_eq!(eval("#t").unwrap(), Value::bool(true));
        assert_eq!(eval("#f").unwrap(), Value::bool(false));
    }

    #[test]
    fn test_vm_string() {
        assert_eq!(eval("\"hello\"").unwrap(), Value::string("hello"));
    }

    #[test]
    fn test_vm_if_true() {
        assert_eq!(eval("(if #t 42 99)").unwrap(), Value::int(42));
    }

    #[test]
    fn test_vm_if_false() {
        assert_eq!(eval("(if #f 42 99)").unwrap(), Value::int(99));
    }

    #[test]
    fn test_vm_begin() {
        assert_eq!(eval("(begin 1 2 3)").unwrap(), Value::int(3));
    }

    #[test]
    fn test_vm_define_and_load() {
        let globals = make_test_env();
        let ctx = EvalContext::new();
        eval_str("(define x 42)", &globals, &ctx).unwrap();
        let result = eval_str("x", &globals, &ctx).unwrap();
        assert_eq!(result, Value::int(42));
    }

    #[test]
    fn test_vm_let() {
        assert_eq!(eval("(let ((x 10)) x)").unwrap(), Value::int(10));
    }

    #[test]
    fn test_vm_let_multiple() {
        assert_eq!(eval("(let ((x 10) (y 20)) y)").unwrap(), Value::int(20));
    }

    #[test]
    fn test_vm_nested_if() {
        assert_eq!(eval("(if (if #t #f #t) 1 2)").unwrap(), Value::int(2));
    }

    #[test]
    fn test_vm_lambda_call() {
        assert_eq!(eval("((lambda (x) x) 42)").unwrap(), Value::int(42));
    }

    #[test]
    fn test_vm_lambda_two_args() {
        assert_eq!(eval("((lambda (x y) y) 1 2)").unwrap(), Value::int(2));
    }

    #[test]
    fn test_vm_closure_capture() {
        assert_eq!(
            eval("(let ((x 10)) ((lambda () x)))").unwrap(),
            Value::int(10)
        );
    }

    #[test]
    fn test_vm_list_literal() {
        let result = eval("(list 1 2 3)").unwrap();
        let items = result.as_list().expect("Expected list");
        assert_eq!(items.len(), 3);
        assert_eq!(items[0], Value::int(1));
        assert_eq!(items[1], Value::int(2));
        assert_eq!(items[2], Value::int(3));
    }

    #[test]
    fn test_vm_make_vector() {
        let result = eval("[1 2 3]").unwrap();
        let items = result.as_vector().expect("Expected vector");
        assert_eq!(items.len(), 3);
        assert_eq!(items[0], Value::int(1));
        assert_eq!(items[1], Value::int(2));
        assert_eq!(items[2], Value::int(3));
    }

    #[test]
    fn test_vm_and_short_circuit() {
        assert_eq!(eval("(and #f 42)").unwrap(), Value::bool(false));
        assert_eq!(eval("(and #t 42)").unwrap(), Value::int(42));
    }

    #[test]
    fn test_vm_or_short_circuit() {
        assert_eq!(eval("(or 42 99)").unwrap(), Value::int(42));
        assert_eq!(eval("(or #f 99)").unwrap(), Value::int(99));
    }

    #[test]
    fn test_vm_throw_catch() {
        // Caught value is now a map with :type, :message, :value keys
        let result = eval("(try (throw \"boom\") (catch e (:value e)))").unwrap();
        assert_eq!(result, Value::string("boom"));
    }

    #[test]
    fn test_vm_throw_catch_type() {
        let result = eval("(try (throw \"boom\") (catch e (:type e)))").unwrap();
        assert_eq!(result, Value::keyword("user"));
    }

    #[test]
    fn test_vm_try_no_throw() {
        assert_eq!(eval("(try 42 (catch e 99))").unwrap(), Value::int(42));
    }

    #[test]
    fn test_vm_try_catch_native_error() {
        // Division by zero from NativeFn should be caught
        let result = eval("(try (/ 1 0) (catch e \"caught\"))").unwrap();
        assert_eq!(result, Value::string("caught"));
    }

    #[test]
    fn test_vm_try_catch_native_error_message() {
        // First verify error type to ensure we caught the right kind of error
        let type_result = eval("(try (/ 1 0) (catch e (:type e)))").unwrap();
        assert_eq!(type_result, Value::keyword("eval"));
        // Then verify the message as secondary validation
        let result = eval("(try (/ 1 0) (catch e (:message e)))").unwrap();
        let s = result.as_str().expect("Expected string");
        assert!(s.contains("division by zero"), "got: {s}");
    }

    #[test]
    fn test_vm_try_catch_type_error() {
        let result = eval("(try (+ 1 \"a\") (catch e (:type e)))").unwrap();
        assert_eq!(result, Value::keyword("type-error"));
    }

    #[test]
    fn test_vm_try_catch_from_closure_call() {
        // Regression: throw inside a called VM closure must be caught by try/catch
        let globals = make_test_env();
        let ctx = EvalContext::new();
        eval_str("(define (thrower) (throw \"boom\"))", &globals, &ctx).unwrap();
        let result = eval_str("(try (thrower) (catch e \"caught\"))", &globals, &ctx).unwrap();
        assert_eq!(result, Value::string("caught"));
    }

    #[test]
    fn test_vm_try_catch_from_lambda_call() {
        // Throw from an immediately-called lambda
        let result = eval("(try ((fn () (throw 42))) (catch e (:value e)))").unwrap();
        assert_eq!(result, Value::int(42));
    }

    #[test]
    fn test_vm_try_catch_nested_call() {
        // Throw two calls deep
        let globals = make_test_env();
        let ctx = EvalContext::new();
        eval_str("(define (inner) (throw \"deep\"))", &globals, &ctx).unwrap();
        eval_str("(define (outer) (inner))", &globals, &ctx).unwrap();
        let result = eval_str("(try (outer) (catch e \"caught\"))", &globals, &ctx).unwrap();
        assert_eq!(result, Value::string("caught"));
    }

    #[test]
    fn test_vm_try_catch_in_call_arg() {
        // Regression: try/catch as argument to another function must preserve stack
        let globals = make_test_env();
        let ctx = EvalContext::new();
        eval_str("(define (thrower) (throw \"boom\"))", &globals, &ctx).unwrap();
        // try result used as arg to +
        let result = eval_str("(+ 1 (try (thrower) (catch e 2)))", &globals, &ctx).unwrap();
        assert_eq!(result, Value::int(3));
    }

    #[test]
    fn test_vm_try_catch_in_list_constructor() {
        // try/catch as one of several args — stack must be preserved for all
        let globals = make_test_env();
        let ctx = EvalContext::new();
        eval_str("(define (thrower) (throw \"boom\"))", &globals, &ctx).unwrap();
        let result = eval_str("(list 1 2 (try (thrower) (catch e 3)) 4)", &globals, &ctx).unwrap();
        let items = result.as_list().expect("list");
        assert_eq!(items.len(), 4);
        assert_eq!(items[2], Value::int(3));
    }

    #[test]
    fn test_vm_try_catch_call_not_last() {
        // Call is not the last instruction in the try body
        let globals = make_test_env();
        let ctx = EvalContext::new();
        eval_str("(define (thrower) (throw \"boom\"))", &globals, &ctx).unwrap();
        let result = eval_str(
            "(try (begin (thrower) 123) (catch e \"caught\"))",
            &globals,
            &ctx,
        )
        .unwrap();
        assert_eq!(result, Value::string("caught"));
    }

    #[test]
    fn test_vm_quote() {
        let result = eval("'(a b c)").unwrap();
        let items = result.as_list().expect("Expected list");
        assert_eq!(items.len(), 3);
    }

    #[test]
    fn test_vm_set() {
        let globals = make_test_env();
        let ctx = EvalContext::new();
        eval_str("(define x 1)", &globals, &ctx).unwrap();
        eval_str("(set! x 42)", &globals, &ctx).unwrap();
        let result = eval_str("x", &globals, &ctx).unwrap();
        assert_eq!(result, Value::int(42));
    }

    #[test]
    fn test_vm_recursive_define() {
        let globals = make_test_env();
        let ctx = EvalContext::new();
        eval_str(
            "(define (fact n) (if (= n 0) 1 (* n (fact (- n 1)))))",
            &globals,
            &ctx,
        )
        .unwrap();
        let result = eval_str("(fact 5)", &globals, &ctx).unwrap();
        assert_eq!(result, Value::int(120));
    }

    #[test]
    fn test_vm_do_loop() {
        let result = eval("(do ((i 0 (+ i 1))) ((= i 5) i))").unwrap();
        assert_eq!(result, Value::int(5));
    }

    #[test]
    fn test_vm_named_let() {
        let result =
            eval("(let loop ((n 5) (acc 1)) (if (= n 0) acc (loop (- n 1) (* acc n))))").unwrap();
        assert_eq!(result, Value::int(120));
    }

    #[test]
    fn test_vm_letrec() {
        let globals = make_test_env();
        let ctx = EvalContext::new();
        let result = eval_str(
            "(letrec ((even? (lambda (n) (if (= n 0) #t (odd? (- n 1))))) (odd? (lambda (n) (if (= n 0) #f (even? (- n 1)))))) (even? 4))",
            &globals,
            &ctx,
        ).unwrap();
        assert_eq!(result, Value::bool(true));
    }

    #[test]
    fn test_vm_rest_params() {
        let result = eval("((lambda (x . rest) rest) 1 2 3)").unwrap();
        let items = result.as_list().expect("Expected list");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0], Value::int(2));
        assert_eq!(items[1], Value::int(3));
    }

    // --- Task 8: Mutable upvalue tests ---

    #[test]
    fn test_vm_counter_closure() {
        // make-counter pattern: closure that mutates a captured variable
        let result =
            eval("(let ((n 0)) (let ((inc (lambda () (set! n (+ n 1)) n))) (inc) (inc) (inc)))")
                .unwrap();
        assert_eq!(result, Value::int(3));
    }

    #[test]
    fn test_vm_shared_mutable_upvalue() {
        // Two closures sharing the same mutable upvalue
        let result = eval(
            "(let ((n 0)) (let ((inc (lambda () (set! n (+ n 1)))) (get (lambda () n))) (inc) (inc) (get)))",
        )
        .unwrap();
        assert_eq!(result, Value::int(2));
    }

    #[test]
    fn test_vm_set_local_in_let() {
        // set! on a local variable (not captured)
        let result = eval("(let ((x 1)) (set! x 42) x)").unwrap();
        assert_eq!(result, Value::int(42));
    }

    #[test]
    fn test_vm_closure_captures_after_mutation() {
        // Closure captures value after mutation
        let result = eval("(let ((x 1)) (set! x 10) ((lambda () x)))").unwrap();
        assert_eq!(result, Value::int(10));
    }

    #[test]
    fn test_vm_closure_returns_closure() {
        // A closure that returns another closure
        let result = eval("(let ((f (lambda () (lambda (x) x)))) ((f) 42))").unwrap();
        assert_eq!(result, Value::int(42));
    }

    #[test]
    fn test_vm_make_adder() {
        // Classic make-adder pattern: closure captures upvalue
        let globals = make_test_env();
        let ctx = EvalContext::new();
        eval_str(
            "(define (make-adder n) (lambda (x) (+ n x)))",
            &globals,
            &ctx,
        )
        .unwrap();
        eval_str("(define add5 (make-adder 5))", &globals, &ctx).unwrap();
        let result = eval_str("(add5 3)", &globals, &ctx).unwrap();
        assert_eq!(result, Value::int(8));
    }

    #[test]
    fn test_vm_compose() {
        // compose: closure returns closure that captures two upvalues
        let globals = make_test_env();
        let ctx = EvalContext::new();
        eval_str(
            "(define (compose f g) (lambda (x) (f (g x))))",
            &globals,
            &ctx,
        )
        .unwrap();
        eval_str("(define inc (lambda (x) (+ x 1)))", &globals, &ctx).unwrap();
        eval_str("(define dbl (lambda (x) (* x 2)))", &globals, &ctx).unwrap();
        let result = eval_str("((compose dbl inc) 5)", &globals, &ctx).unwrap();
        assert_eq!(result, Value::int(12));
    }

    #[test]
    fn test_vm_nested_make_closure() {
        // Three levels deep
        let result = eval("((((lambda () (lambda () (lambda () 42))))))").unwrap();
        assert_eq!(result, Value::int(42));
    }

    #[test]
    fn test_vm_named_fn_rest_params() {
        let globals = make_test_env();
        let ctx = EvalContext::new();
        eval_str("(define (f . args) args)", &globals, &ctx).unwrap();
        let result = eval_str("(f 1 2 3)", &globals, &ctx).unwrap();
        let items = result.as_list().expect("Expected list");
        assert_eq!(items.len(), 3);
        assert_eq!(items[0], Value::int(1));
    }

    #[test]
    fn test_vm_named_let_still_works_with_fix() {
        let globals = make_test_env();
        let ctx = EvalContext::new();
        let result = eval_str(
            "(let loop ((n 5) (acc 1)) (if (= n 0) acc (loop (- n 1) (* acc n))))",
            &globals,
            &ctx,
        )
        .unwrap();
        assert_eq!(result, Value::int(120));
    }

    #[test]
    fn test_vm_curry() {
        // Curry pattern
        let globals = make_test_env();
        let ctx = EvalContext::new();
        eval_str(
            "(define (curry f) (lambda (x) (lambda (y) (f x y))))",
            &globals,
            &ctx,
        )
        .unwrap();
        let result = eval_str("(((curry +) 3) 4)", &globals, &ctx).unwrap();
        assert_eq!(result, Value::int(7));
    }

    // --- Regression tests: division and equality semantics ---

    #[test]
    fn test_vm_div_int_returns_rational_when_non_whole() {
        // R7RS: exact/exact division yields an exact rational, not a float.
        let globals = make_test_env();
        let ctx = EvalContext::new();
        let result = eval_str("(/ 3 2)", &globals, &ctx).unwrap();
        assert_eq!(result, eval_str("3/2", &globals, &ctx).unwrap());
    }

    #[test]
    fn test_vm_div_int_returns_int_when_whole() {
        let globals = make_test_env();
        let ctx = EvalContext::new();
        let result = eval_str("(/ 4 2)", &globals, &ctx).unwrap();
        assert_eq!(result, Value::int(2));
    }

    #[test]
    fn test_vm_div_int_negative_non_whole() {
        // R7RS: exact/exact division yields an exact rational (7/3), not a float.
        let globals = make_test_env();
        let ctx = EvalContext::new();
        let result = eval_str("(/ 7 3)", &globals, &ctx).unwrap();
        assert_eq!(result, eval_str("7/3", &globals, &ctx).unwrap());
    }

    #[test]
    fn test_vm_div_by_zero() {
        let globals = make_test_env();
        let ctx = EvalContext::new();
        assert!(eval_str("(/ 1 0)", &globals, &ctx).is_err());
    }

    #[test]
    fn test_vm_eq_int_float_coercion() {
        let globals = make_test_env();
        let ctx = EvalContext::new();
        assert_eq!(
            eval_str("(= 1 1.0)", &globals, &ctx).unwrap(),
            Value::bool(true)
        );
    }

    #[test]
    fn test_vm_eq_float_int_coercion() {
        let globals = make_test_env();
        let ctx = EvalContext::new();
        assert_eq!(
            eval_str("(= 1.0 1)", &globals, &ctx).unwrap(),
            Value::bool(true)
        );
    }

    #[test]
    fn test_vm_eq_int_float_not_equal() {
        let globals = make_test_env();
        let ctx = EvalContext::new();
        assert_eq!(
            eval_str("(= 1 2.0)", &globals, &ctx).unwrap(),
            Value::bool(false)
        );
    }

    #[test]
    fn test_vm_eq_same_type_int() {
        let globals = make_test_env();
        let ctx = EvalContext::new();
        assert_eq!(
            eval_str("(= 1 1)", &globals, &ctx).unwrap(),
            Value::bool(true)
        );
        assert_eq!(
            eval_str("(= 1 2)", &globals, &ctx).unwrap(),
            Value::bool(false)
        );
    }

    #[test]
    fn test_vm_eq_opcode_direct() {
        use crate::emit::Emitter;
        use crate::opcodes::Op;
        let globals = Rc::new(Env::new());
        let ctx = EvalContext::new();
        let mut e = Emitter::new();
        e.emit_const(Value::int(1)).unwrap();
        e.emit_const(Value::float(1.0)).unwrap();
        e.emit_op(Op::Eq);
        e.emit_op(Op::Return);
        let func = Rc::new(crate::chunk::Function {
            name: None,
            chunk: e.into_chunk(),
            upvalue_descs: vec![],
            upvalue_names: vec![],
            arity: 0,
            has_rest: false,
            param_names: Vec::new().into(),
            local_names: vec![],
            source_file: None,
            local_scopes: Vec::new(),
            cache_offset: 0,
        });
        let closure = Rc::new(Closure {
            func,
            upvalues: vec![],
            globals: None,
            functions: None,
        });
        let mut vm = VM::new(globals, vec![], &[], 0).unwrap();
        let result = vm.execute(closure, &ctx).unwrap();
        assert_eq!(
            result,
            Value::bool(true),
            "Op::Eq should coerce int 1 == float 1.0"
        );
    }

    #[test]
    fn call_native_out_of_range_id_errors_not_panics() {
        // A crafted .semac can carry a CALL_NATIVE whose native_id exceeds the
        // resolved native table. The bounds check must return a SemaError, not
        // panic — a debug_assert! alone is compiled out in release builds (DoS).
        use crate::emit::Emitter;
        use crate::opcodes::Op;
        let globals = Rc::new(Env::new());
        let ctx = EvalContext::new();
        let mut e = Emitter::new();
        e.emit_op(Op::CallNative);
        e.emit_u16(99); // native_id far past the (empty) table
        e.emit_u16(0); // argc
        e.emit_op(Op::Return);
        let func = Rc::new(crate::chunk::Function {
            name: None,
            chunk: e.into_chunk(),
            upvalue_descs: vec![],
            upvalue_names: vec![],
            arity: 0,
            has_rest: false,
            param_names: Vec::new().into(),
            local_names: vec![],
            source_file: None,
            local_scopes: Vec::new(),
            cache_offset: 0,
        });
        let closure = Rc::new(Closure {
            func,
            upvalues: vec![],
            globals: None,
            functions: None,
        });
        let mut vm = VM::new(globals, vec![], &[], 0).unwrap();
        let result = vm.execute(closure, &ctx);
        assert!(
            result.is_err(),
            "out-of-range native_id must error, got {result:?}"
        );
    }

    #[test]
    fn test_debug_hook_stops_at_span() {
        use crate::debug::{DebugCommand, DebugEvent, DebugState, StepMode};
        use std::sync::mpsc;

        let globals = make_test_env();
        let ctx = EvalContext::new();

        let input = "(+ 1 2)\n(+ 3 4)";
        let (vals, span_map) = sema_reader::read_many_with_spans(input).unwrap();
        let prog = compile_program_with_spans(&vals, &span_map, None).unwrap();

        let (event_tx, event_rx) = mpsc::channel();
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let mut debug_state = DebugState::new(event_tx, cmd_rx);
        debug_state.step_mode = StepMode::StepInto;

        // Send Continue commands so the VM doesn't block forever
        cmd_tx.send(DebugCommand::Continue).unwrap();
        cmd_tx.send(DebugCommand::Continue).unwrap();

        let mut vm = VM::new(globals, prog.functions, &[], prog.main_cache_slots).unwrap();
        let result = vm
            .execute_debug(prog.closure, &ctx, &mut debug_state)
            .unwrap();
        assert_eq!(result, Value::int(7));

        // Should have received at least one Stopped event
        let event = event_rx.try_recv();
        assert!(
            event.is_ok(),
            "should have received a Stopped event from debug hook"
        );
        match event.unwrap() {
            DebugEvent::Stopped { .. } => {} // expected
            other => panic!("expected Stopped event, got {other:?}"),
        }
    }

    #[test]
    fn execute_debug_rejects_same_and_cross_context_runtime_quantums_before_execution() {
        for cross_context in [false, true] {
            let globals = Rc::new(Env::new());
            let calls = Rc::new(Cell::new(0));
            let calls_for_native = Rc::clone(&calls);
            globals.set(
                intern("debug-entry-probe"),
                Value::native_fn(NativeFn::simple("debug-entry-probe", move |_| {
                    calls_for_native.set(calls_for_native.get() + 1);
                    Ok(Value::nil())
                })),
            );
            let forms =
                sema_reader::read_many("(debug-entry-probe)").expect("fixture source parses");
            let program = compile_program(&forms, None).expect("fixture source compiles");
            let mut vm = VM::new(
                globals,
                program.functions,
                &program.native_table,
                program.main_cache_slots,
            )
            .expect("fixture VM constructs");
            let callback_context = EvalContext::new();
            let runtime_context = cross_context
                .then(EvalContext::new)
                .unwrap_or_else(EvalContext::new);
            let _quantum = if cross_context {
                runtime_context
                    .enter_runtime_quantum()
                    .expect("enter cross-context runtime quantum")
            } else {
                callback_context
                    .enter_runtime_quantum()
                    .expect("enter same-context runtime quantum")
            };
            let mut debug_state = crate::debug::DebugState::new_headless();

            let error = vm
                .execute_debug(program.closure, &callback_context, &mut debug_state)
                .expect_err("debug VM entry must be host-only");

            assert!(error
                .to_string()
                .contains("legacy native callback cannot re-enter a VM"));
            assert_eq!(calls.get(), 0, "debug bytecode must not execute");
            assert!(vm.frames.is_empty(), "guard must run before frame mutation");
            assert!(vm.stack.is_empty(), "guard must run before stack mutation");
        }
    }

    #[test]
    fn debug_evaluate_compiles_directly() {
        let globals = make_test_env();
        let forms = sema_reader::read_many("nil").expect("fixture source parses");
        let program = compile_program(&forms, None).expect("fixture source compiles");
        let mut vm = VM::new(
            globals,
            program.functions,
            &program.native_table,
            program.main_cache_slots,
        )
        .expect("fixture VM constructs");
        vm.seed_main_frame(program.closure);
        let ctx = EvalContext::new();
        let expr = sema_reader::read("(+ 1 2)").expect("debug expression parses");

        let value = vm
            .debug_evaluate(0, &expr, &ctx, &crate::debug::DebugState::new_headless())
            .expect("debug evaluation must compile directly");

        assert_eq!(value, Value::int(3));
    }

    #[test]
    fn debug_evaluate_uses_the_registered_macro_expander() {
        let globals = make_test_env();
        let forms = sema_reader::read_many("nil").expect("fixture source parses");
        let program = compile_program(&forms, None).expect("fixture source compiles");
        let mut vm = VM::new(
            globals,
            program.functions,
            &program.native_table,
            program.main_cache_slots,
        )
        .expect("fixture VM constructs");
        vm.seed_main_frame(program.closure);
        let ctx = EvalContext::new();
        sema_core::set_macro_expand_callback(&ctx, debug_macro_probe);
        let expr = sema_reader::read("(debug-expand-value)").expect("debug expression parses");

        let value = vm
            .debug_evaluate(0, &expr, &ctx, &crate::debug::DebugState::new_headless())
            .expect("debug evaluation uses the dedicated macro expander");

        assert_eq!(value, Value::int(42));
    }

    #[test]
    fn failed_debug_macro_expansion_rolls_back_paused_owner_writes() {
        let (mut vm, cell) = paused_debug_owner_with_open_mutator(Value::int(7));
        let ctx = EvalContext::new();
        sema_core::set_macro_expand_callback(&ctx, debug_macro_probe);
        let expr = sema_reader::read("(debug-expand-fail)").expect("debug expression parses");

        let error = vm
            .debug_evaluate(0, &expr, &ctx, &crate::debug::DebugState::new_headless())
            .expect_err("macro expansion failure must reject the debugger expression");

        assert!(matches!(
            error.inner(),
            SemaError::Eval(message) if message == "debug macro expansion failed"
        ));
        assert_eq!(vm.stack[0], Value::int(7));
        assert!(matches!(
            &*cell.state.borrow(),
            UpvalueState::Tracked { value, .. } if value == &Value::int(7)
        ));
    }

    #[test]
    fn failed_debug_compile_after_macro_expansion_rolls_back_paused_owner_writes() {
        let (mut vm, cell) = paused_debug_owner_with_open_mutator(Value::int(7));
        let ctx = EvalContext::new();
        sema_core::set_macro_expand_callback(&ctx, debug_macro_probe);
        let expr =
            sema_reader::read("(debug-expand-compile-fail)").expect("debug expression parses");

        vm.debug_evaluate(0, &expr, &ctx, &crate::debug::DebugState::new_headless())
            .expect_err("compile failure must reject the expanded debugger expression");

        assert_eq!(vm.stack[0], Value::int(7));
        assert!(matches!(
            &*cell.state.borrow(),
            UpvalueState::Tracked { value, .. } if value == &Value::int(7)
        ));
    }

    #[test]
    fn debug_evaluate_reads_locals_globals_and_upvalues() {
        let globals = make_test_env();
        globals.set(intern("global-value"), Value::int(4));
        let forms = sema_reader::read_many("nil").expect("fixture source parses");
        let program = compile_program(&forms, None).expect("fixture source compiles");
        let mut owner_func = (*program.closure.func).clone();
        owner_func.chunk.n_locals = 1;
        owner_func.local_names = vec![(0, intern("local-value"))];
        owner_func.upvalue_names = vec![intern("captured-value")];
        let owner_closure = Rc::new(Closure {
            func: Rc::new(owner_func),
            upvalues: vec![Rc::new(UpvalueCell::new_closed(Value::int(5)))],
            globals: None,
            functions: None,
        });
        let mut vm = VM::new(
            globals,
            program.functions,
            &program.native_table,
            program.main_cache_slots,
        )
        .expect("fixture VM constructs");
        vm.seed_main_frame(owner_closure);
        vm.stack[0] = Value::int(3);
        let ctx = EvalContext::new();
        let expr = sema_reader::read("(list local-value global-value captured-value)")
            .expect("debug expression parses");

        let value = vm
            .debug_evaluate(0, &expr, &ctx, &crate::debug::DebugState::new_headless())
            .expect("debug bindings evaluate");

        assert_eq!(
            value,
            Value::list(vec![Value::int(3), Value::int(4), Value::int(5)])
        );
    }

    #[test]
    fn debug_evaluate_drives_helper_hof_and_multimethod_calls() {
        let globals = make_test_env();
        globals.set(
            intern("debug-apply-one"),
            Value::native_fn(
                NativeFn::simple_result("debug-apply-one", |args| {
                    Ok(NativeOutcome::Call(NativeCall {
                        callable: args[0].clone(),
                        args: vec![args[1].clone()],
                        continuation: Box::new(DebugIdentityContinuation),
                    }))
                })
                .with_escaping_args(&[0]),
            ),
        );
        let dispatch = Value::native_fn(NativeFn::simple_result("debug-dispatch", |_| {
            Ok(NativeOutcome::Return(Value::keyword("selected")))
        }));
        let selected = Value::native_fn(NativeFn::simple_result("debug-selected", |args| {
            Ok(NativeOutcome::Return(args[0].clone()))
        }));
        let mut methods = BTreeMap::new();
        methods.insert(Value::keyword("selected"), selected);
        globals.set(
            intern("debug-mm"),
            Value::multimethod(MultiMethod {
                name: intern("debug-mm"),
                dispatch_fn: dispatch,
                methods: RefCell::new(methods),
                default: RefCell::new(None),
            }),
        );
        let forms = sema_reader::read_many("nil").expect("fixture source parses");
        let program = compile_program(&forms, None).expect("fixture source compiles");
        let mut vm = VM::new(
            globals,
            program.functions,
            &program.native_table,
            program.main_cache_slots,
        )
        .expect("fixture VM constructs");
        vm.seed_main_frame(program.closure);
        let ctx = EvalContext::new();
        let expr = sema_reader::read(
            "(let ((helper (lambda (value) (+ value 1))))\
               (list (debug-apply-one helper 41) (debug-mm 43)))",
        )
        .expect("debug expression parses");

        let value = vm
            .debug_evaluate(0, &expr, &ctx, &crate::debug::DebugState::new_headless())
            .expect("debug helper, HOF, and multimethod calls complete inline");

        assert_eq!(value, Value::list(vec![Value::int(42), Value::int(43)]));
    }

    #[test]
    fn debug_evaluate_installs_the_exact_task_context() {
        let globals = make_test_env();
        globals.set(
            intern("debug-context-marker"),
            Value::native_fn(NativeFn::with_context_result(
                "debug-context-marker",
                |context, _| {
                    let marker = context
                        .task_context
                        .borrow()
                        .get::<DebugContextMarker>()
                        .map(|marker| marker.0)
                        .ok_or_else(|| SemaError::eval("debug task context marker is missing"))?;
                    Ok(NativeOutcome::Return(Value::int(marker)))
                },
            )),
        );
        let forms = sema_reader::read_many("nil").expect("fixture source parses");
        let program = compile_program(&forms, None).expect("fixture source compiles");
        let mut vm = VM::new(
            globals,
            program.functions,
            &program.native_table,
            program.main_cache_slots,
        )
        .expect("fixture VM constructs");
        vm.seed_main_frame(program.closure);
        let task_context = TaskContextHandle::default();
        task_context
            .borrow_mut()
            .insert(Rc::new(DebugContextMarker(77)));
        let ctx = EvalContext::new();
        ctx.install_task_context(task_context);
        let expr = sema_reader::read("(debug-context-marker)").expect("debug expression parses");

        let value = vm
            .debug_evaluate(0, &expr, &ctx, &crate::debug::DebugState::new_headless())
            .expect("debug evaluation sees the active task context");

        assert_eq!(value, Value::int(77));
    }

    #[test]
    fn debug_evaluate_enforces_budget_and_current_cancellation() {
        let globals = make_test_env();
        let forms = sema_reader::read_many("nil").expect("fixture source parses");
        let program = compile_program(&forms, None).expect("fixture source compiles");
        let mut vm = VM::new(
            globals,
            program.functions,
            &program.native_table,
            program.main_cache_slots,
        )
        .expect("fixture VM constructs");
        vm.seed_main_frame(program.closure);
        let ctx = EvalContext::new();
        let infinite = sema_reader::read("(let loop () (loop))").expect("debug expression parses");

        let budget_error = vm
            .debug_evaluate(
                0,
                &infinite,
                &ctx,
                &crate::debug::DebugState::new_headless(),
            )
            .expect_err("infinite debug evaluation must hit its budget");
        assert!(matches!(
            budget_error.inner(),
            SemaError::Eval(message) if message == "debug evaluation exceeded instruction limit"
        ));

        vm.quantum_cancellation = CancellationView::new(true, None);
        let literal = sema_reader::read("42").expect("debug expression parses");
        let cancellation_error = vm
            .debug_evaluate(0, &literal, &ctx, &crate::debug::DebugState::new_headless())
            .expect_err("debug evaluation must observe current cancellation");
        assert!(matches!(
            cancellation_error.inner(),
            SemaError::Eval(message) if message == "debug evaluation was cancelled"
        ));
    }

    #[test]
    fn conditional_debug_evaluation_is_boolean_and_fails_open_on_errors() {
        let globals = make_test_env();
        globals.set(
            intern("debug-suspend"),
            Value::native_fn(NativeFn::simple_with_runtime(
                "debug-suspend",
                |_| Ok(Value::nil()),
                |_, _| {
                    Ok(NativeOutcome::Suspend(NativeSuspend {
                        wait: WaitKind::Timer(Duration::from_secs(1)),
                        continuation: Box::new(DebugIdentityContinuation),
                    }))
                },
            )),
        );
        let forms = sema_reader::read_many("nil").expect("fixture source parses");
        let program = compile_program(&forms, None).expect("fixture source compiles");
        let mut vm = VM::new(
            globals,
            program.functions,
            &program.native_table,
            program.main_cache_slots,
        )
        .expect("fixture VM constructs");
        vm.seed_main_frame(program.closure);
        let ctx = EvalContext::new();
        let file = std::path::PathBuf::from("debug-condition-test.sema");
        let mut allows_stop = |condition: &str| {
            let mut debug = crate::debug::DebugState::new_headless();
            debug.set_breakpoints_with_conditions(
                &file,
                &[crate::debug::SourceBreakpoint {
                    line: 1,
                    condition: Some(condition.to_string()),
                }],
            );
            vm.debug_condition_allows_stop(Some(&file), 1, &debug, &ctx)
        };

        assert!(allows_stop("#t"));
        assert!(!allows_stop("#f"));
        assert!(allows_stop("("), "parse errors must fail open");
        assert!(allows_stop("missing-name"), "eval errors must fail open");
        assert!(
            allows_stop("(let loop () (loop))"),
            "budget errors must fail open"
        );
        assert!(
            allows_stop("(debug-suspend)"),
            "suspension errors must fail open"
        );
    }

    #[test]
    fn debug_evaluate_snapshots_global_closures_against_the_paused_owner() {
        let (mut vm, cell) = paused_debug_owner_with_open_mutator(Value::int(7));
        let ctx = EvalContext::new();
        let expr = sema_reader::read("(mutate-captured 99)").expect("debug expression parses");

        let value = vm
            .debug_evaluate(0, &expr, &ctx, &crate::debug::DebugState::new_headless())
            .expect("debug evaluation runs a closure reached through globals");

        assert_eq!(value, Value::int(99));
        assert_eq!(vm.stack[0], Value::int(99));
        assert!(matches!(
            &*cell.state.borrow(),
            UpvalueState::Tracked { value, .. } if value == &Value::int(99)
        ));
    }

    #[test]
    fn debug_evaluate_snapshots_closures_reached_through_a_closure_home_env() {
        let (mut vm, cell) = paused_debug_owner_with_open_mutator(Value::int(7));
        let inner = vm
            .globals
            .take(intern("mutate-captured"))
            .expect("inner mutator is installed");
        let module_env = Rc::new(Env::with_parent(Rc::clone(&vm.globals)));
        module_env.set(intern("mutate-captured"), inner);
        let ctx = EvalContext::new();
        let outer = eval_str(
            "(lambda (value) (mutate-captured value))",
            &module_env,
            &ctx,
        )
        .expect("outer module closure evaluates");
        vm.globals.set(intern("transitive-mutate"), outer);
        let expr = sema_reader::read("(transitive-mutate 99)").expect("debug expression parses");

        let value = vm
            .debug_evaluate(0, &expr, &ctx, &crate::debug::DebugState::new_headless())
            .expect("debug evaluation follows a closure home environment");

        assert_eq!(value, Value::int(99));
        assert_eq!(vm.stack[0], Value::int(99));
        assert!(matches!(
            &*cell.state.borrow(),
            UpvalueState::Tracked { value, .. } if value == &Value::int(99)
        ));
    }

    #[test]
    fn debug_evaluate_snapshots_closures_reached_through_function_constants() {
        let (mut vm, cell) = paused_debug_owner_with_open_mutator(Value::int(7));
        let mutator = vm
            .globals
            .take(intern("mutate-captured"))
            .expect("mutator fixture is installed");
        let ctx = EvalContext::new();
        let holder_template = eval_str(
            "(lambda () (lambda () \"debug-constant-sentinel\"))",
            &vm.globals,
            &ctx,
        )
        .expect("constant holder template evaluates");
        let (template_closure, functions, native_fns) =
            extract_vm_closure(&holder_template).expect("holder template is a VM closure");
        let sentinel = Value::string("debug-constant-sentinel");
        let mut rewritten_functions: Vec<Rc<Function>> = functions.iter().cloned().collect();
        let mut replaced = false;
        for function in &mut rewritten_functions {
            let Some(const_index) = function
                .chunk
                .consts
                .iter()
                .position(|value| value == &sentinel)
            else {
                continue;
            };
            let mut rewritten = (**function).clone();
            rewritten.chunk.consts[const_index] = mutator.clone();
            *function = Rc::new(rewritten);
            replaced = true;
        }
        assert!(
            replaced,
            "the nested function contains the sentinel constant"
        );
        let functions = Rc::new(rewritten_functions);
        let holder_closure = Rc::new(Closure {
            func: Rc::clone(&template_closure.func),
            upvalues: template_closure.upvalues.clone(),
            globals: template_closure.globals.clone(),
            functions: Some(Rc::clone(&functions)),
        });
        let payload = Rc::new(VmClosurePayload {
            closure: holder_closure,
            functions,
            native_fns,
        });
        let mut holder =
            NativeFn::with_payload("constant-holder", payload as Rc<dyn Any>, |_, _| {
                Ok(Value::nil())
            });
        holder.is_closure = true;
        vm.globals
            .set(intern("constant-holder"), Value::native_fn(holder));
        let expr = sema_reader::read("(((constant-holder)) 99)").expect("debug expression parses");

        let value = vm
            .debug_evaluate(0, &expr, &ctx, &crate::debug::DebugState::new_headless())
            .expect("debug evaluation follows constants in the holder's function table");

        assert_eq!(value, Value::int(99));
        assert_eq!(vm.stack[0], Value::int(99));
        assert!(matches!(
            &*cell.state.borrow(),
            UpvalueState::Tracked { value, .. } if value == &Value::int(99)
        ));
    }

    #[test]
    fn debug_evaluate_snapshots_an_open_owner_through_an_opaque_payload() {
        let (mut vm, cell) = paused_debug_owner_with_open_mutator(Value::int(7));
        let mutator = vm
            .globals
            .take(intern("mutate-captured"))
            .expect("mutator fixture is installed");
        vm.stack[0] = debug_handler_value(mutator);
        let mut owner_func = (*vm.frames[0].closure.func).clone();
        owner_func.local_names = vec![(0, intern("route"))];
        vm.frames[0].closure = Rc::new(Closure {
            func: Rc::new(owner_func),
            upvalues: Vec::new(),
            globals: None,
            functions: None,
        });
        let ctx = EvalContext::new();
        let expr = sema_reader::read("(route 99)").expect("debug expression parses");

        let value = vm
            .debug_evaluate(0, &expr, &ctx, &crate::debug::DebugState::new_headless())
            .expect("debug evaluation follows the handler retained by an opaque payload");

        assert_eq!(value, Value::int(99));
        assert_eq!(vm.stack[0], Value::int(99));
        assert!(matches!(
            &*cell.state.borrow(),
            UpvalueState::Tracked { value, .. } if value == &Value::int(99)
        ));
    }

    #[test]
    fn failed_debug_evaluate_rolls_back_a_closed_cell_through_an_opaque_payload() {
        let (mut vm, cell) = paused_debug_owner_with_open_mutator(Value::int(7));
        let mutator = vm
            .globals
            .take(intern("mutate-captured"))
            .expect("mutator fixture is installed");
        *cell.state.borrow_mut() = UpvalueState::Closed(Value::int(7));
        vm.frames[0].open_upvalues = None;
        vm.stack[0] = debug_handler_value(mutator);
        let mut owner_func = (*vm.frames[0].closure.func).clone();
        owner_func.local_names = vec![(0, intern("route"))];
        vm.frames[0].closure = Rc::new(Closure {
            func: Rc::new(owner_func),
            upvalues: Vec::new(),
            globals: None,
            functions: None,
        });
        vm.globals.set(
            intern("debug-suspend"),
            Value::native_fn(NativeFn::simple_with_runtime(
                "debug-suspend",
                |_| Ok(Value::nil()),
                |_, _| {
                    Ok(NativeOutcome::Suspend(NativeSuspend {
                        wait: WaitKind::Timer(Duration::from_secs(1)),
                        continuation: Box::new(DebugIdentityContinuation),
                    }))
                },
            )),
        );
        let ctx = EvalContext::new();
        let expr = sema_reader::read("(begin (route 99) (debug-suspend))")
            .expect("debug expression parses");

        let error = vm
            .debug_evaluate(0, &expr, &ctx, &crate::debug::DebugState::new_headless())
            .expect_err("a debugger expression must reject suspension");

        assert!(matches!(
            error.inner(),
            SemaError::Eval(message) if message == "debug evaluation cannot suspend"
        ));
        assert!(matches!(
            &*cell.state.borrow(),
            UpvalueState::Closed(value) if value == &Value::int(7)
        ));
    }

    #[test]
    fn failed_debug_set_rolls_back_reachable_tracked_upvalue_writes() {
        let (mut vm, cell) = paused_debug_owner_with_open_mutator(Value::int(7));
        vm.globals.set(
            intern("debug-suspend"),
            Value::native_fn(NativeFn::simple_with_runtime(
                "debug-suspend",
                |_| Ok(Value::nil()),
                |_, _| {
                    Ok(NativeOutcome::Suspend(NativeSuspend {
                        wait: WaitKind::Timer(Duration::from_secs(1)),
                        continuation: Box::new(DebugIdentityContinuation),
                    }))
                },
            )),
        );
        let ctx = EvalContext::new();
        let expr =
            sema_reader::read("(set! captured (begin (mutate-captured 99) (debug-suspend)))")
                .expect("debug expression parses");

        let error = vm
            .debug_evaluate_mut(0, &expr, &ctx, &crate::debug::DebugState::new_headless())
            .expect_err("a debugger expression must reject suspension");

        assert!(matches!(
            error.inner(),
            SemaError::Eval(message) if message == "debug evaluation cannot suspend"
        ));
        assert_eq!(vm.stack[0], Value::int(7));
        assert!(matches!(
            &*cell.state.borrow(),
            UpvalueState::Tracked { value, .. } if value == &Value::int(7)
        ));
    }

    #[test]
    fn failed_debug_set_restores_a_closed_upvalue_target_mutated_by_a_helper() {
        let (mut vm, cell) = paused_debug_owner_with_closed_upvalue_mutator(Value::int(7));
        vm.globals.set(
            intern("debug-suspend"),
            Value::native_fn(NativeFn::simple_with_runtime(
                "debug-suspend",
                |_| Ok(Value::nil()),
                |_, _| {
                    Ok(NativeOutcome::Suspend(NativeSuspend {
                        wait: WaitKind::Timer(Duration::from_secs(1)),
                        continuation: Box::new(DebugIdentityContinuation),
                    }))
                },
            )),
        );
        let ctx = EvalContext::new();
        let expr = sema_reader::read(
            "(set! captured-upvalue (begin (mutate-captured 99) (debug-suspend)))",
        )
        .expect("debug expression parses");

        let error = vm
            .debug_evaluate_mut(0, &expr, &ctx, &crate::debug::DebugState::new_headless())
            .expect_err("a debugger expression must reject suspension");

        assert!(matches!(
            error.inner(),
            SemaError::Eval(message) if message == "debug evaluation cannot suspend"
        ));
        assert!(matches!(
            &*cell.state.borrow(),
            UpvalueState::Closed(value) if value == &Value::int(7)
        ));
    }

    #[test]
    fn successful_debug_set_updates_a_tracked_local_cell() {
        let (mut vm, cell) = paused_debug_owner_with_open_mutator(Value::int(7));
        let ctx = EvalContext::new();
        let expr = sema_reader::read("(set! captured 42)").expect("debug expression parses");

        let value = vm
            .debug_evaluate_mut(0, &expr, &ctx, &crate::debug::DebugState::new_headless())
            .expect("debug assignment succeeds");

        assert_eq!(value, Value::int(42));
        assert_eq!(vm.stack[0], Value::int(42));
        assert!(matches!(
            &*cell.state.borrow(),
            UpvalueState::Tracked { value, .. } if value == &Value::int(42)
        ));
    }

    #[test]
    fn debug_set_tracked_upvalue_synchronizes_its_live_defining_frame() {
        let (mut vm, child_closure, cell) = open_upvalue_fixture(Value::int(7));
        *cell.state.borrow_mut() = UpvalueState::Tracked {
            frame_base: 0,
            slot: 0,
            value: Value::int(7),
        };
        let mut parent_func = (*vm.frames[0].closure.func).clone();
        parent_func.chunk.n_locals = 1;
        parent_func.local_names = vec![(0, intern("captured"))];
        vm.frames[0].closure = Rc::new(Closure {
            func: Rc::new(parent_func),
            upvalues: Vec::new(),
            globals: None,
            functions: None,
        });
        let mut child_func = (*child_closure.func).clone();
        child_func.upvalue_names = vec![intern("captured")];
        vm.frames.push(CallFrame {
            closure: Rc::new(Closure {
                func: Rc::new(child_func),
                upvalues: vec![Rc::clone(&cell)],
                globals: child_closure.globals.clone(),
                functions: child_closure.functions.clone(),
            }),
            pc: 0,
            base: 1,
            open_upvalues: None,
            cache_base: 0,
        });
        vm.stack.push(Value::nil());

        vm.debug_set_variable(
            crate::debug::scope_upvalues_ref(1),
            "captured",
            Value::int(42),
        )
        .expect("tracked child upvalue is writable");

        assert!(matches!(
            &*cell.state.borrow(),
            UpvalueState::Tracked { value, .. } if value == &Value::int(42)
        ));
        assert_eq!(
            vm.stack[0],
            Value::int(42),
            "the defining frame's next LOAD_LOCAL must observe the write"
        );
    }

    #[test]
    fn debug_set_variable_rejects_a_missing_target_before_evaluating_its_rhs() {
        let (mut vm, cell) = paused_debug_owner_with_closed_upvalue_mutator(Value::int(7));
        let ctx = EvalContext::new();

        let error = vm
            .debug_set_variable_expression(
                crate::debug::scope_locals_ref(0),
                "missing",
                "(mutate-captured 99)",
                &ctx,
                &crate::debug::DebugState::new_headless(),
            )
            .expect_err("a missing setVariable target must be rejected");

        assert_eq!(error, "Eval error: setVariable: local 'missing' not found");
        assert!(matches!(
            &*cell.state.borrow(),
            UpvalueState::Closed(value) if value == &Value::int(7)
        ));
    }

    #[test]
    fn failed_debug_set_restores_closed_cells_nested_in_stopped_local_values() {
        let (mut vm, cell) = paused_debug_owner_with_open_mutator(Value::int(7));
        *cell.state.borrow_mut() = UpvalueState::Closed(Value::int(7));
        vm.frames[0].open_upvalues = None;
        vm.stack[0] = vm
            .globals
            .get(intern("mutate-captured"))
            .expect("nested closure fixture is installed");
        let ctx = EvalContext::new();
        let global_mutator = eval_str(
            "(let ((global-counter 0))\
               (lambda (value) (set! global-counter value) global-counter))",
            &vm.globals,
            &ctx,
        )
        .expect("independent global mutator evaluates");
        let (global_closure, _, _) =
            extract_vm_closure(&global_mutator).expect("global mutator is a VM closure");
        let global_cell = Rc::clone(&global_closure.upvalues[0]);
        vm.globals.set(intern("mutate-global"), global_mutator);
        vm.globals.set(
            intern("debug-suspend"),
            Value::native_fn(NativeFn::simple_with_runtime(
                "debug-suspend",
                |_| Ok(Value::nil()),
                |_, _| {
                    Ok(NativeOutcome::Suspend(NativeSuspend {
                        wait: WaitKind::Timer(Duration::from_secs(1)),
                        continuation: Box::new(DebugIdentityContinuation),
                    }))
                },
            )),
        );
        let expr = sema_reader::read(
            "(set! captured\
               (begin (mutate-captured 99) (mutate-global 88) (debug-suspend)))",
        )
        .expect("debug expression parses");

        let error = vm
            .debug_evaluate_mut(0, &expr, &ctx, &crate::debug::DebugState::new_headless())
            .expect_err("a debugger expression must reject suspension");

        assert!(matches!(
            error.inner(),
            SemaError::Eval(message) if message == "debug evaluation cannot suspend"
        ));
        assert!(vm.stack[0].as_native_fn_ref().is_some());
        assert!(matches!(
            &*cell.state.borrow(),
            UpvalueState::Closed(value) if value == &Value::int(7)
        ));
        assert!(matches!(
            &*global_cell.state.borrow(),
            UpvalueState::Closed(value) if value == &Value::int(88)
        ));
    }

    #[test]
    fn debug_evaluate_rejects_a_stopped_value_graph_over_the_snapshot_limit() {
        let globals = make_test_env();
        let forms = sema_reader::read_many("nil").expect("fixture source parses");
        let program = compile_program(&forms, None).expect("fixture source compiles");
        let mut owner_func = (*program.closure.func).clone();
        owner_func.chunk.n_locals = 1;
        owner_func.local_names = vec![(0, intern("wide-value"))];
        let owner_closure = Rc::new(Closure {
            func: Rc::new(owner_func),
            upvalues: Vec::new(),
            globals: None,
            functions: None,
        });
        let mut vm = VM::new(
            globals,
            program.functions,
            &program.native_table,
            program.main_cache_slots,
        )
        .expect("fixture VM constructs");
        vm.seed_main_frame(owner_closure);
        vm.stack[0] = Value::list(vec![Value::nil(); DEBUG_EVALUATION_SNAPSHOT_NODE_LIMIT + 1]);
        let ctx = EvalContext::new();
        let expr = sema_reader::read("42").expect("debug expression parses");

        let error = vm
            .debug_evaluate(0, &expr, &ctx, &crate::debug::DebugState::new_headless())
            .expect_err("wide stopped values must hit the snapshot limit");

        assert!(matches!(
            error.inner(),
            SemaError::Eval(message)
                if message == "debug evaluation exceeded snapshot node limit"
        ));
    }

    #[test]
    fn debug_evaluate_walks_deep_acyclic_stopped_values_iteratively() {
        const DEPTH: usize = 20_000;

        let globals = make_test_env();
        let forms = sema_reader::read_many("nil").expect("fixture source parses");
        let program = compile_program(&forms, None).expect("fixture source compiles");
        let mut owner_func = (*program.closure.func).clone();
        owner_func.chunk.n_locals = 1;
        owner_func.local_names = vec![(0, intern("deep-value"))];
        let owner_closure = Rc::new(Closure {
            func: Rc::new(owner_func),
            upvalues: Vec::new(),
            globals: None,
            functions: None,
        });
        let mut vm = VM::new(
            globals,
            program.functions,
            &program.native_table,
            program.main_cache_slots,
        )
        .expect("fixture VM constructs");
        vm.seed_main_frame(owner_closure);
        let mut value = Value::nil();
        let mut drop_pins = Vec::with_capacity(DEPTH);
        for _ in 0..DEPTH {
            value = Value::list(vec![value]);
            drop_pins.push(value.clone());
        }
        vm.stack[0] = value;
        let ctx = EvalContext::new();
        let expr = sema_reader::read("42").expect("debug expression parses");

        let result = vm
            .debug_evaluate(0, &expr, &ctx, &crate::debug::DebugState::new_headless())
            .expect("deep acyclic debugger values stay within the node limit");

        assert_eq!(result, Value::int(42));
        // Keep every chain link alive independently so test teardown does not
        // recursively drop a deliberately pathological value graph.
        std::mem::forget(drop_pins);
    }

    #[test]
    fn debug_get_stacktrace_while_running_replies() {
        // DAP-1: a state query (GetStackTrace) sent while the program is still
        // running must be answered by the running-VM poll loop, not dropped.
        // A dropped reply leaves the DAP server's spawn_blocking thread blocked
        // on recv() forever (session hang / leaked thread).
        use crate::debug::{DebugCommand, DebugState, StepMode};
        use std::sync::mpsc;

        let globals = make_test_env();
        let ctx = EvalContext::new();
        // A loop long enough to cross the 128-instruction poll interval.
        let input = "(define (loop n acc) (if (= n 0) acc (loop (- n 1) (+ acc n))))\n(loop 200 0)";
        let (vals, span_map) = sema_reader::read_many_with_spans(input).unwrap();
        let prog = compile_program_with_spans(&vals, &span_map, None).unwrap();

        let (event_tx, _event_rx) = mpsc::channel();
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let mut debug_state = DebugState::new(event_tx, cmd_rx);
        debug_state.step_mode = StepMode::Continue; // run straight through, no stops

        // Queue a stack-trace request to be serviced mid-run.
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        cmd_tx
            .send(DebugCommand::GetStackTrace { reply: reply_tx })
            .unwrap();

        let mut vm = VM::new(globals, prog.functions, &[], prog.main_cache_slots).unwrap();
        let result = vm
            .execute_debug(prog.closure, &ctx, &mut debug_state)
            .unwrap();
        assert_eq!(result, Value::int(20100)); // sum 1..=200

        let frames = reply_rx
            .try_recv()
            .expect("GetStackTrace while running must receive a reply, not be dropped");
        assert!(
            !frames.is_empty(),
            "stack trace should have at least one frame"
        );
    }

    #[test]
    fn test_vm_oob_jump_returns_error() {
        // Issue #1: A jump that goes past the end of bytecode should return
        // an error, not cause undefined behavior from unsafe pointer reads.
        // (A crafted .semac with such a jump never reaches the VM — see
        // `semac_oob_jump_rejected_at_load` in
        // tests/bytecode_validator_regression.rs.)
        use crate::emit::Emitter;
        use crate::opcodes::Op;

        let globals = Rc::new(Env::new());
        let ctx = EvalContext::new();
        let mut e = Emitter::new();
        // Emit a forward JUMP with an offset that goes way past the end
        e.emit_op(Op::Jump);
        e.emit_i32(1000); // jump to PC 1005, but code is only ~6 bytes
        e.emit_op(Op::Return);
        let func = Rc::new(crate::chunk::Function {
            name: None,
            chunk: e.into_chunk(),
            upvalue_descs: vec![],
            upvalue_names: vec![],
            arity: 0,
            has_rest: false,
            param_names: Vec::new().into(),
            local_names: vec![],
            source_file: None,
            local_scopes: Vec::new(),
            cache_offset: 0,
        });
        let closure = Rc::new(Closure {
            func,
            upvalues: vec![],
            globals: None,
            functions: None,
        });
        let mut vm = VM::new(globals, vec![], &[], 0).unwrap();
        let result = vm.execute(closure, &ctx);
        assert!(
            result.is_err(),
            "out-of-bounds jump should return an error, not UB"
        );
    }

    #[test]
    fn test_valid_breakpoint_lines_extraction() {
        // Verify that valid_breakpoint_lines correctly extracts lines with spans
        let code = "(+ 1 2)\n\n; comment\n(+ 3 4)";
        let (vals, span_map) = sema_reader::read_many_with_spans(code).unwrap();
        let prog = compile_program_with_spans(&vals, &span_map, None).unwrap();
        let lines = valid_breakpoint_lines(&prog.closure, &prog.functions);

        assert!(lines.contains(&1), "line 1 (expr) should be valid");
        assert!(!lines.contains(&2), "line 2 (empty) should not be valid");
        assert!(!lines.contains(&3), "line 3 (comment) should not be valid");
        assert!(lines.contains(&4), "line 4 (expr) should be valid");
    }

    #[test]
    fn test_snap_breakpoint_line() {
        let valid = vec![1, 3, 5, 10];

        // Exact match
        assert_eq!(snap_breakpoint_line(3, &valid), Some(3));
        // Snap forward (closer)
        assert_eq!(snap_breakpoint_line(4, &valid), Some(5));
        // Equidistant between 1 and 3: prefers forward
        assert_eq!(snap_breakpoint_line(2, &valid), Some(3));
        // Equidistant: prefers forward
        assert_eq!(snap_breakpoint_line(4, &[3, 5]), Some(5));
        // Past the end: snaps to last
        assert_eq!(snap_breakpoint_line(20, &valid), Some(10));
        // Before the start: snaps to first
        assert_eq!(snap_breakpoint_line(0, &valid), Some(1));
        // Empty valid lines
        assert_eq!(snap_breakpoint_line(5, &[]), None);
    }

    #[test]
    fn test_bare_literal_breakpoint_snaps() {
        // Bare literals (like `42` or `"hello"`) don't get spans.
        // A breakpoint on a bare literal line should snap to the nearest line with spans.
        let code = "\"hello\"\n42\n(+ 1 2)";
        let (vals, span_map) = sema_reader::read_many_with_spans(code).unwrap();
        let prog = compile_program_with_spans(&vals, &span_map, None).unwrap();
        let valid = valid_breakpoint_lines(&prog.closure, &prog.functions);

        // Lines 1 and 2 are bare literals — no spans
        assert!(!valid.contains(&1), "bare string should lack span");
        assert!(!valid.contains(&2), "bare int should lack span");
        assert!(valid.contains(&3), "function call should have span");

        // Snapping: line 1 and 2 should snap to line 3
        assert_eq!(snap_breakpoint_line(1, &valid), Some(3));
        assert_eq!(snap_breakpoint_line(2, &valid), Some(3));
    }

    #[test]
    fn test_debug_variables_expand_records_with_field_names() {
        let mut vm = VM::new(make_test_env(), vec![], &[], 0).unwrap();
        let point_value = Value::record(sema_core::Record {
            type_tag: intern("point"),
            field_names: vec![intern("x"), intern("y")],
            fields: vec![Value::int(3), Value::int(4)],
        });
        let point = vm.debug_value_to_variable("p", point_value);
        assert!(
            point.variables_reference > 0,
            "record local should be expandable: {point:?}"
        );

        let fields = vm.debug_variables(point.variables_reference);
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].name, "x");
        assert_eq!(fields[0].value, "3");
        assert_eq!(fields[1].name, "y");
        assert_eq!(fields[1].value, "4");
    }

    #[test]
    fn test_debug_variables_expand_records_with_fallback_field_names() {
        let mut vm = VM::new(make_test_env(), vec![], &[], 0).unwrap();
        let point_value = Value::record(sema_core::Record {
            type_tag: intern("point"),
            field_names: Vec::new(),
            fields: vec![Value::int(3), Value::int(4)],
        });
        let point = vm.debug_value_to_variable("p", point_value);

        let fields = vm.debug_variables(point.variables_reference);
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].name, "field_0");
        assert_eq!(fields[1].name, "field_1");
    }

    #[test]
    fn test_valid_breakpoint_lines_are_grouped_by_source_file() {
        use std::path::PathBuf;

        let source_a = PathBuf::from("/tmp/sema-debug-a.sema");
        let source_b = PathBuf::from("/tmp/sema-debug-b.sema");
        let mut chunk_a = Chunk::new();
        chunk_a.spans.push((
            0,
            sema_core::Span {
                line: 10,
                col: 0,
                end_line: 10,
                end_col: 1,
            },
        ));
        let func_a = Rc::new(Function {
            name: None,
            chunk: chunk_a,
            upvalue_descs: vec![],
            arity: 0,
            has_rest: false,
            param_names: Vec::new().into(),
            local_names: vec![],
            upvalue_names: vec![],
            source_file: Some(source_a.clone()),
            local_scopes: Vec::new(),
            cache_offset: 0,
        });

        let mut chunk_b = Chunk::new();
        chunk_b.spans.push((
            0,
            sema_core::Span {
                line: 20,
                col: 0,
                end_line: 20,
                end_col: 1,
            },
        ));
        let func_b = Rc::new(Function {
            name: None,
            chunk: chunk_b,
            upvalue_descs: vec![],
            arity: 0,
            has_rest: false,
            param_names: Vec::new().into(),
            local_names: vec![],
            upvalue_names: vec![],
            source_file: Some(source_b.clone()),
            local_scopes: Vec::new(),
            cache_offset: 0,
        });

        let main = Rc::new(Closure {
            func: func_a.clone(),
            upvalues: vec![],
            globals: None,
            functions: None,
        });
        let lines = valid_breakpoint_lines_by_file(&main, &[func_a, func_b]);

        assert_eq!(lines.get(&source_a), Some(&vec![10]));
        assert_eq!(lines.get(&source_b), Some(&vec![20]));
    }

    #[test]
    fn test_global_redefinition_is_idempotent() {
        // Issue #3: The HTTP replay-restart strategy re-executes side effects.
        // Verify that re-defining globals doesn't error — (define x ...) twice
        // should just overwrite, not fail.
        let globals = make_test_env();
        let ctx = EvalContext::new();

        // First run: define x
        eval_str("(define x 42)", &globals, &ctx).unwrap();
        assert_eq!(eval_str("x", &globals, &ctx).unwrap(), Value::int(42));

        // Second run (simulating replay): redefine x with same value
        eval_str("(define x 42)", &globals, &ctx).unwrap();
        assert_eq!(eval_str("x", &globals, &ctx).unwrap(), Value::int(42));

        // Third run: redefine with different value (replay after mutation)
        eval_str("(define x 99)", &globals, &ctx).unwrap();
        assert_eq!(eval_str("x", &globals, &ctx).unwrap(), Value::int(99));
    }

    #[test]
    fn test_spans_in_compiled_chunks() {
        let input = "(+ 1 2)\n(+ 3 4)";
        let (vals, span_map) = sema_reader::read_many_with_spans(input).unwrap();
        let prog = compile_program_with_spans(&vals, &span_map, None).unwrap();
        assert!(
            !prog.closure.func.chunk.spans.is_empty(),
            "spans should be populated"
        );
        // Verify spans have correct line numbers
        let lines: Vec<u32> = prog
            .closure
            .func
            .chunk
            .spans
            .iter()
            .map(|(_, s)| s.line as u32)
            .collect();
        assert!(lines.contains(&1), "should have span on line 1");
        assert!(lines.contains(&2), "should have span on line 2");
    }

    #[test]
    fn test_vm_div_opcode_direct() {
        use crate::emit::Emitter;
        use crate::opcodes::Op;
        let globals = Rc::new(Env::new());
        let ctx = EvalContext::new();
        let mut e = Emitter::new();
        e.emit_const(Value::int(3)).unwrap();
        e.emit_const(Value::int(2)).unwrap();
        e.emit_op(Op::Div);
        e.emit_op(Op::Return);
        let func = Rc::new(crate::chunk::Function {
            name: None,
            chunk: e.into_chunk(),
            upvalue_descs: vec![],
            upvalue_names: vec![],
            arity: 0,
            has_rest: false,
            param_names: Vec::new().into(),
            local_names: vec![],
            source_file: None,
            local_scopes: Vec::new(),
            cache_offset: 0,
        });
        let closure = Rc::new(Closure {
            func,
            upvalues: vec![],
            globals: None,
            functions: None,
        });
        let mut vm = VM::new(globals.clone(), vec![], &[], 0).unwrap();
        let result = vm.execute(closure, &ctx).unwrap();
        // R7RS: exact/exact division yields an exact rational (3/2), not a float or 1.
        assert_eq!(
            result,
            eval_str("3/2", &globals, &ctx).unwrap(),
            "Op::Div 3/2 should return the exact rational 3/2"
        );
    }

    // ---- CallNative tests ----
    // These exercise the CallNative opcode path by compiling with known_natives.

    /// Make a test env with non-intrinsic native functions for CallNative testing.
    /// Arithmetic (+, -, *, /) are lowered as intrinsic opcodes and never go
    /// through CallNative, so we need other native functions.
    fn make_call_native_env() -> Rc<Env> {
        let env = make_test_env(); // keep arithmetic for general use
                                   // Add non-intrinsic natives that WILL go through CallNative
        env.set(
            intern("identity"),
            Value::native_fn(NativeFn::simple("identity", |args| Ok(args[0].clone()))),
        );
        env.set(
            intern("add1"),
            Value::native_fn(NativeFn::simple("add1", |args| {
                Ok(Value::int(args[0].as_int().unwrap() + 1))
            })),
        );
        env.set(
            intern("explode"),
            Value::native_fn(NativeFn::simple("explode", |_args| {
                Err(SemaError::eval("boom"))
            })),
        );
        env.set(
            intern("type-explode"),
            Value::native_fn(NativeFn::simple("type-explode", |_args| {
                Err(SemaError::type_error("number", "string"))
            })),
        );
        env
    }

    fn eval_str_with_call_native(
        input: &str,
        globals: &Rc<Env>,
        ctx: &EvalContext,
    ) -> Result<Value, SemaError> {
        let vals = sema_reader::read_many(input)
            .map_err(|e| SemaError::eval(format!("parse error: {e}")))?;
        // Collect all native function names from the env
        let known: std::collections::HashSet<_> = globals
            .all_names()
            .into_iter()
            .filter(|&spur| globals.get(spur).is_some_and(|v| v.is_native_fn()))
            .collect();
        let prog = compile_program(&vals, Some(known))?;
        let mut vm = VM::new(
            globals.clone(),
            prog.functions,
            &prog.native_table,
            prog.main_cache_slots,
        )?;
        vm.execute(prog.closure, ctx)
    }

    #[test]
    fn test_call_native_basic() {
        let globals = make_call_native_env();
        let ctx = EvalContext::new();
        // identity and add1 should go through CallNative path
        assert_eq!(
            eval_str_with_call_native("(identity 42)", &globals, &ctx).unwrap(),
            Value::int(42)
        );
        assert_eq!(
            eval_str_with_call_native("(add1 9)", &globals, &ctx).unwrap(),
            Value::int(10)
        );
    }

    #[test]
    fn test_call_native_nested() {
        let globals = make_call_native_env();
        let ctx = EvalContext::new();
        // Nested: (add1 (add1 (identity 5))) = 7
        assert_eq!(
            eval_str_with_call_native("(add1 (add1 (identity 5)))", &globals, &ctx).unwrap(),
            Value::int(7)
        );
    }

    #[test]
    fn test_call_native_in_if() {
        let globals = make_call_native_env();
        let ctx = EvalContext::new();
        assert_eq!(
            eval_str_with_call_native("(if #t (add1 10) (add1 20))", &globals, &ctx).unwrap(),
            Value::int(11)
        );
        assert_eq!(
            eval_str_with_call_native("(if #f (add1 10) (add1 20))", &globals, &ctx).unwrap(),
            Value::int(21)
        );
    }

    #[test]
    fn test_call_native_error_caught_by_try() {
        let globals = make_call_native_env();
        let ctx = EvalContext::new();
        let result =
            eval_str_with_call_native("(try (explode) (catch e \"caught\"))", &globals, &ctx)
                .unwrap();
        assert_eq!(result, Value::string("caught"));
    }

    #[test]
    fn test_call_native_error_message() {
        let globals = make_call_native_env();
        let ctx = EvalContext::new();
        let result =
            eval_str_with_call_native("(try (explode) (catch e (:message e)))", &globals, &ctx)
                .unwrap();
        assert_eq!(result, Value::string("boom"));
    }

    #[test]
    fn test_call_native_type_error() {
        let globals = make_call_native_env();
        let ctx = EvalContext::new();
        let result =
            eval_str_with_call_native("(try (type-explode) (catch e (:type e)))", &globals, &ctx)
                .unwrap();
        assert_eq!(result, Value::keyword("type-error"));
    }

    #[test]
    fn test_call_native_inside_closure() {
        let globals = make_call_native_env();
        let ctx = EvalContext::new();
        // Native call inside a user-defined function
        let result =
            eval_str_with_call_native("(define (inc x) (add1 x)) (inc 41)", &globals, &ctx)
                .unwrap();
        assert_eq!(result, Value::int(42));
    }

    #[test]
    fn test_call_native_shadowed_not_emitted() {
        let globals = make_call_native_env();
        let ctx = EvalContext::new();
        // Redefine identity to always return 999; the redefined version should be called
        let result =
            eval_str_with_call_native("(define (identity x) 999) (identity 1)", &globals, &ctx)
                .unwrap();
        assert_eq!(result, Value::int(999));
    }

    #[test]
    fn test_call_native_list_constructor() {
        let globals = make_call_native_env();
        let ctx = EvalContext::new();
        let result = eval_str_with_call_native("(list 1 2 3)", &globals, &ctx).unwrap();
        assert!(result.is_list());
        let items: Vec<Value> = result.as_list().unwrap().to_vec();
        assert_eq!(items, vec![Value::int(1), Value::int(2), Value::int(3)]);
    }

    #[test]
    fn test_call_native_unknown_native_at_vm_creation() {
        // If native_table references a name not in globals, VM::new should error
        let globals = Rc::new(Env::new()); // empty env
        let bogus_spur = intern("nonexistent-fn");
        match VM::new(globals, vec![], &[bogus_spur], 0) {
            Err(e) => assert!(
                e.to_string().contains("not found"),
                "expected 'not found' error, got: {e}"
            ),
            Ok(_) => panic!("expected error for unknown native"),
        }
    }

    #[test]
    fn test_call_native_non_native_value_at_vm_creation() {
        // If native_table references a name that's not a NativeFn, VM::new should error
        let globals = Rc::new(Env::new());
        let spur = intern("not-a-fn");
        globals.set(spur, Value::int(42)); // not a native fn
        match VM::new(globals, vec![], &[spur], 0) {
            Err(e) => assert!(
                e.to_string().contains("not a native function"),
                "expected 'not a native function' error, got: {e}"
            ),
            Ok(_) => panic!("expected error for non-native value"),
        }
    }

    #[test]
    fn test_call_native_matches_call_global_results() {
        // Verify CallNative and CallGlobal produce identical results for non-intrinsic natives.
        // Note: +, -, *, / are lowered as intrinsic opcodes, so they never go through
        // either CallGlobal or CallNative — we test functions that actually use these paths.
        let globals = make_call_native_env();
        let ctx = EvalContext::new();
        let expressions = &[
            "(identity 42)",
            "(add1 0)",
            "(add1 99)",
            "(not #f)",
            "(not #t)",
            "(list 1 2 3)",
            "(identity (add1 5))",
        ];
        for expr in expressions {
            // Via CallGlobal (no known_natives)
            let vals = sema_reader::read_many(expr).unwrap();
            let prog_global = compile_program(&vals, None).unwrap();
            let mut vm_global = VM::new(
                globals.clone(),
                prog_global.functions,
                &[],
                prog_global.main_cache_slots,
            )
            .unwrap();
            let via_global = vm_global.execute(prog_global.closure, &ctx).unwrap();

            // Via CallNative (with known_natives)
            let via_native = eval_str_with_call_native(expr, &globals, &ctx).unwrap();

            assert_eq!(
                via_global, via_native,
                "CallGlobal vs CallNative mismatch for: {expr}"
            );
        }
    }

    // ---- Stack overflow tests ----

    #[test]
    fn test_vm_stack_overflow_gives_clean_error() {
        // Non-tail recursion: (+ 1 (f)) prevents TCO, so frames grow unbounded
        let globals = make_test_env();
        let ctx = EvalContext::new();
        let result = eval_str("(define (f) (+ 1 (f))) (f)", &globals, &ctx);
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("stack overflow"),
            "expected stack overflow error, got: {err}"
        );
    }

    #[test]
    fn test_vm_stack_overflow_caught_by_try() {
        let globals = make_test_env();
        let ctx = EvalContext::new();
        let result = eval_str(
            "(define (f) (+ 1 (f))) (try (f) (catch e \"caught\"))",
            &globals,
            &ctx,
        )
        .unwrap();
        assert_eq!(result, Value::string("caught"));
    }

    #[test]
    fn test_vm_stack_overflow_mutual_recursion() {
        let globals = make_test_env();
        let ctx = EvalContext::new();
        let result = eval_str(
            "(define (a n) (b n)) (define (b n) (+ 1 (a n))) (try (a 0) (catch e \"caught\"))",
            &globals,
            &ctx,
        )
        .unwrap();
        assert_eq!(result, Value::string("caught"));
    }

    #[test]
    fn test_vm_deep_but_finite_recursion_ok() {
        // 1000 frames should be well within the 2048 limit
        let globals = make_test_env();
        let ctx = EvalContext::new();
        let result = eval_str(
            "(define (f n) (if (= n 0) 0 (+ 1 (f (- n 1))))) (f 1000)",
            &globals,
            &ctx,
        )
        .unwrap();
        assert_eq!(result, Value::int(1000));
    }

    // ---- Native call error + stack integrity tests ----

    #[test]
    fn test_call_native_error_preserves_stack_for_subsequent_ops() {
        // After a caught native error, subsequent operations should work correctly
        let globals = make_call_native_env();
        let ctx = EvalContext::new();
        let result =
            eval_str_with_call_native("(+ (try (explode) (catch e 10)) (add1 4))", &globals, &ctx)
                .unwrap();
        assert_eq!(result, Value::int(15));
    }

    #[test]
    fn test_call_native_multiple_errors_in_sequence() {
        let globals = make_call_native_env();
        let ctx = EvalContext::new();
        // Chain caught errors: (+ (+ caught1 caught2) caught3)
        let result = eval_str_with_call_native(
            "(+ (+ (try (explode) (catch e 1)) (try (explode) (catch e 2))) (try (type-explode) (catch e 3)))",
            &globals,
            &ctx,
        )
        .unwrap();
        assert_eq!(result, Value::int(6));
    }

    #[test]
    fn test_call_native_error_in_nested_call() {
        // Error from a native inside a user function, caught at outer level
        let globals = make_call_native_env();
        let ctx = EvalContext::new();
        let result = eval_str_with_call_native(
            "(define (f) (add1 (explode))) (try (f) (catch e \"caught\"))",
            &globals,
            &ctx,
        )
        .unwrap();
        assert_eq!(result, Value::string("caught"));
    }

    // ---- Named-let TCO edge cases ----

    #[test]
    fn test_named_let_in_non_tail_position() {
        // Named-let as argument to + (non-tail context).
        // The recursive call inside the lambda body should still get TCO.
        let globals = make_test_env();
        let ctx = EvalContext::new();
        let result = eval_str(
            "(+ 1 (let loop ((n 10000) (acc 0)) (if (= n 0) acc (loop (- n 1) (+ acc 1)))))",
            &globals,
            &ctx,
        )
        .unwrap();
        assert_eq!(result, Value::int(10001));
    }

    #[test]
    fn test_named_let_nested() {
        let globals = make_test_env();
        let ctx = EvalContext::new();
        let result = eval_str(
            "(let outer ((i 3) (total 0))
               (if (= i 0) total
                 (let inner ((j i) (sum 0))
                   (if (= j 0)
                     (outer (- i 1) (+ total sum))
                     (inner (- j 1) (+ sum j))))))",
            &globals,
            &ctx,
        )
        .unwrap();
        // i=3: inner sums 3+2+1=6; i=2: inner sums 2+1=3; i=1: inner sums 1=1; total=10
        assert_eq!(result, Value::int(10));
    }

    #[test]
    fn test_named_let_zero_iterations() {
        let globals = make_test_env();
        let ctx = EvalContext::new();
        let result = eval_str(
            "(let loop ((n 0) (acc 42)) (if (= n 0) acc (loop (- n 1) acc)))",
            &globals,
            &ctx,
        )
        .unwrap();
        assert_eq!(result, Value::int(42));
    }

    // ---- Global cache tests ----

    #[test]
    fn test_global_cache_invalidation_on_redefine() {
        // Redefining a global should invalidate the cache
        let globals = make_test_env();
        let ctx = EvalContext::new();
        eval_str("(define x 1)", &globals, &ctx).unwrap();
        assert_eq!(eval_str("x", &globals, &ctx).unwrap(), Value::int(1));
        eval_str("(define x 2)", &globals, &ctx).unwrap();
        assert_eq!(eval_str("x", &globals, &ctx).unwrap(), Value::int(2));
    }

    #[test]
    fn test_global_cache_many_globals() {
        // Test with more globals than cache slots to exercise eviction
        let globals = make_test_env();
        let ctx = EvalContext::new();
        // Define 300 globals (more than 256 cache slots)
        let mut defs = String::new();
        for i in 0..300 {
            defs.push_str(&format!("(define g{i} {i}) "));
        }
        eval_str(&defs, &globals, &ctx).unwrap();
        // Read them all back — some will cache-miss due to eviction
        for i in 0..300 {
            let result = eval_str(&format!("g{i}"), &globals, &ctx).unwrap();
            assert_eq!(result, Value::int(i), "g{i} should be {i}");
        }
    }

    // ── as_local_set robustness (DAP-7) ─────────────────────────

    fn parse_one(src: &str) -> Value {
        sema_reader::read_many(src)
            .unwrap()
            .into_iter()
            .next()
            .unwrap()
    }

    #[test]
    fn test_as_local_set_matches_builtin_form() {
        let expr = parse_one("(set! x 1)");
        let (target, value) = VM::as_local_set(&expr).expect("should match a set! form");
        assert_eq!(sema_core::resolve(target), "x");
        assert_eq!(value, Value::int(1));
    }

    #[test]
    fn test_as_local_set_rejects_non_set_forms() {
        // Wrong head symbol — must not be treated as a write-back candidate.
        assert!(VM::as_local_set(&parse_one("(define x 1)")).is_none());
        assert!(VM::as_local_set(&parse_one("(+ x 1)")).is_none());
        // Wrong arity.
        assert!(VM::as_local_set(&parse_one("(set! x)")).is_none());
        assert!(VM::as_local_set(&parse_one("(set! x 1 2)")).is_none());
        // Non-symbol target (e.g. a place expression) — not a simple local set!.
        assert!(VM::as_local_set(&parse_one("(set! (car xs) 1)")).is_none());
        // Not a list at all.
        assert!(VM::as_local_set(&parse_one("x")).is_none());
        assert!(VM::as_local_set(&parse_one("42")).is_none());
    }
}
