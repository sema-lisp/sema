use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::{Rc, Weak};

use smallvec::SmallVec;

use sema_core::{
    bits_to_spur,
    error::{suggest_similar, veteran_hint, CallFrame as CoreCallFrame, StackTrace},
    number::SemaNumber,
    resolve as resolve_spur, Env, EvalContext, NativeFn, SemaError, Spur, Value, ValueViewRef,
    NAN_INT_SMALL_PATTERN, NAN_PAYLOAD_BITS, NAN_PAYLOAD_MASK, NAN_TAG_MASK, TAG_NATIVE_FN,
};

use crate::chunk::Function;
use crate::opcodes::op;
use crate::opcodes::Op;

const DEBUG_VALUE_REF_BASE: u64 = crate::debug::DEBUG_VALUE_REF_BASE;

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
            if let UpvalueState::Closed(v) = &*state {
                sink(sema_core::GcEdge::Value(v));
            }
            true
        }
        Err(_) => false,
    }
}

/// Sever a white upvalue cell: `Closed(v)` → `Closed(NIL)`, returning `v` so
/// the collector defers its drop until all severing has completed.
fn sever_upvalue_cell(ptr: sema_core::NodePtr) -> Option<Value> {
    // SAFETY: live `Rc<UpvalueCell>` data pointer — see trace_vm_closure_payload.
    let cell = unsafe { &*(ptr.raw() as *const UpvalueCell) };
    match cell.state.try_borrow_mut() {
        Ok(mut state) => match std::mem::replace(&mut *state, UpvalueState::Closed(Value::NIL)) {
            UpvalueState::Closed(v) => Some(v),
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

/// Extracted VM closure: the closure itself and the function table from its compilation context.
pub type VmClosureInfo = (Rc<Closure>, Rc<Vec<Rc<Function>>>);

/// Extract a VM closure from a Value, if it wraps a VmClosurePayload.
/// Returns the closure and the function table needed to create a task VM.
pub fn extract_vm_closure(val: &Value) -> Option<VmClosureInfo> {
    let nf = val.as_native_fn_ref()?;
    let payload = nf.payload.as_ref()?.downcast_ref::<VmClosurePayload>()?;
    Some((payload.closure.clone(), payload.functions.clone()))
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
    /// Per-instruction inline cache for global lookups:
    /// (spur_bits, env_version, decoded binding).
    /// spur_bits distinguishes globals sharing the same slot (cross-VM closures).
    inline_cache: Vec<(u32, u64, CachedGlobal)>,
    /// Resolved native function table: native_id → (NativeFn Rc, name).
    /// Populated at VM creation from the compiler's native_table + global env.
    native_fns: Vec<Rc<NativeFn>>,
    debug_values: HashMap<u64, Value>,
    next_debug_value_ref: u64,
    /// Frame-count floor at which the dispatch loop treats a RETURN as
    /// "finished". Normally 0 (run until the call stack is empty). Raised
    /// temporarily by `run_nested_closure` so a re-entrant in-VM HOF callback
    /// returns to its native caller without unwinding the parent's frames.
    frame_floor: usize,
    /// One-entry cache of the last home env this VM registered with the
    /// cycle collector (CORE-2): consecutive `make_closure`s share a home,
    /// so a pointer-equality hit skips the collector's seen-set probe. The
    /// `Weak` guards address reuse — a dead entry (strong count 0) never
    /// matches, even if a fresh env landed on the same address.
    gc_adopted_home: std::cell::RefCell<Weak<Env>>,
}

thread_local! {
    /// Stack of pointers to VMs that are currently executing a native call on
    /// this thread. When a stdlib higher-order function invokes a VM closure
    /// via `call_callback`, the closure's fallback consults this stack so it can
    /// run *inside* the live VM (keeping open upvalue cells connected to the
    /// parent stack) instead of spawning a fresh VM that loses `set!` mutations.
    ///
    /// SAFETY: each pointer is valid for as long as the corresponding native
    /// call is on the Rust call stack. A `CurrentVmGuard` pushes the pointer
    /// immediately before a synchronous native call and pops it immediately
    /// after, so the pointer is only observed while the owning VM is paused at
    /// that exact call site and is not otherwise touched.
    static CURRENT_VM: RefCell<Vec<*mut VM>> = const { RefCell::new(Vec::new()) };
}

/// RAII guard that registers a VM as the current re-entrant target for the
/// duration of a native call, then unregisters it on drop.
struct CurrentVmGuard;

impl CurrentVmGuard {
    fn enter(vm: &mut VM) -> Self {
        let ptr = vm as *mut VM;
        CURRENT_VM.with(|stack| stack.borrow_mut().push(ptr));
        CurrentVmGuard
    }
}

impl Drop for CurrentVmGuard {
    fn drop(&mut self) {
        CURRENT_VM.with(|stack| {
            stack.borrow_mut().pop();
        });
    }
}

/// Identity of the root ancestor of an env chain: the root's `bindings`
/// allocation. All envs of one interpreter — the global env, module envs, and
/// closure homes — chain up to the same root *bindings*, so this is a stable
/// "same interpreter universe" test that doesn't depend on which frame a VM
/// happens to be paused in. The `Env` struct itself is not a usable identity:
/// module envs parent to a fresh `Rc` *clone* of the root
/// (`create_module_env`), and per-form module VMs wrap further clones — all of
/// which share the root's `bindings` Rc.
fn env_root(env: &Rc<Env>) -> *const () {
    let mut cur = env;
    while let Some(parent) = &cur.parent {
        cur = parent;
    }
    Rc::as_ptr(&cur.bindings) as *const ()
}

/// Try to run `closure` on the live VM currently executing a native call on
/// this thread. Returns `Some(result)` if a compatible VM was found and the
/// closure was dispatched in-VM; `None` if no compatible VM is registered (the
/// caller should fall back to a fresh VM).
///
/// "Compatible" means the closure belongs to the same interpreter universe as
/// the VM (their env chains share a root). Per-frame state is NOT compared:
/// the run loop re-points `self.globals`/`self.functions` at every frame
/// activation from the frame's own closure (M1 + M4), so a same-interpreter
/// closure from any module runs correctly as a nested frame. Running nested is
/// not just an optimization — it keeps the closure graph's still-open upvalue
/// cells connected to the stack that owns them. A fresh-VM fallback only
/// snapshots the *dispatched* closure's cells; a closure it reaches
/// transitively (via captured data or module state) would dereference open
/// cells against the wrong stack — out-of-bounds at best, a silent wrong-slot
/// read/write at worst.
fn try_run_on_current_vm(
    closure: &Rc<Closure>,
    globals: &Rc<Env>,
    args: &[Value],
    ctx: &EvalContext,
) -> Option<Result<Value, SemaError>> {
    try_run_on_current_vm_args(closure, globals, CallArgs::Borrowed(args), ctx)
}

fn try_run_on_current_vm_args(
    closure: &Rc<Closure>,
    globals: &Rc<Env>,
    args: CallArgs,
    ctx: &EvalContext,
) -> Option<Result<Value, SemaError>> {
    let closure_root = env_root(globals);
    // Snapshot the innermost compatible VM pointer, then release the borrow
    // before re-entering the VM (the nested run may itself register a new
    // current VM).
    let vm_ptr = CURRENT_VM.with(|stack| {
        let stack = stack.borrow();
        stack.iter().rev().copied().find(|&ptr| {
            // SAFETY: see CURRENT_VM docs — the pointer is valid while the
            // native call that registered it is on the Rust stack, which is
            // strictly the case here (we are inside that native call).
            let vm = unsafe { &*ptr };
            env_root(&vm.globals) == closure_root
        })
    })?;
    // SAFETY: the owning VM is paused at the native call site that registered
    // this pointer and does not touch `self` until the call returns. Args live
    // in a buffer owned by the native caller (borrowed or handed over for
    // moving out — see `CallArgs`), so there is no outstanding borrow of the
    // VM's stack. The reference does not escape this call.
    let vm = unsafe { &mut *vm_ptr };
    Some(vm.run_nested_closure_args(closure.clone(), args, ctx))
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
/// Mirrors the borrowed fallback wrapper built in `make_closure`
/// decision-for-decision (async inline task → current-VM nested run → foreign
/// fresh VM). Returns `None` when `func` is not a VM closure; the caller then
/// falls back to the borrowed protocol.
pub fn call_closure_owned(
    func: &Value,
    ctx: &EvalContext,
    args: &mut [Value],
) -> Option<Result<Value, SemaError>> {
    let (closure, functions) = extract_vm_closure(func)?;
    // The top-level main closure never travels as a callback value; if it
    // somehow does (globals is None), let the generic borrowed path handle it.
    let globals = closure.globals.as_ref()?.clone();
    if sema_core::in_async_context() {
        // The task VM clones args while setting up regardless; ownership is
        // moot here. See the fallback wrapper for why yields need this route.
        return Some(crate::scheduler::run_closure_as_inline_task(
            ctx, closure, functions, &*args,
        ));
    }
    if let Some(result) = try_run_on_current_vm_args(&closure, &globals, CallArgs::Owned(args), ctx)
    {
        return Some(result);
    }
    // Foreign fresh VM: snapshot open upvalues against the owning VM (if any)
    // before running on a different stack.
    close_closure_upvalues_for_foreign_run(&closure);
    let mut vm = VM::new_with_rc_functions(globals, functions);
    if let Err(e) = vm.setup_for_call_args(closure, CallArgs::Owned(args)) {
        return Some(Err(e));
    }
    Some(vm.run(ctx))
}

/// The home globals env of the VM currently executing a native call on this
/// thread (the innermost `CURRENT_VM`), if any. A native invoked from a running
/// VM uses this to act on the *current* environment — e.g. a nested `import`/
/// `load` copies bindings into the executing module's env rather than a fixed
/// global env (M4 nested-module isolation).
///
/// SAFETY: the top `CURRENT_VM` pointer is valid while the native call that
/// registered it is on the Rust stack, which is exactly the case when this is
/// called from inside that native. The VM is paused at the call site.
pub fn current_vm_globals() -> Option<Rc<Env>> {
    CURRENT_VM.with(|stack| {
        stack
            .borrow()
            .last()
            .map(|&ptr| unsafe { &*ptr }.globals.clone())
    })
}

thread_local! {
    /// Stack of pointers to the `DebugState` of an active debug session on this
    /// thread. Set by `execute_debug` (and the cooperative WASM start) around the
    /// run loop, popped on exit. The async scheduler is reached through the
    /// `RUN_SCHEDULER_CALLBACK` fn-pointer seam (`async_signal.rs`), which cannot
    /// carry a borrowed `&mut DebugState`; it consults this thread-local instead so
    /// task steps run in debug mode and a mid-task breakpoint can stop/resume.
    ///
    /// SAFETY mirrors `CURRENT_VM`: each pointer is valid for as long as the
    /// `execute_debug` frame that pushed it is on the Rust call stack. While that
    /// frame is blocked inside a native call that re-enters the scheduler, the
    /// `&mut DebugState` it owns is DORMANT (not otherwise touched) — the scheduler
    /// reborrows it through this raw pointer for the duration of one task step and
    /// drops the borrow before returning, so no two live `&mut` ever alias.
    static ACTIVE_DEBUG: RefCell<Vec<*mut crate::debug::DebugState>> =
        const { RefCell::new(Vec::new()) };
}

thread_local! {
    /// The `StopInfo` of a breakpoint that fired inside an async task during a
    /// COOPERATIVE (headless) debug session. Set by the scheduler's
    /// `step_task_debug` when, instead of blocking in `handle_debug_stop`, it
    /// leaves the task paused and unwinds so the cooperative call can surface the
    /// stop to JS. Consumed by `start_cooperative`/`run_cooperative`, which
    /// translate it into `VmExecResult::Stopped(info)`. Cleared on resume.
    static COOP_TASK_STOP: RefCell<Option<crate::debug::StopInfo>> = const { RefCell::new(None) };

    /// Id of the async task currently paused at a cooperative breakpoint. Set
    /// alongside `COOP_TASK_STOP` so inspection requests between JS calls
    /// (`with_coop_paused_task_vm`) can relocate the paused task in the scheduler
    /// and read ITS frames/locals — the task's own per-task VM, not the main VM
    /// which is parked at the `await`. Cleared on resume.
    static COOP_PAUSED_TASK_ID: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

/// Record the location a task stopped at for a cooperative (headless) debug
/// session, plus the id of the paused task so its VM can be inspected while the
/// cooperative session is suspended in JS. Called by the scheduler.
pub fn set_coop_task_stop(task_id: u64, info: crate::debug::StopInfo) {
    COOP_TASK_STOP.with(|s| *s.borrow_mut() = Some(info));
    COOP_PAUSED_TASK_ID.with(|c| c.set(Some(task_id)));
}

/// Take the pending cooperative task-stop location, if any. Called by
/// `run_cooperative`/`start_cooperative` to surface the stop to JS.
pub fn take_coop_task_stop() -> Option<crate::debug::StopInfo> {
    COOP_TASK_STOP.with(|s| s.borrow_mut().take())
}

/// Id of the async task paused at a cooperative breakpoint, if any.
pub fn coop_paused_task_id() -> Option<u64> {
    COOP_PAUSED_TASK_ID.with(|c| c.get())
}

/// Clear the paused-task id once the cooperative session resumes (the task is
/// about to be re-driven, so inspecting it as "paused" is no longer meaningful).
pub fn clear_coop_paused_task_id() {
    COOP_PAUSED_TASK_ID.with(|c| c.set(None));
}

/// Surface a cooperative async-task stop to JS as `VmExecResult::Stopped`,
/// enforcing the invariant that the scheduler-driving native registered HOW to
/// resume (`set_debug_coop_resume`). Every cooperative task pause is triggered by
/// a scheduler-driving combinator (`async/await`/`all`/`race`/`run`/`timeout`),
/// each of which records a [`DebugCoopResume`] before yielding the main VM. If a
/// stop surfaces with no pending resume, a NEW combinator paused the scheduler
/// without recording one — `run_cooperative`'s resume path would then take the
/// non-resume branch and silently wedge (the paused task never re-drives, the
/// awaited value is never reconstructed). Fail loudly here instead so the gap is
/// caught the first time that combinator is debugged, not shipped.
fn surface_coop_task_stop(
    info: crate::debug::StopInfo,
) -> Result<crate::debug::VmExecResult, SemaError> {
    if !sema_core::debug_coop_resume_pending() {
        return Err(SemaError::eval(
            "internal: cooperative debug stop surfaced without a registered \
             DebugCoopResume — a scheduler-driving async combinator paused for a \
             breakpoint but did not call set_debug_coop_resume",
        ));
    }
    Ok(crate::debug::VmExecResult::Stopped(info))
}

/// Reconstruct the value a scheduler-driving native (await/all/timeout/race/run)
/// would have returned, now that its target promise(s) have settled — used by the
/// cooperative debug-resume path to resume the main VM after a task breakpoint.
/// A rejected/cancelled target surfaces as the same error the native would have
/// produced. Mirrors the success/error mapping in `sema-stdlib/src/async_ops.rs`.
fn reconstruct_coop_resume_value(how: &sema_core::DebugCoopResume) -> Result<Value, SemaError> {
    use sema_core::{DebugCoopResume, PromiseState};
    match how {
        DebugCoopResume::Run => Ok(Value::nil()),
        DebugCoopResume::Await(p) => match &*p.state.borrow() {
            PromiseState::Resolved(v) => Ok(v.clone()),
            PromiseState::Rejected(e) => {
                Err(SemaError::eval(format!("async/await: task rejected: {e}")))
            }
            PromiseState::Cancelled => Err(SemaError::eval("async/await: task was cancelled")),
            PromiseState::Pending => Err(SemaError::eval(
                "async/await: still pending after scheduler run",
            )),
        },
        DebugCoopResume::All(promises) => {
            let mut results = Vec::with_capacity(promises.len());
            for p in promises {
                match &*p.state.borrow() {
                    PromiseState::Resolved(v) => results.push(v.clone()),
                    PromiseState::Rejected(e) => {
                        return Err(SemaError::eval(format!("async/all: task rejected: {e}")))
                    }
                    PromiseState::Cancelled => {
                        return Err(SemaError::eval("async/all: task was cancelled"))
                    }
                    PromiseState::Pending => {
                        return Err(SemaError::eval("async/all: task still pending"))
                    }
                }
            }
            Ok(Value::list(results))
        }
        DebugCoopResume::Race(promises) => {
            for p in promises {
                if let PromiseState::Resolved(v) = &*p.state.borrow() {
                    return Ok(v.clone());
                }
            }
            for p in promises {
                if let PromiseState::Rejected(e) = &*p.state.borrow() {
                    return Err(SemaError::eval(format!("async/race: task rejected: {e}")));
                }
            }
            Err(SemaError::eval("async/race: no promise resolved"))
        }
    }
}

/// RAII guard registering a `DebugState` as the active debug session for the
/// duration of a debug run, unregistering it on drop (including panic unwind).
struct ActiveDebugGuard;

impl ActiveDebugGuard {
    fn enter(debug: &mut crate::debug::DebugState) -> Self {
        let ptr = debug as *mut crate::debug::DebugState;
        ACTIVE_DEBUG.with(|stack| stack.borrow_mut().push(ptr));
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
    let ptr = ACTIVE_DEBUG.with(|stack| stack.borrow().last().copied())?;
    // SAFETY: as documented above.
    let debug = unsafe { &mut *ptr };
    Some(f(debug))
}

/// Close `closure`'s still-open upvalue cells against the VM(s) currently
/// running a native call on this thread, snapshotting their values from the
/// owning VM's live stack.
///
/// MUST be called before a VM closure is dispatched onto a *foreign* stack — a
/// fresh fallback VM, or an async task VM created by `spawn` /
/// `run_closure_as_inline_task`. An Open cell holds `{ frame_base, slot }`
/// indices into the VM that created it; reading it on a different VM's stack is
/// out-of-bounds. Snapshotting here (while the owning VM is paused at its native
/// call) detaches the cells safely.
///
/// A no-op for cells that are already Closed or whose owning frame is no longer
/// on any registered VM's stack.
pub fn close_closure_upvalues_for_foreign_run(closure: &Closure) {
    // Snapshot the registered VM pointers, then operate through them. The
    // pointers are valid for the duration of this call (see CURRENT_VM docs).
    let ptrs: Vec<*mut VM> = CURRENT_VM.with(|stack| stack.borrow().clone());
    for cell in &closure.upvalues {
        let (frame_base, slot) = {
            let state = cell.state.borrow();
            match &*state {
                UpvalueState::Open { frame_base, slot } => (*frame_base, *slot),
                UpvalueState::Closed(_) => continue,
            }
        };
        // Find the registered VM that owns this cell (its frame is on that
        // VM's stack). Walk most-recent first.
        for &ptr in ptrs.iter().rev() {
            // SAFETY: pointer registered by a live CurrentVmGuard on the Rust
            // stack; the owning VM is paused and not otherwise borrowed.
            let vm = unsafe { &mut *ptr };
            if frame_base + slot < vm.stack.len() && vm.frames.iter().any(|f| f.base == frame_base)
            {
                let value = vm.stack[frame_base + slot].clone();
                *cell.state.borrow_mut() = UpvalueState::Closed(value);
                if let Some(frame) = vm.frames.iter_mut().find(|f| f.base == frame_base) {
                    if let Some(open) = frame.open_upvalues.as_mut() {
                        if let Some(entry) = open.get_mut(slot) {
                            *entry = None;
                        }
                    }
                }
                break;
            }
        }
    }
}

/// Error for dereferencing an Open upvalue cell whose stack slot is not on the
/// executing VM's stack — a closure with open upvalues escaped its owning VM
/// without being snapshotted (see `close_closure_upvalues_for_foreign_run`).
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
fn close_open_upvalues(open: &mut [Option<Rc<UpvalueCell>>], stack: &[Value], base: usize) {
    for (slot, maybe_cell) in open.iter_mut().enumerate() {
        if let Some(cell) = maybe_cell {
            let mut state = cell.state.borrow_mut();
            if matches!(&*state, UpvalueState::Open { .. }) {
                *state = UpvalueState::Closed(stack[base + slot].clone());
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
                if matches!(&*state, UpvalueState::Open { .. }) {
                    *state = UpvalueState::Closed(stack[base + slot].clone());
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
        Ok(VM {
            stack: Vec::with_capacity(256),
            frames: Vec::with_capacity(64),
            globals,
            functions: Rc::new(functions),
            inline_cache: vec![(u32::MAX, 0, CachedGlobal::Plain(Value::nil())); total_cache_slots],
            native_fns,
            debug_values: HashMap::new(),
            next_debug_value_ref: DEBUG_VALUE_REF_BASE,
            frame_floor: 0,
            gc_adopted_home: std::cell::RefCell::new(Weak::new()),
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

    fn new_with_rc_functions(globals: Rc<Env>, functions: Rc<Vec<Rc<Function>>>) -> Self {
        let total_cache_slots: usize = functions
            .iter()
            .map(|f| f.chunk.n_global_cache_slots as usize)
            .sum();
        ensure_cycle_gc_wired();
        VM {
            stack: Vec::with_capacity(256),
            frames: Vec::with_capacity(64),
            globals,
            functions,
            inline_cache: vec![(u32::MAX, 0, CachedGlobal::Plain(Value::nil())); total_cache_slots],
            native_fns: Vec::new(),
            debug_values: HashMap::new(),
            next_debug_value_ref: DEBUG_VALUE_REF_BASE,
            frame_floor: 0,
            gc_adopted_home: std::cell::RefCell::new(Weak::new()),
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
            functions,
            inline_cache: vec![(u32::MAX, 0, CachedGlobal::Plain(Value::nil())); total_cache_slots],
            native_fns,
            debug_values: HashMap::new(),
            next_debug_value_ref: DEBUG_VALUE_REF_BASE,
            frame_floor: 0,
            gc_adopted_home: std::cell::RefCell::new(Weak::new()),
        })
    }

    pub fn execute(&mut self, closure: Rc<Closure>, ctx: &EvalContext) -> Result<Value, SemaError> {
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
                crate::debug::VmExecResult::AsyncYield(_) => {
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
                    let result = match crate::debug::decode_scope_ref(variables_reference) {
                        Some(crate::debug::ScopeKind::Locals(frame_id))
                        | Some(crate::debug::ScopeKind::Upvalues(frame_id)) => {
                            sema_reader::read(&value_expression)
                                .map_err(|e| e.to_string())
                                .and_then(|expr| {
                                    self.debug_evaluate(frame_id, &expr, ctx, debug)
                                        .map_err(|e| e.to_string())
                                })
                                .and_then(|value| {
                                    self.debug_set_variable(variables_reference, &name, value)
                                        .map_err(|e| e.to_string())
                                })
                        }
                        None => Err("setVariable: invalid variablesReference".to_string()),
                    };
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
    fn debug_exception_park(&mut self, ctx: &EvalContext, debug: &mut crate::debug::DebugState) {
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

    /// Run the VM cooperatively: execute until completion or a debug stop.
    /// The caller is responsible for managing debug state between calls.
    pub fn run_cooperative(
        &mut self,
        ctx: &EvalContext,
        debug: &mut crate::debug::DebugState,
    ) -> Result<crate::debug::VmExecResult, SemaError> {
        // Resume a cooperative debug pause that occurred inside an async task: a
        // scheduler-driving native (await/all/timeout/race/run) yielded the main
        // VM for a task breakpoint. Before resuming the main VM, re-drive the
        // scheduler so the paused task continues; if it pauses again surface the
        // new stop, otherwise reconstruct the native's value and resume the main
        // VM via the stack-top placeholder it left (`set_resume_value` semantics).
        if let Some((target, how)) = sema_core::take_debug_coop_resume() {
            // The paused task is about to be re-driven; inspecting it as "paused"
            // is no longer meaningful (a later stop re-records a fresh id).
            clear_coop_paused_task_id();
            // The scheduler runs in debug mode for this re-drive too, so a later
            // breakpoint in the same or a sibling task stops as well.
            let _active = ActiveDebugGuard::enter(debug);
            match sema_core::call_run_scheduler_target(ctx, target.clone()) {
                Ok(sema_core::SchedulerRunResult::DebugPaused) => {
                    // Paused again on another breakpoint. Re-arm the resume so the
                    // NEXT call drives the scheduler once more, and surface the new
                    // stop. (The native already consumed its yield; we re-store the
                    // coop resume here since we took it above.)
                    sema_core::set_debug_coop_resume(target, how);
                    if let Some(info) = take_coop_task_stop() {
                        // Resume was just re-armed above, so the guard's invariant
                        // holds by construction here.
                        return surface_coop_task_stop(info);
                    }
                    // Shouldn't happen, but don't wedge: fall through to a yield.
                    return Ok(crate::debug::VmExecResult::Yielded);
                }
                Ok(_) => {
                    // Target settled. Reconstruct the value the yielded native
                    // (await/all/timeout/race/run) would have returned, put it on
                    // the main VM's stack top (the native left a nil placeholder
                    // there), and resume the main VM from after that native call.
                    // A rejected/cancelled target surfaces as an error here — the
                    // same error the native would have produced had it not paused.
                    let resume_value = reconstruct_coop_resume_value(&how)?;
                    self.replace_stack_top(resume_value);
                    return self.run_inner::<true>(ctx, Some(debug));
                }
                Err(e) => return Err(e),
            }
        }
        // Normal cooperative step (no pending debug-pause resume): run with the
        // session registered so a task breakpoint hit during this step (e.g. the
        // main VM reaches an await whose task then breaks) surfaces as a stop.
        let _active = ActiveDebugGuard::enter(debug);
        let result = self.run_inner::<true>(ctx, Some(debug))?;
        if let Some(info) = take_coop_task_stop() {
            return surface_coop_task_stop(info);
        }
        Ok(result)
    }

    /// Start cooperative debug execution: push the initial frame and run.
    pub fn start_cooperative(
        &mut self,
        closure: Rc<Closure>,
        ctx: &EvalContext,
        debug: &mut crate::debug::DebugState,
    ) -> Result<crate::debug::VmExecResult, SemaError> {
        // Session-boundary hygiene: a fresh cooperative session must not inherit
        // ANY cooperative-debug state from a prior session that was abandoned
        // (Stop) while paused at an async breakpoint. Without this, the next
        // session's first Continue would consume a stale `DEBUG_COOP_RESUME` and
        // re-drive a dead target — clobbering this program's VM stack — or surface
        // a stale `COOP_TASK_STOP`, or inspect a leftover task by a reused id, or
        // silently run an abandoned `Ready` task left in the reused scheduler.
        // The normal resume path clears these thread-locals; this covers the
        // Stop-while-paused path, which never resumes.
        clear_coop_paused_task_id();
        let _ = take_coop_task_stop();
        let _ = sema_core::take_debug_coop_resume();
        crate::scheduler::reset_scheduler_tasks();
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
        // (reached via the RUN_SCHEDULER_CALLBACK seam during a native call) runs
        // task steps in debug mode; a mid-task breakpoint then surfaces as a
        // cooperative stop (see `step_task_debug`). The guard drops when control
        // returns to JS — correct, since no scheduler runs between JS calls.
        let _active = ActiveDebugGuard::enter(debug);
        let result = self.run_inner::<true>(ctx, Some(debug))?;
        // If a task breakpoint fired during this drive, the scheduler-driving
        // native yielded the main VM (AsyncYield) and recorded the stop; surface
        // it to JS as a cooperative Stop instead of the raw AsyncYield.
        if let Some(info) = take_coop_task_stop() {
            return surface_coop_task_stop(info);
        }
        Ok(result)
    }

    /// Number of active call frames.
    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    fn run(&mut self, ctx: &EvalContext) -> Result<Value, SemaError> {
        match self.run_inner::<false>(ctx, None)? {
            crate::debug::VmExecResult::Finished(v) => Ok(v),
            crate::debug::VmExecResult::Stopped(_) | crate::debug::VmExecResult::Yielded => {
                unreachable!("Stopped/Yielded without debug state")
            }
            crate::debug::VmExecResult::AsyncYield(_) => Err(SemaError::eval(
                "async yield outside of scheduler context".to_string(),
            )),
        }
    }

    /// Run `closure` with `args` as a nested frame on this *live* VM and return
    /// its result, leaving the parent frames and stack intact.
    ///
    /// This is the in-VM routing path for stdlib higher-order callbacks (C1).
    /// Because the closure executes on the same VM, its open upvalue cells stay
    /// connected to the parent frame's live stack slots, so `set!` inside the
    /// callback propagates back to the caller — fixing the divergence where the
    /// fresh-VM fallback mutated a detached closed snapshot.
    ///
    /// The dispatch loop is bounded by `frame_floor`: it returns as soon as the
    /// frame it pushed (and any frames pushed beneath it) have returned, without
    /// unwinding the caller's frames.
    fn run_nested_closure_args(
        &mut self,
        closure: Rc<Closure>,
        args: CallArgs,
        ctx: &EvalContext,
    ) -> Result<Value, SemaError> {
        // Floor = the parent's current frame depth. After setup_for_call pushes
        // the callee frame, the loop must stop unwinding once it pops back to
        // this depth (rather than emptying the whole call stack).
        let floor = self.frames.len();
        let stack_floor = self.stack.len();
        // The nested run's frame activations re-point `self.globals` /
        // `self.functions` (M1 + M4) and nothing re-activates the paused
        // caller's frame until it resumes. Save/restore them so the VM's live
        // state stays coherent with the paused frame while the native call is
        // still in progress (e.g. `current_vm_globals()` for nested imports).
        let saved_globals = self.globals.clone();
        let saved_functions = self.functions.clone();
        self.setup_for_call_args(closure, args)?;
        let saved_floor = self.frame_floor;
        self.frame_floor = floor;
        // Each native→VM re-entry nests a fresh dispatch loop on the *Rust*
        // stack; grow it on demand so deep re-entrant recursion hits the VM's
        // catchable frame guard instead of overflowing the OS stack (SIGABRT).
        let result = sema_core::stack::maybe_grow(|| self.run_inner::<false>(ctx, None));
        self.frame_floor = saved_floor;
        self.globals = saved_globals;
        self.functions = saved_functions;
        match result {
            Ok(crate::debug::VmExecResult::Finished(v)) => Ok(v),
            Ok(crate::debug::VmExecResult::Stopped(_))
            | Ok(crate::debug::VmExecResult::Yielded) => {
                unreachable!("Stopped/Yielded without debug state")
            }
            Ok(crate::debug::VmExecResult::AsyncYield(_)) => {
                // Re-entrant HOF callbacks are synchronous; a yield here cannot
                // be resumed. Roll back the partial nested frames/stack so the
                // parent VM stays consistent, then surface the same error the
                // fresh-VM fallback would have produced.
                self.frames.truncate(floor);
                self.stack.truncate(stack_floor);
                Err(SemaError::eval(
                    "async yield outside of scheduler context".to_string(),
                ))
            }
            Err(e) => {
                // The error propagated past every handler in the nested frames
                // without being caught. run_inner leaves those frames in place
                // on error, so unwind them back to the parent's depth before
                // returning so the parent VM can handle/propagate cleanly.
                self.frames.truncate(floor);
                self.stack.truncate(stack_floor);
                Err(e)
            }
        }
    }

    /// Run the VM without debug state, returning the raw VmExecResult.
    /// Used by the async scheduler for task execution and resume.
    pub fn run_async(
        &mut self,
        ctx: &EvalContext,
    ) -> Result<crate::debug::VmExecResult, SemaError> {
        self.run_inner::<false>(ctx, None)
    }

    /// Debug-aware resume of an async task step: like [`run_async`] but with the
    /// breakpoint/step machinery live (`run_inner::<true>`). The async
    /// scheduler uses this for parked-task resumes when a debug session is active,
    /// so a breakpoint on a line that runs only inside the task can stop. A
    /// returned `Stopped` is handled by the scheduler via [`handle_debug_stop`].
    pub fn run_async_debug(
        &mut self,
        ctx: &EvalContext,
        debug: &mut crate::debug::DebugState,
    ) -> Result<crate::debug::VmExecResult, SemaError> {
        self.run_inner::<true>(ctx, Some(debug))
    }

    /// Replace the top of the stack with a value.
    /// Used by the scheduler to set the resume value before continuing
    /// a yielded task (the yield left a nil placeholder on the stack).
    pub fn replace_stack_top(&mut self, val: Value) {
        if let Some(top) = self.stack.last_mut() {
            *top = val;
        }
    }

    /// Execute a closure and return the raw VmExecResult (for async scheduler).
    pub fn execute_async(
        &mut self,
        closure: Rc<Closure>,
        ctx: &EvalContext,
    ) -> Result<crate::debug::VmExecResult, SemaError> {
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
        self.run_inner::<false>(ctx, None)
    }

    /// Debug-aware first step of an async task: like [`execute_async`] but with the
    /// breakpoint/step machinery live. Used by the scheduler to start a task when a
    /// debug session is active.
    pub fn execute_async_debug(
        &mut self,
        closure: Rc<Closure>,
        ctx: &EvalContext,
        debug: &mut crate::debug::DebugState,
    ) -> Result<crate::debug::VmExecResult, SemaError> {
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
        self.run_inner::<true>(ctx, Some(debug))
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
        let base_functions = self.functions.clone();
        // Snapshot the VM's base globals — the env the top-level main closure
        // (which carries no explicit home env) resolves against. `self.globals`
        // is kept pointing at the running frame's home env (below); this
        // immutable snapshot is the fallback for `None` closures (M1).
        let base_globals = self.globals.clone();

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
                        self.stack[base + slot] = val;
                    }

                    // --- Upvalues ---
                    op::LOAD_UPVALUE => {
                        let idx = read_u16!(code, pc) as usize;
                        let resolved = {
                            let state = self.frames[fi].closure.upvalues[idx].state.borrow();
                            match &*state {
                                UpvalueState::Closed(v) => Ok(v.clone()),
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
                            // Drain any stale yield signal set before the error
                            drop(sema_core::take_yield_signal());
                            match self.handle_exception(err, saved_pc)? {
                                ExceptionAction::Handled => {}
                                ExceptionAction::Propagate(e) => return Err(e),
                            }
                        }
                        // Check if a native function (dispatched via call_value)
                        // signaled an async yield
                        if let Some(reason) = sema_core::take_yield_signal() {
                            if let Some(top) = self.stack.last_mut() {
                                *top = Value::nil();
                            }
                            self.frames[fi].pc = pc;
                            return Ok(crate::debug::VmExecResult::AsyncYield(reason));
                        }
                        continue 'dispatch;
                    }
                    op::TAIL_CALL => {
                        let argc = read_u16!(code, pc) as usize;
                        self.frames[fi].pc = pc;
                        let saved_pc = pc - op::SIZE_OP_U16;
                        if let Err(err) = self.tail_call_value(argc, ctx) {
                            // Drain any stale yield signal set before the error
                            drop(sema_core::take_yield_signal());
                            match self.handle_exception(err, saved_pc)? {
                                ExceptionAction::Handled => {}
                                ExceptionAction::Propagate(e) => return Err(e),
                            }
                        }
                        // Check if a native function (dispatched via tail_call_value
                        // → call_value) signaled an async yield
                        if let Some(reason) = sema_core::take_yield_signal() {
                            if let Some(top) = self.stack.last_mut() {
                                *top = Value::nil();
                            }
                            self.frames[fi].pc = pc;
                            return Ok(crate::debug::VmExecResult::AsyncYield(reason));
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
                        if self.frames.len() == self.frame_floor {
                            // Either the top-level program finished (floor == 0)
                            // or a re-entrant nested closure returned to its
                            // native caller (floor raised by run_nested_closure).
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

                        // C1: keep open upvalues open across the call so a
                        // re-entrant in-VM HOF callback (routed via
                        // try_run_on_current_vm) can write `set!` back through
                        // them. Closures that instead cross onto a foreign stack
                        // (fresh fallback VM / async task VM) are snapshotted at
                        // that crossing point (see close_closure_upvalues_for_foreign_run).
                        let native = self.native_fns[native_id].clone();
                        let args_start = self.stack.len() - argc;
                        // Move args into an owned buffer and drop them from the
                        // stack before the call so no borrow of self.stack is held
                        // while the native may re-enter this VM (run_nested_closure
                        // needs &mut self via the CURRENT_VM pointer). SmallVec
                        // keeps argc <= 8 off the heap; drain moves without
                        // refcount traffic.
                        let call_args: SmallVec<[Value; 8]> =
                            self.stack.drain(args_start..).collect();
                        let result = {
                            let _vm_guard = CurrentVmGuard::enter(self);
                            (native.func)(ctx, &call_args)
                        };
                        match result {
                            Ok(val) => {
                                // Check if the native function signaled an async yield
                                if let Some(reason) = sema_core::take_yield_signal() {
                                    // Args are already truncated. Push nil as a placeholder
                                    // for the call result slot. On resume, the scheduler will
                                    // pop this and push the actual resume value before
                                    // continuing execution.
                                    self.stack.push(Value::nil());
                                    self.frames[fi].pc = pc; // PC already past CALL_NATIVE
                                    return Ok(crate::debug::VmExecResult::AsyncYield(reason));
                                }
                                self.stack.push(val);
                            }
                            Err(err) => {
                                // Drain any stale yield signal set before the error
                                drop(sema_core::take_yield_signal());
                                handle_err!(self, fi, pc, err, saved_pc, 'dispatch);
                            }
                        }
                        // The native completed on this frame (a re-entrant
                        // callback ran nested — run_nested_closure floors at
                        // this frame and restores globals/functions), so stay
                        // in the inner loop instead of re-entering 'dispatch.
                        // The length check guards the invariant; DEBUG
                        // re-enters so stepping/breakpoint semantics are
                        // unchanged.
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
                                        if let Some(reason) = sema_core::take_yield_signal() {
                                            // The call already pushed a result value.
                                            // Replace it with a nil placeholder; on
                                            // resume the scheduler substitutes the
                                            // actual resume value.
                                            if let Some(top) = self.stack.last_mut() {
                                                *top = Value::nil();
                                            }
                                            self.frames[fi].pc = pc; // PC already past CALL_GLOBAL
                                            return Ok(crate::debug::VmExecResult::AsyncYield(
                                                reason,
                                            ));
                                        }
                                        // The native completed on this frame (a
                                        // re-entrant callback ran nested —
                                        // run_nested_closure floors at this frame
                                        // and restores globals/functions), so stay
                                        // in the inner loop instead of re-entering
                                        // 'dispatch. The length check guards the
                                        // invariant; DEBUG re-enters so stepping
                                        // and breakpoint semantics are unchanged.
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
                                        drop(sema_core::take_yield_signal());
                                        handle_err!(self, fi, pc, err, saved_pc, 'dispatch);
                                    }
                                }
                            }
                            // Slow path: non-native callable — use call_value_with
                            CachedGlobal::Plain(value) => {
                                let func_val = value.clone();
                                if let Err(err) = self.call_value_with(func_val, argc, ctx) {
                                    drop(sema_core::take_yield_signal());
                                    match self.handle_exception(err, saved_pc)? {
                                        ExceptionAction::Handled => {}
                                        ExceptionAction::Propagate(e) => return Err(e),
                                    }
                                }
                                // Check if the native signaled async yield
                                if let Some(reason) = sema_core::take_yield_signal() {
                                    // The call already pushed a result value. Replace it
                                    // with nil placeholder. On resume, the scheduler replaces
                                    // this with the actual resume value.
                                    if let Some(top) = self.stack.last_mut() {
                                        *top = Value::nil();
                                    }
                                    self.frames[fi].pc = pc; // PC already past CALL_GLOBAL
                                    return Ok(crate::debug::VmExecResult::AsyncYield(reason));
                                }
                                continue 'dispatch;
                            }
                        }
                    }

                    op::STORE_LOCAL0 => {
                        let val = unsafe { pop_unchecked(&mut self.stack) };
                        self.stack[base] = val;
                    }
                    op::STORE_LOCAL1 => {
                        let val = unsafe { pop_unchecked(&mut self.stack) };
                        self.stack[base + 1] = val;
                    }
                    op::STORE_LOCAL2 => {
                        let val = unsafe { pop_unchecked(&mut self.stack) };
                        self.stack[base + 2] = val;
                    }
                    op::STORE_LOCAL3 => {
                        let val = unsafe { pop_unchecked(&mut self.stack) };
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
                        } else {
                            let err = SemaError::type_error(
                                "list, vector, string, map, or hashmap",
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
            // C1: keep open upvalues open across the call so a re-entrant
            // in-VM HOF callback can write back through them. Move args into an
            // owned buffer (releasing the stack borrow) so the native may
            // re-enter this VM via run_nested_closure. Closures crossing onto a
            // foreign stack are snapshotted at the crossing point.
            let func_rc = self.stack[func_idx].as_native_fn_rc().unwrap();
            let call_args: SmallVec<[Value; 8]> = self.stack.drain(func_idx + 1..).collect();
            self.stack.pop(); // pop the native fn value
            let result = {
                let _vm_guard = CurrentVmGuard::enter(self);
                (func_rc.func)(ctx, &call_args)
            };
            result.map(|val| self.stack.push(val))?;
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
            // C1: keep upvalues open across the callback. The callback may
            // re-enter this VM (e.g. a multimethod whose handler is a VM
            // closure). Move args into an owned buffer so no stack borrow is
            // held during the (possibly re-entrant) call. Closures crossing
            // onto a foreign stack are snapshotted at the crossing point.
            let func_val = self.stack[func_idx].clone();
            let call_args: SmallVec<[Value; 8]> = self.stack.drain(func_idx + 1..).collect();
            self.stack.pop(); // pop the callable value
            let result = {
                let _vm_guard = CurrentVmGuard::enter(self);
                sema_core::call_callback(ctx, &func_val, &call_args)
            };
            let result = result?;
            self.stack.push(result);
            Ok(())
        }
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
            // C1: keep upvalues open; move args off the stack so the native may
            // re-enter this VM via run_nested_closure without an outstanding
            // stack borrow. Closures crossing onto a foreign stack are
            // snapshotted there.
            let func_rc = func_val.as_native_fn_rc().unwrap();
            let args_start = self.stack.len() - argc;
            let call_args: SmallVec<[Value; 8]> = self.stack.drain(args_start..).collect();
            let result = {
                let _vm_guard = CurrentVmGuard::enter(self);
                (func_rc.func)(ctx, &call_args)
            };
            result.map(|val| self.stack.push(val))?;
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
            let result = {
                let _vm_guard = CurrentVmGuard::enter(self);
                sema_core::call_callback(ctx, &func_val, &call_args)
            };
            let result = result?;
            self.stack.push(result);
            Ok(())
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
        // C1: keep upvalues open; move args off the stack so the native may
        // re-enter this VM via run_nested_closure without an outstanding
        // stack borrow. Closures crossing onto a foreign stack are
        // snapshotted at the crossing point.
        let args_start = self.stack.len() - argc;
        let call_args: SmallVec<[Value; 8]> = self.stack.drain(args_start..).collect();
        let result = {
            let _vm_guard = CurrentVmGuard::enter(self);
            (func.func)(ctx, &call_args)
        };
        result.map(|val| self.stack.push(val))?;
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
                let closure = &payload_for_box.closure;
                let functions = &payload_for_box.functions;
                let globals = closure
                    .globals
                    .as_ref()
                    .expect("MakeClosure closures always carry Some(home)");

                // Inside an async task, route through the scheduler so any
                // yield in the inner closure (channel/send, channel/recv,
                // await, sleep) suspends cleanly. Otherwise the inner VM's
                // AsyncYield would surface as "async yield outside of
                // scheduler context" and crash the calling HOF.
                if sema_core::in_async_context() {
                    // In async context the VM call sites close this closure's
                    // open upvalues before invoking the HOF (see call_value /
                    // CALL_NATIVE), so the cells are already snapshotted and safe
                    // to run on the fresh task VM stack.
                    return crate::scheduler::run_closure_as_inline_task(
                        ctx,
                        closure.clone(),
                        functions.clone(),
                        args,
                    );
                }

                // C1: if this closure belongs to a VM currently running a native
                // call on this thread (e.g. a stdlib HOF like map/filter/foldl
                // invoked us), run it as a nested frame on that *live* VM. This
                // keeps the closure's open upvalue cells connected to the
                // parent's stack slots so `set!` mutations flow back to the
                // caller. Falls back to a fresh VM only when no compatible VM is
                // on the stack.
                if let Some(result) = try_run_on_current_vm(closure, globals, args, ctx) {
                    return result;
                }

                // Foreign fresh VM: snapshot open upvalues against the owning
                // VM (if any) before running on a different stack.
                close_closure_upvalues_for_foreign_run(closure);
                let mut vm = VM::new_with_rc_functions(globals.clone(), functions.clone());
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
        // Walk frames from top looking for a handler. Stop at `frame_floor`:
        // during a re-entrant nested run (run_nested_closure) the frames below
        // the floor belong to the parent VM execution, which must handle or
        // propagate the error itself once control returns to it.
        while self.frames.len() > self.frame_floor {
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
        &self,
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
        &self,
        frame_id: usize,
        expr: &Value,
        ctx: &EvalContext,
        _debug: &crate::debug::DebugState,
    ) -> Result<Value, SemaError> {
        let env = self.debug_env_for_frame(frame_id)?;
        match sema_core::eval_callback(ctx, expr, &env) {
            Ok(value) => Ok(value),
            Err(_) if ctx.eval_fn.get().is_none() => {
                let prog = compile_program(std::slice::from_ref(expr), None)?;
                let mut vm = VM::new(
                    env,
                    prog.functions,
                    &prog.native_table,
                    prog.main_cache_slots,
                )?;
                vm.execute(prog.closure, ctx)
            }
            Err(err) => Err(err),
        }
    }

    pub fn debug_set_variable(
        &mut self,
        variables_reference: u64,
        name: &str,
        value: Value,
    ) -> Result<crate::debug::DapVariable, SemaError> {
        match crate::debug::decode_scope_ref(variables_reference) {
            Some(crate::debug::ScopeKind::Locals(frame_id)) => {
                self.debug_set_local(frame_id, name, value)
            }
            Some(crate::debug::ScopeKind::Upvalues(frame_id)) => {
                self.debug_set_upvalue(frame_id, name, value)
            }
            None => Err(SemaError::eval(
                "setVariable: invalid variablesReference".to_string(),
            )),
        }
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
            UpvalueState::Open { frame_base, slot } => self
                .stack
                .get(*frame_base + *slot)
                .cloned()
                .unwrap_or_else(Value::nil),
        }
    }

    fn debug_set_local(
        &mut self,
        frame_id: usize,
        name: &str,
        value: Value,
    ) -> Result<crate::debug::DapVariable, SemaError> {
        let Some(slot) = self.in_scope_local_slot(frame_id, name) else {
            return Err(SemaError::eval(format!(
                "setVariable: local '{name}' not found"
            )));
        };
        self.debug_set_local_slot(frame_id, slot, name, value)
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

        {
            let mut state = upvalue.state.borrow_mut();
            match &mut *state {
                UpvalueState::Closed(slot_value) => {
                    *slot_value = value.clone();
                }
                UpvalueState::Open { frame_base, slot } => {
                    let Some(slot_value) = self.stack.get_mut(*frame_base + *slot) else {
                        return Err(SemaError::eval(format!(
                            "setVariable: upvalue '{name}' is out of range"
                        )));
                    };
                    *slot_value = value.clone();
                }
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::{Chunk, Function};
    use sema_core::{intern, NativeFn};

    /// The cooperative-stop guard: surfacing a task stop with NO pending
    /// `DebugCoopResume` is an internal error (a scheduler-driving combinator
    /// forgot `set_debug_coop_resume`), while a stop WITH one surfaces normally.
    /// This pins the invariant so a future combinator that paginates the
    /// scheduler without registering a resume fails loudly instead of wedging.
    #[test]
    fn surface_coop_task_stop_requires_a_pending_resume() {
        use crate::debug::{StopInfo, StopReason, VmExecResult};

        let info = || StopInfo {
            reason: StopReason::Breakpoint,
            file: None,
            line: 7,
        };

        // No resume registered → loud internal error.
        let _ = sema_core::take_debug_coop_resume(); // ensure clean slate
        let err = surface_coop_task_stop(info()).unwrap_err();
        assert!(
            err.to_string().contains("DebugCoopResume"),
            "guard error should name the missing resume: {err}"
        );

        // With a resume registered → surfaces as Stopped on the same line.
        let promise = Rc::new(sema_core::AsyncPromise {
            state: std::cell::RefCell::new(sema_core::PromiseState::Pending),
            task_id: std::cell::Cell::new(0),
        });
        sema_core::set_debug_coop_resume(
            sema_core::SchedulerTarget::One(promise.clone()),
            sema_core::DebugCoopResume::Await(promise),
        );
        let surfaced = surface_coop_task_stop(info());
        let Ok(VmExecResult::Stopped(got)) = surfaced else {
            panic!("expected Stopped(line 7), got {surfaced:?}");
        };
        assert_eq!(got.line, 7);
        let _ = sema_core::take_debug_coop_resume(); // leave the thread-local clean
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
    fn test_breakpoint_fires_on_each_loop_iteration() {
        // Issue #2: resume_skip prevents breakpoints from re-triggering in
        // single-line loops. After stopping at a breakpoint on a loop line,
        // "Continue" should stop again on the next iteration.
        use crate::debug::{DebugState, StepMode, VmExecResult};
        use std::path::PathBuf;

        let globals = make_test_env();
        let ctx = EvalContext::new();

        // A single-line loop: the loop body stays on line 1
        let input = "(let loop ((i 0)) (if (< i 5) (loop (+ i 1)) i))";
        let (vals, span_map) = sema_reader::read_many_with_spans(input).unwrap();
        let source_file = PathBuf::from("<test>");
        let prog = compile_program_with_spans(&vals, &span_map, Some(source_file.clone())).unwrap();

        let mut vm = VM::new(globals, prog.functions, &[], prog.main_cache_slots).unwrap();
        let mut debug = DebugState::new_headless();

        // Set breakpoint on line 1 (the only line)
        debug.set_breakpoints(&source_file, &[1]);
        debug.step_mode = StepMode::StepInto; // stop on entry first

        // Start: should stop on entry
        let result = vm
            .start_cooperative(prog.closure, &ctx, &mut debug)
            .unwrap();
        assert!(
            matches!(result, VmExecResult::Stopped(_)),
            "should stop on entry"
        );

        // Continue: should hit the breakpoint on line 1
        debug.step_mode = StepMode::Continue;
        let result = vm.run_cooperative(&ctx, &mut debug).unwrap();
        assert!(
            matches!(result, VmExecResult::Stopped(_)),
            "should stop at breakpoint on first pass"
        );

        // Continue again: should hit breakpoint again on next iteration
        debug.step_mode = StepMode::Continue;
        let result = vm.run_cooperative(&ctx, &mut debug).unwrap();
        assert!(
            matches!(result, VmExecResult::Stopped(_)),
            "should stop at breakpoint on second iteration (resume_skip bug)"
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
    fn test_debug_evaluate_uses_paused_frame_locals_over_globals() {
        use crate::debug::{DebugState, StepMode, VmExecResult};
        use std::path::PathBuf;

        let globals = make_test_env();
        globals.set(sema_core::intern("x"), Value::int(1));
        let ctx = EvalContext::new();

        let input = "(define (f x)\n  ; body line\n  (+ x 1))\n(f 10)";
        let (vals, span_map) = sema_reader::read_many_with_spans(input).unwrap();
        let source_file = PathBuf::from("<debug-eval>");
        let prog = compile_program_with_spans(&vals, &span_map, Some(source_file.clone())).unwrap();

        let mut vm = VM::new(globals, prog.functions, &[], prog.main_cache_slots).unwrap();
        let mut debug = DebugState::new_headless();
        debug.set_breakpoints(&source_file, &[3]);
        debug.step_mode = StepMode::Continue;

        let result = vm
            .start_cooperative(prog.closure, &ctx, &mut debug)
            .unwrap();
        assert!(matches!(result, VmExecResult::Stopped(_)));
        let frame_id = vm.debug_stack_trace().first().unwrap().id as usize;

        let expr = sema_reader::read("(+ x 5)").unwrap();
        let value = vm.debug_evaluate(frame_id, &expr, &ctx, &debug).unwrap();
        assert_eq!(value, Value::int(15));
    }

    #[test]
    fn test_debug_set_local_updates_paused_stack_slot() {
        use crate::debug::{DebugState, StepMode, VmExecResult};
        use std::path::PathBuf;

        let globals = make_test_env();
        let ctx = EvalContext::new();

        let input = "(define (f x)\n  ; body line\n  (+ x 1))\n(f 10)";
        let (vals, span_map) = sema_reader::read_many_with_spans(input).unwrap();
        let source_file = PathBuf::from("<debug-set>");
        let prog = compile_program_with_spans(&vals, &span_map, Some(source_file.clone())).unwrap();

        let mut vm = VM::new(globals, prog.functions, &[], prog.main_cache_slots).unwrap();
        let mut debug = DebugState::new_headless();
        debug.set_breakpoints(&source_file, &[3]);
        debug.step_mode = StepMode::Continue;

        let result = vm
            .start_cooperative(prog.closure, &ctx, &mut debug)
            .unwrap();
        assert!(matches!(result, VmExecResult::Stopped(_)));
        let frame_id = vm.debug_stack_trace().first().unwrap().id as usize;

        let updated = vm
            .debug_set_variable(
                crate::debug::scope_locals_ref(frame_id),
                "x",
                Value::int(32),
            )
            .unwrap();
        assert_eq!(updated.value, "32");

        let expr = sema_reader::read("(+ x 5)").unwrap();
        let value = vm.debug_evaluate(frame_id, &expr, &ctx, &debug).unwrap();
        assert_eq!(value, Value::int(37));
    }

    #[test]
    fn test_debug_upvalues_use_lexical_names_and_support_closed_mutation() {
        use crate::debug::{DebugState, StepMode, VmExecResult};
        use std::path::PathBuf;

        let globals = make_test_env();
        let ctx = EvalContext::new();
        let input = "(define (make-adder base)\n  (lambda (x)\n    (+ base x)))\n(define add5 (make-adder 5))\n(add5 10)";
        let (vals, span_map) = sema_reader::read_many_with_spans(input).unwrap();
        let source_file = PathBuf::from("<debug-closed-upvalue>");
        let prog = compile_program_with_spans(&vals, &span_map, Some(source_file.clone())).unwrap();

        let mut vm = VM::new(globals, prog.functions, &[], prog.main_cache_slots).unwrap();
        let mut debug = DebugState::new_headless();
        debug.set_breakpoints(&source_file, &[3]);
        debug.step_mode = StepMode::Continue;

        let result = vm
            .start_cooperative(prog.closure, &ctx, &mut debug)
            .unwrap();
        assert!(matches!(result, VmExecResult::Stopped(_)));
        let frame_id = vm.debug_stack_trace().first().unwrap().id as usize;

        let upvalues = vm.debug_variables(crate::debug::scope_upvalues_ref(frame_id));
        assert!(
            upvalues
                .iter()
                .any(|var| var.name == "base" && var.value == "5"),
            "expected lexical upvalue name in debug variables: {upvalues:?}"
        );

        let expr = sema_reader::read("(+ base x)").unwrap();
        assert_eq!(
            vm.debug_evaluate(frame_id, &expr, &ctx, &debug).unwrap(),
            Value::int(15)
        );

        let updated = vm
            .debug_set_variable(
                crate::debug::scope_upvalues_ref(frame_id),
                "base",
                Value::int(20),
            )
            .unwrap();
        assert_eq!(updated.name, "base");
        assert_eq!(updated.value, "20");
        assert_eq!(
            vm.debug_evaluate(frame_id, &expr, &ctx, &debug).unwrap(),
            Value::int(30)
        );
    }

    #[test]
    fn test_debug_upvalues_support_open_mutation_by_lexical_name() {
        use crate::debug::{DebugState, StepMode, VmExecResult};
        use std::path::PathBuf;

        let globals = make_test_env();
        let ctx = EvalContext::new();
        let input =
            "(define (outer base)\n  (define f (lambda (x)\n    (+ base x)))\n  (f 10))\n(outer 5)";
        let (vals, span_map) = sema_reader::read_many_with_spans(input).unwrap();
        let source_file = PathBuf::from("<debug-open-upvalue>");
        let prog = compile_program_with_spans(&vals, &span_map, Some(source_file.clone())).unwrap();

        let mut vm = VM::new(globals, prog.functions, &[], prog.main_cache_slots).unwrap();
        let mut debug = DebugState::new_headless();
        debug.set_breakpoints(&source_file, &[3]);
        debug.step_mode = StepMode::Continue;

        let result = vm
            .start_cooperative(prog.closure, &ctx, &mut debug)
            .unwrap();
        assert!(matches!(result, VmExecResult::Stopped(_)));
        let frame_id = vm.debug_stack_trace().first().unwrap().id as usize;

        vm.debug_set_variable(
            crate::debug::scope_upvalues_ref(frame_id),
            "base",
            Value::int(40),
        )
        .unwrap();
        let expr = sema_reader::read("(+ base x)").unwrap();
        assert_eq!(
            vm.debug_evaluate(frame_id, &expr, &ctx, &debug).unwrap(),
            Value::int(50)
        );
    }

    #[test]
    fn test_debug_variables_expand_compound_values_lazily() {
        use crate::debug::{DebugState, StepMode, VmExecResult};
        use std::path::PathBuf;

        let globals = make_test_env();
        let ctx = EvalContext::new();
        let input = "(define (f xs)\n  (list xs))\n(f (list 1 (list 2 3)))";
        let (vals, span_map) = sema_reader::read_many_with_spans(input).unwrap();
        let source_file = PathBuf::from("<debug-expand>");
        let prog = compile_program_with_spans(&vals, &span_map, Some(source_file.clone())).unwrap();

        let mut vm = VM::new(globals, prog.functions, &[], prog.main_cache_slots).unwrap();
        let mut debug = DebugState::new_headless();
        debug.set_breakpoints(&source_file, &[2]);
        debug.step_mode = StepMode::Continue;

        let result = vm
            .start_cooperative(prog.closure, &ctx, &mut debug)
            .unwrap();
        assert!(matches!(result, VmExecResult::Stopped(_)));
        let frame_id = vm.debug_stack_trace().first().unwrap().id as usize;

        let locals = vm.debug_variables(crate::debug::scope_locals_ref(frame_id));
        let xs = locals
            .iter()
            .find(|var| var.name == "xs")
            .expect("xs local should be visible");
        assert!(
            xs.variables_reference > 0,
            "xs should be expandable: {xs:?}"
        );

        let children = vm.debug_variables(xs.variables_reference);
        assert_eq!(children.len(), 2);
        assert_eq!(children[0].name, "[0]");
        assert_eq!(children[0].value, "1");
        assert_eq!(children[1].name, "[1]");
        assert!(children[1].variables_reference > 0);

        let nested = vm.debug_variables(children[1].variables_reference);
        assert_eq!(nested.len(), 2);
        assert_eq!(nested[0].value, "2");
        assert_eq!(nested[1].value, "3");
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
