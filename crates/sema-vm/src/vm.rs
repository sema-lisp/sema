use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use sema_core::{
    error::{suggest_similar, veteran_hint},
    resolve as resolve_spur, Env, EvalContext, NativeFn, SemaError, Spur, Value, ValueView,
    NAN_INT_SMALL_PATTERN, NAN_PAYLOAD_BITS, NAN_PAYLOAD_MASK, NAN_TAG_MASK, TAG_NATIVE_FN,
};

use crate::chunk::Function;
use crate::opcodes::op;

const DEBUG_VALUE_REF_BASE: u64 = crate::debug::DEBUG_VALUE_REF_BASE;

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
}

/// Payload stored in NativeFn for VM closures.
/// Carries both the closure and the function table from its compilation context.
struct VmClosurePayload {
    closure: Rc<Closure>,
    functions: Rc<Vec<Rc<Function>>>,
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
/// when the name closely matches one in `globals`. Mirrors the tree-walker
/// path in `sema-eval/src/eval.rs::eval_step` so users get the same
/// suggestions regardless of backend.
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
    /// Per-instruction inline cache for global lookups: (spur_bits, env_version, value).
    /// spur_bits distinguishes globals sharing the same slot (cross-VM closures).
    inline_cache: Vec<(u32, u64, Value)>,
    /// Resolved native function table: native_id → (NativeFn Rc, name).
    /// Populated at VM creation from the compiler's native_table + global env.
    native_fns: Vec<Rc<NativeFn>>,
    debug_values: HashMap<u64, Value>,
    next_debug_value_ref: u64,
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
        Ok(VM {
            stack: Vec::with_capacity(256),
            frames: Vec::with_capacity(64),
            globals,
            functions: Rc::new(functions),
            inline_cache: vec![(u32::MAX, 0, Value::nil()); total_cache_slots],
            native_fns,
            debug_values: HashMap::new(),
            next_debug_value_ref: DEBUG_VALUE_REF_BASE,
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
                .resize(needed, (u32::MAX, 0, Value::nil()));
        }
    }

    fn new_with_rc_functions(globals: Rc<Env>, functions: Rc<Vec<Rc<Function>>>) -> Self {
        let total_cache_slots: usize = functions
            .iter()
            .map(|f| f.chunk.n_global_cache_slots as usize)
            .sum();
        VM {
            stack: Vec::with_capacity(256),
            frames: Vec::with_capacity(64),
            globals,
            functions,
            inline_cache: vec![(u32::MAX, 0, Value::nil()); total_cache_slots],
            native_fns: Vec::new(),
            debug_values: HashMap::new(),
            next_debug_value_ref: DEBUG_VALUE_REF_BASE,
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
        Ok(VM {
            stack: Vec::with_capacity(256),
            frames: Vec::with_capacity(64),
            globals,
            functions,
            inline_cache: vec![(u32::MAX, 0, Value::nil()); total_cache_slots],
            native_fns,
            debug_values: HashMap::new(),
            next_debug_value_ref: DEBUG_VALUE_REF_BASE,
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

        loop {
            match self.run_inner(ctx, Some(debug))? {
                crate::debug::VmExecResult::Finished(v) => return Ok(v),
                crate::debug::VmExecResult::Yielded => continue,
                crate::debug::VmExecResult::AsyncYield(_) => {
                    return Err(SemaError::eval(
                        "async yield outside of scheduler context".to_string(),
                    ));
                }
                crate::debug::VmExecResult::Stopped(info) => {
                    // Per the DAP spec, variablesReferences are only valid until
                    // the next stop. Reset the handle map on each stop so Values
                    // expanded in prior stops are not retained for the whole
                    // session (otherwise debug_values grows unbounded across a
                    // long stepping session, pinning the underlying heap).
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
                                break;
                            }
                            Ok(crate::debug::DebugCommand::StepInto) => {
                                debug.step_mode = crate::debug::StepMode::StepInto;
                                debug.step_frame_depth = self.frames.len();
                                break;
                            }
                            Ok(crate::debug::DebugCommand::StepOver) => {
                                debug.step_mode = crate::debug::StepMode::StepOver;
                                debug.step_frame_depth = self.frames.len();
                                break;
                            }
                            Ok(crate::debug::DebugCommand::StepOut) => {
                                debug.step_mode = crate::debug::StepMode::StepOut;
                                debug.step_frame_depth = self.frames.len();
                                break;
                            }
                            Ok(crate::debug::DebugCommand::Pause) => {}
                            Ok(crate::debug::DebugCommand::SetBreakpoints {
                                file,
                                lines,
                                reply,
                            }) => {
                                let ids = debug.set_breakpoints(&file, &lines);
                                let _ = reply.send(ids);
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
                                            .map(|value| {
                                                self.debug_value_to_variable("result", value)
                                            })
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
                                let result =
                                    match crate::debug::decode_scope_ref(variables_reference) {
                                        Some(crate::debug::ScopeKind::Locals(frame_id))
                                        | Some(crate::debug::ScopeKind::Upvalues(frame_id)) => {
                                            sema_reader::read(&value_expression)
                                                .map_err(|e| e.to_string())
                                                .and_then(|expr| {
                                                    self.debug_evaluate(frame_id, &expr, ctx, debug)
                                                        .map_err(|e| e.to_string())
                                                })
                                                .and_then(|value| {
                                                    self.debug_set_variable(
                                                        variables_reference,
                                                        &name,
                                                        value,
                                                    )
                                                    .map_err(|e| e.to_string())
                                                })
                                        }
                                        None => {
                                            Err("setVariable: invalid variablesReference"
                                                .to_string())
                                        }
                                    };
                                let _ = reply.send(result);
                            }
                            Ok(crate::debug::DebugCommand::Disconnect) => {
                                return Ok(Value::nil());
                            }
                            Err(_) => {
                                debug.step_mode = crate::debug::StepMode::Continue;
                                break;
                            }
                        }
                    }
                }
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
        self.run_inner(ctx, Some(debug))
    }

    /// Start cooperative debug execution: push the initial frame and run.
    pub fn start_cooperative(
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
        self.run_inner(ctx, Some(debug))
    }

    /// Number of active call frames.
    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    fn run(&mut self, ctx: &EvalContext) -> Result<Value, SemaError> {
        match self.run_inner(ctx, None)? {
            crate::debug::VmExecResult::Finished(v) => Ok(v),
            crate::debug::VmExecResult::Stopped(_) | crate::debug::VmExecResult::Yielded => {
                unreachable!("Stopped/Yielded without debug state")
            }
            crate::debug::VmExecResult::AsyncYield(_) => Err(SemaError::eval(
                "async yield outside of scheduler context".to_string(),
            )),
        }
    }

    /// Run the VM without debug state, returning the raw VmExecResult.
    /// Used by the async scheduler for task execution and resume.
    pub fn run_async(
        &mut self,
        ctx: &EvalContext,
    ) -> Result<crate::debug::VmExecResult, SemaError> {
        self.run_inner(ctx, None)
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
        self.run_inner(ctx, None)
    }

    /// Prepare the VM to run `closure` with `args` already bound to its
    /// parameters, but do not run it. The scheduler uses this to register
    /// a closure-as-task whose first call will be `run_async`.
    pub fn setup_for_call(
        &mut self,
        closure: Rc<Closure>,
        args: &[Value],
    ) -> Result<(), SemaError> {
        let func = &closure.func;
        let arity = func.arity as usize;
        let has_rest = func.has_rest;
        let n_locals = func.chunk.n_locals as usize;

        if has_rest {
            if args.len() < arity {
                return Err(SemaError::arity(
                    func.name
                        .map(resolve_spur)
                        .unwrap_or_else(|| "<lambda>".to_string()),
                    format!("{}+", arity),
                    args.len(),
                ));
            }
        } else if args.len() != arity {
            return Err(SemaError::arity(
                func.name
                    .map(resolve_spur)
                    .unwrap_or_else(|| "<lambda>".to_string()),
                arity.to_string(),
                args.len(),
            ));
        }

        self.ensure_cache_space(func);
        let base = self.stack.len();
        self.stack.resize(base + n_locals, Value::nil());
        if has_rest {
            for i in 0..arity {
                self.stack[base + i] = args.get(i).cloned().unwrap_or(Value::nil());
            }
            let rest: Vec<Value> = args[arity..].to_vec();
            self.stack[base + arity] = Value::list(rest);
        } else {
            for i in 0..arity {
                self.stack[base + i] = args.get(i).cloned().unwrap_or(Value::nil());
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

    fn run_inner(
        &mut self,
        ctx: &EvalContext,
        mut debug: Option<&mut crate::debug::DebugState>,
    ) -> Result<crate::debug::VmExecResult, SemaError> {
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

        // Two-level dispatch: outer loop caches frame locals, inner loop dispatches opcodes.
        // We only break to the outer loop when frames change (Call/TailCall/Return/exceptions).
        let mut debug_poll_counter: u32 = 0;
        // Track whether we've been through at least one dispatch iteration.
        // On frame transitions (Call/TailCall/Return), resume_skip is cleared
        // so breakpoints re-trigger on new loop iterations.
        let mut dispatch_count: u32 = 0;
        'dispatch: loop {
            if dispatch_count > 0 {
                if let Some(ref mut dbg) = debug {
                    dbg.resume_skip = false;
                }
            }
            dispatch_count += 1;
            // Wall-clock deadline check on every frame transition. Catches
            // infinite tail recursion like `(define (loop) (loop))` which
            // re-enters this outer loop on every TailCall.
            if ctx.deadline_exceeded() {
                return Err(SemaError::eval(
                    "evaluation exceeded time budget (looks like an infinite loop?)".to_string(),
                ));
            }
            let fi = self.frames.len() - 1;
            let frame = &self.frames[fi];
            let code = frame.closure.func.chunk.code.as_ptr();
            let consts: *const [Value] = frame.closure.func.chunk.consts.as_slice();
            let base = frame.base;
            let mut pc = frame.pc;
            let code_len = frame.closure.func.chunk.code.len();

            // Cache the next span boundary to avoid binary_search per instruction
            let (mut next_span_idx, mut next_span_pc) = if debug.is_some() {
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
                if pc >= code_len {
                    return Err(SemaError::eval(format!(
                        "VM: program counter out of bounds (pc={pc}, len={code_len})"
                    )));
                }
                let op = unsafe { *code.add(pc) };
                pc += 1;

                // Debug hook: span-cached check and command polling
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
                                    return Ok(crate::debug::VmExecResult::Finished(Value::nil()));
                                }
                                crate::debug::DebugCommand::SetBreakpoints {
                                    file,
                                    lines,
                                    reply,
                                } => {
                                    let ids = dbg.set_breakpoints(&file, &lines);
                                    let _ = reply.send(ids);
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
                                crate::debug::DebugCommand::GetVariables { reference, reply } => {
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
                                next_span_pc = spans.get(i).map(|(p, _)| *p).unwrap_or(u32::MAX);
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
                            if dbg.should_stop(file.as_ref(), line, frame_depth) {
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
                        dbg.instructions_remaining = dbg.instructions_remaining.saturating_sub(128);
                        if dbg.instructions_remaining == 0 {
                            self.frames[fi].pc = pc - 1;
                            return Ok(crate::debug::VmExecResult::Yielded);
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
                    op::STORE_LOCAL => {
                        let slot = read_u16!(code, pc) as usize;
                        let val = unsafe { pop_unchecked(&mut self.stack) };
                        self.stack[base + slot] = val;
                    }

                    // --- Upvalues ---
                    op::LOAD_UPVALUE => {
                        let idx = read_u16!(code, pc) as usize;
                        let val = {
                            let state = self.frames[fi].closure.upvalues[idx].state.borrow();
                            match &*state {
                                UpvalueState::Closed(v) => v.clone(),
                                UpvalueState::Open { frame_base, slot } => {
                                    self.stack[*frame_base + *slot].clone()
                                }
                            }
                        };
                        self.stack.push(val);
                    }
                    op::STORE_UPVALUE => {
                        let idx = read_u16!(code, pc) as usize;
                        let val = unsafe { pop_unchecked(&mut self.stack) };
                        {
                            let mut state =
                                self.frames[fi].closure.upvalues[idx].state.borrow_mut();
                            match &mut *state {
                                UpvalueState::Closed(v) => *v = val,
                                UpvalueState::Open { frame_base, slot } => {
                                    self.stack[*frame_base + *slot] = val;
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
                            self.stack.push(entry.2.clone());
                        } else {
                            // SAFETY: Spur is #[repr(transparent)] over NonZeroU32 (from lasso crate).
                            // `bits` was emitted by the compiler as the u32 representation of an
                            // interned Spur for this global name; it is therefore guaranteed
                            // non-zero and layout-compatible with Spur.
                            let spur: Spur = unsafe { std::mem::transmute::<u32, Spur>(bits) };
                            match self.globals.get(spur) {
                                Some(val) => {
                                    self.inline_cache[cache_idx] = (bits, version, val.clone());
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
                        // SAFETY: Spur is #[repr(transparent)] over NonZeroU32. `bits` was emitted
                        // by the compiler from an interned Spur for this global name and is
                        // therefore non-zero and layout-compatible with Spur.
                        let spur: Spur = unsafe { std::mem::transmute::<u32, Spur>(bits) };
                        let val = unsafe { pop_unchecked(&mut self.stack) };
                        if !self.globals.set_existing(spur, val.clone()) {
                            self.globals.set(spur, val);
                        }
                    }
                    op::DEFINE_GLOBAL => {
                        let bits = read_u32!(code, pc);
                        // SAFETY: Spur is #[repr(transparent)] over NonZeroU32. `bits` was emitted
                        // by the compiler from an interned Spur for this global name and is
                        // therefore non-zero and layout-compatible with Spur.
                        let spur: Spur = unsafe { std::mem::transmute::<u32, Spur>(bits) };
                        let val = unsafe { pop_unchecked(&mut self.stack) };
                        self.globals.set(spur, val);
                    }

                    // --- Control flow ---
                    op::JUMP => {
                        let offset = read_i32!(code, pc);
                        pc = (pc as i64 + offset as i64) as usize;
                        // Backward jump: this is a loop iteration boundary.
                        // Check the wall-clock deadline so tight loops like
                        // `(while #t)` can be interrupted.
                        if offset < 0 && ctx.deadline_exceeded() {
                            return Err(SemaError::eval(
                                "evaluation exceeded time budget (looks like an infinite loop?)"
                                    .to_string(),
                            ));
                        }
                    }
                    op::JUMP_IF_FALSE => {
                        let offset = read_i32!(code, pc);
                        let val = unsafe { pop_unchecked(&mut self.stack) };
                        if !val.is_truthy() {
                            pc = (pc as i64 + offset as i64) as usize;
                            if offset < 0 && ctx.deadline_exceeded() {
                                return Err(SemaError::eval(
                                    "evaluation exceeded time budget (looks like an infinite loop?)"
                                        .to_string(),
                                ));
                            }
                        }
                    }
                    op::JUMP_IF_TRUE => {
                        let offset = read_i32!(code, pc);
                        let val = unsafe { pop_unchecked(&mut self.stack) };
                        if val.is_truthy() {
                            pc = (pc as i64 + offset as i64) as usize;
                            if offset < 0 && ctx.deadline_exceeded() {
                                return Err(SemaError::eval(
                                    "evaluation exceeded time budget (looks like an infinite loop?)"
                                        .to_string(),
                                ));
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

                        // Close open upvalues before non-VM call (native may invoke VM closures via callback)
                        if let Some(ref mut open) = self.frames[fi].open_upvalues {
                            close_open_upvalues(open, &self.stack, base);
                        }

                        // Borrow args directly from stack (no Vec allocation).
                        // Rc::clone is cheap; it avoids holding &self.native_fns
                        // and &self.stack simultaneously.
                        let native = self.native_fns[native_id].clone();
                        let args_start = self.stack.len() - argc;
                        let result = (native.func)(ctx, &self.stack[args_start..]);
                        self.stack.truncate(args_start);
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
                        continue 'dispatch;
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
                        let mut map = hashbrown::HashMap::new();
                        for pair in items.chunks(2) {
                            map.insert(pair[0].clone(), pair[1].clone());
                        }
                        self.stack.push(Value::hashmap_from_rc(Rc::new(map)));
                    }

                    // --- Exceptions ---
                    op::THROW => {
                        let val = unsafe { pop_unchecked(&mut self.stack) };
                        let err = SemaError::UserException(val);
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
                            self.stack.push(Value::int(n.wrapping_neg()));
                        } else if let Some(f) = a.as_float() {
                            self.stack.push(Value::float(-f));
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
                            let sum = (a_bits.wrapping_add(b_bits)) & NAN_PAYLOAD_MASK;
                            let result = NAN_INT_SMALL_PATTERN | sum;
                            unsafe {
                                std::ptr::write(
                                    self.stack.as_mut_ptr().add(len - 2),
                                    Value::from_raw_bits(result),
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
                            let diff = (a_bits.wrapping_sub(b_bits)) & NAN_PAYLOAD_MASK;
                            let result = NAN_INT_SMALL_PATTERN | diff;
                            unsafe {
                                std::ptr::write(
                                    self.stack.as_mut_ptr().add(len - 2),
                                    Value::from_raw_bits(result),
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
                            // Use Value::int for multiplication — result may overflow 45 bits
                            unsafe {
                                std::ptr::write(
                                    self.stack.as_mut_ptr().add(len - 2),
                                    Value::int(ax.wrapping_mul(bx)),
                                );
                                self.stack.set_len(len - 1);
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

                        // Look up the global (with inline cache)
                        let cache_idx = self.frames[fi].cache_base + cache_slot;
                        let version = self.globals.version.get();
                        let entry = &self.inline_cache[cache_idx];
                        let func_val = if entry.0 == bits && entry.1 == version {
                            entry.2.clone()
                        } else {
                            // SAFETY: Spur is #[repr(transparent)] over NonZeroU32. `bits` was
                            // emitted by the compiler from an interned Spur for the callee name
                            // and is therefore non-zero and layout-compatible with Spur.
                            let spur: Spur = unsafe { std::mem::transmute::<u32, Spur>(bits) };
                            match self.globals.get(spur) {
                                Some(val) => {
                                    self.inline_cache[cache_idx] = (bits, version, val.clone());
                                    val
                                }
                                None => {
                                    let err = unbound_global_error(spur, &self.globals);
                                    handle_err!(self, fi, pc, err, saved_pc, 'dispatch);
                                }
                            }
                        };

                        // Fast path: VM closure — use direct call without function slot
                        if func_val.raw_tag() == Some(TAG_NATIVE_FN) {
                            let vm_closure_data = {
                                let native = func_val.as_native_fn_ref().unwrap();
                                native.payload.as_ref().and_then(|p| {
                                    p.downcast_ref::<VmClosurePayload>().map(|vmc| {
                                        let closure = vmc.closure.clone();
                                        let functions =
                                            if Rc::ptr_eq(&vmc.functions, &self.functions) {
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
                                if let Err(err) = self.call_vm_closure_direct(closure, argc) {
                                    match self.handle_exception(err, saved_pc)? {
                                        ExceptionAction::Handled => {}
                                        ExceptionAction::Propagate(e) => return Err(e),
                                    }
                                }
                                continue 'dispatch;
                            }
                        }

                        // Slow path: non-VM callable — use call_value_with
                        if let Err(err) = self.call_value_with(func_val, argc, ctx) {
                            drop(sema_core::take_yield_signal());
                            match self.handle_exception(err, saved_pc)? {
                                ExceptionAction::Handled => {}
                                ExceptionAction::Propagate(e) => return Err(e),
                            }
                        }
                        // Check if a native function (called via call_value_with) signaled async yield
                        if let Some(reason) = sema_core::take_yield_signal() {
                            // call_value_with already pushed a result value. Replace it
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
                        self.stack.push(Value::bool(val.is_int() || val.is_float()));
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
                            let err = SemaError::type_error("map or hashmap", coll.type_name());
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
                            let err = SemaError::type_error("map or hashmap", coll.type_name());
                            handle_err!(self, fi, pc, err, pc - op::SIZE_OP, 'dispatch);
                        }
                    }

                    op::MOD => {
                        let b = unsafe { pop_unchecked(&mut self.stack) };
                        let a = unsafe { pop_unchecked(&mut self.stack) };
                        match (a.as_int(), b.as_int()) {
                            (Some(ai), Some(bi)) => {
                                if bi == 0 {
                                    let err = SemaError::eval("modulo by zero");
                                    handle_err!(self, fi, pc, err, pc - op::SIZE_OP, 'dispatch);
                                } else {
                                    self.stack.push(Value::int(ai % bi));
                                }
                            }
                            _ => {
                                let af = a.as_float().or_else(|| a.as_int().map(|i| i as f64));
                                let bf = b.as_float().or_else(|| b.as_int().map(|i| i as f64));
                                match (af, bf) {
                                    (Some(af), Some(bf)) => {
                                        self.stack.push(Value::float(af % bf));
                                    }
                                    _ => {
                                        let err = SemaError::type_error("number", a.type_name());
                                        handle_err!(self, fi, pc, err, pc - op::SIZE_OP, 'dispatch);
                                    }
                                }
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
                                let err = SemaError::type_error("int", idx_val.type_name());
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
                        } else {
                            let err = SemaError::type_error("list or vector", coll.type_name());
                            handle_err!(self, fi, pc, err, pc - op::SIZE_OP, 'dispatch);
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
            // Close open upvalues before non-VM call (native may invoke VM closures via callback)
            let caller_base = self.frames.last().unwrap().base;
            if let Some(ref mut open) = self.frames.last_mut().unwrap().open_upvalues {
                close_open_upvalues(open, &self.stack, caller_base);
            }
            // Regular native fn — borrow args directly from stack (no Vec allocation)
            let func_rc = self.stack[func_idx].as_native_fn_rc().unwrap();
            let args_start = func_idx + 1;
            let result = (func_rc.func)(ctx, &self.stack[args_start..]);
            self.stack.truncate(func_idx);
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
            // Close open upvalues before non-VM call (callback may invoke VM closures)
            let caller_base = self.frames.last().unwrap().base;
            if let Some(ref mut open) = self.frames.last_mut().unwrap().open_upvalues {
                close_open_upvalues(open, &self.stack, caller_base);
            }
            // Lambda or other callable — borrow args directly from stack.
            // Safety: call_callback dispatches to the tree-walking evaluator,
            // which has its own environment and does not mutate the VM's stack.
            let func_val = self.stack[func_idx].clone();
            let args_start = func_idx + 1;
            let result = sema_core::call_callback(ctx, &func_val, &self.stack[args_start..]);
            self.stack.truncate(func_idx);
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
            // Close open upvalues before non-VM call (native may invoke VM closures via callback)
            let caller_base = self.frames.last().unwrap().base;
            if let Some(ref mut open) = self.frames.last_mut().unwrap().open_upvalues {
                close_open_upvalues(open, &self.stack, caller_base);
            }
            let func_rc = func_val.as_native_fn_rc().unwrap();
            let args_start = self.stack.len() - argc;
            let result = (func_rc.func)(ctx, &self.stack[args_start..]);
            self.stack.truncate(args_start);
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
            // Close open upvalues before non-VM call (callback may invoke VM closures)
            let caller_base = self.frames.last().unwrap().base;
            if let Some(ref mut open) = self.frames.last_mut().unwrap().open_upvalues {
                close_open_upvalues(open, &self.stack, caller_base);
            }
            // Safety: call_callback dispatches to the tree-walking evaluator,
            // which does not mutate the VM's stack.
            let args_start = self.stack.len() - argc;
            let result = sema_core::call_callback(ctx, &func_val, &self.stack[args_start..]);
            self.stack.truncate(args_start);
            let result = result?;
            self.stack.push(result);
            Ok(())
        }
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

        let closure = Rc::new(Closure { func, upvalues });
        let payload: Rc<dyn std::any::Any> = Rc::new(VmClosurePayload {
            closure: closure.clone(),
            functions: self.functions.clone(),
        });
        let closure_for_fallback = closure.clone();
        let functions = self.functions.clone();
        let globals = self.globals.clone();

        // The NativeFn wrapper is used as a fallback when called from outside the VM
        // (e.g., from stdlib HOFs like map/filter). Inside the VM, call_value detects
        // the payload and pushes a CallFrame instead — no Rust recursion.
        let native = Value::native_fn_from_rc(Rc::new(sema_core::NativeFn::with_payload(
            closure_for_fallback
                .func
                .name
                .map(resolve_spur)
                .unwrap_or_else(|| "<vm-closure>".to_string()),
            payload,
            move |ctx, args| {
                // Inside an async task, route through the scheduler so any
                // yield in the inner closure (channel/send, channel/recv,
                // await, sleep) suspends cleanly. Otherwise the inner VM's
                // AsyncYield would surface as "async yield outside of
                // scheduler context" and crash the calling HOF.
                if sema_core::in_async_context() {
                    return crate::scheduler::run_closure_as_inline_task(
                        ctx,
                        closure_for_fallback.clone(),
                        functions.clone(),
                        args,
                    );
                }

                let mut vm = VM::new_with_rc_functions(globals.clone(), functions.clone());
                vm.setup_for_call(closure_for_fallback.clone(), args)?;
                vm.run(ctx)
            },
        )));

        self.stack.push(native);
        Ok(())
    }

    // --- Exception handling ---

    #[cold]
    #[inline(never)]
    fn handle_exception(
        &mut self,
        err: SemaError,
        failing_pc: usize,
    ) -> Result<ExceptionAction, SemaError> {
        let mut pc_for_lookup = failing_pc as u32;
        // Walk frames from top looking for a handler
        while let Some(frame) = self.frames.last() {
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

                // Push error value as a map matching the tree-walker's error_to_value format
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
        let pc32 = frame.pc as u32;
        let spans = &frame.closure.func.chunk.spans;
        // Find the most recent span at or before the current PC
        match spans.binary_search_by_key(&pc32, |(p, _)| *p) {
            Ok(idx) => {
                let span = &spans[idx].1;
                (span.line as u64, span.col as u64 + 1)
            }
            Err(idx) if idx > 0 => {
                let span = &spans[idx - 1].1;
                (span.line as u64, span.col as u64 + 1)
            }
            _ => (0, 0),
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
            value.view(),
            ValueView::List(_)
                | ValueView::Vector(_)
                | ValueView::Map(_)
                | ValueView::HashMap(_)
                | ValueView::Record(_)
                | ValueView::Bytevector(_)
        )
    }

    fn debug_children(&mut self, value: Value) -> Vec<crate::debug::DapVariable> {
        match value.view() {
            ValueView::List(items) | ValueView::Vector(items) => items
                .iter()
                .enumerate()
                .map(|(i, child)| self.debug_value_to_variable(&format!("[{i}]"), child.clone()))
                .collect(),
            ValueView::Map(map) => map
                .iter()
                .map(|(key, child)| {
                    self.debug_value_to_variable(&sema_core::pretty_print(key, 80), child.clone())
                })
                .collect(),
            ValueView::HashMap(map) => {
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
            ValueView::Record(record) => record
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
            ValueView::Bytevector(bytes) => bytes
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

/// Convert a SemaError into a Sema map value, matching the tree-walker's format.
fn error_to_value(err: &SemaError) -> Value {
    let inner = err.inner();
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
    }
    Value::map(map)
}

// --- Arithmetic helpers ---

#[inline(always)]
fn vm_add(a: &Value, b: &Value) -> Result<Value, SemaError> {
    use sema_core::ValueView;
    match (a.view(), b.view()) {
        (ValueView::Int(x), ValueView::Int(y)) => Ok(Value::int(x.wrapping_add(y))),
        (ValueView::Float(x), ValueView::Float(y)) => Ok(Value::float(x + y)),
        (ValueView::Int(x), ValueView::Float(y)) => Ok(Value::float(x as f64 + y)),
        (ValueView::Float(x), ValueView::Int(y)) => Ok(Value::float(x + y as f64)),
        (ValueView::String(x), ValueView::String(y)) => {
            let mut s = (*x).clone();
            s.push_str(&y);
            Ok(Value::string(&s))
        }
        _ => Err(SemaError::type_error(
            "number or string",
            format!("{} and {}", a.type_name(), b.type_name()),
        )),
    }
}

#[inline(always)]
fn vm_sub(a: &Value, b: &Value) -> Result<Value, SemaError> {
    use sema_core::ValueView;
    match (a.view(), b.view()) {
        (ValueView::Int(x), ValueView::Int(y)) => Ok(Value::int(x.wrapping_sub(y))),
        (ValueView::Float(x), ValueView::Float(y)) => Ok(Value::float(x - y)),
        (ValueView::Int(x), ValueView::Float(y)) => Ok(Value::float(x as f64 - y)),
        (ValueView::Float(x), ValueView::Int(y)) => Ok(Value::float(x - y as f64)),
        _ => Err(SemaError::type_error(
            "number",
            format!("{} and {}", a.type_name(), b.type_name()),
        )),
    }
}

#[inline(always)]
fn vm_mul(a: &Value, b: &Value) -> Result<Value, SemaError> {
    use sema_core::ValueView;
    match (a.view(), b.view()) {
        (ValueView::Int(x), ValueView::Int(y)) => Ok(Value::int(x.wrapping_mul(y))),
        (ValueView::Float(x), ValueView::Float(y)) => Ok(Value::float(x * y)),
        (ValueView::Int(x), ValueView::Float(y)) => Ok(Value::float(x as f64 * y)),
        (ValueView::Float(x), ValueView::Int(y)) => Ok(Value::float(x * y as f64)),
        _ => Err(SemaError::type_error(
            "number",
            format!("{} and {}", a.type_name(), b.type_name()),
        )),
    }
}

#[inline(always)]
fn vm_div(a: &Value, b: &Value) -> Result<Value, SemaError> {
    use sema_core::ValueView;
    match (a.view(), b.view()) {
        (ValueView::Int(_), ValueView::Int(0)) => Err(SemaError::eval("division by zero")),
        (ValueView::Int(x), ValueView::Int(y)) => {
            if x % y == 0 {
                Ok(Value::int(x / y))
            } else {
                Ok(Value::float(x as f64 / y as f64))
            }
        }
        (ValueView::Float(x), ValueView::Float(y)) => Ok(Value::float(x / y)),
        (ValueView::Int(x), ValueView::Float(y)) => Ok(Value::float(x as f64 / y)),
        (ValueView::Float(x), ValueView::Int(y)) => Ok(Value::float(x / y as f64)),
        _ => Err(SemaError::type_error(
            "number",
            format!("{} and {}", a.type_name(), b.type_name()),
        )),
    }
}

/// Numeric-coercing equality: matches stdlib `=` semantics.
#[inline(always)]
fn vm_eq(a: &Value, b: &Value) -> bool {
    use sema_core::ValueView;
    match (a.view(), b.view()) {
        (ValueView::Int(x), ValueView::Int(y)) => x == y,
        (ValueView::Float(x), ValueView::Float(y)) => x == y,
        (ValueView::Int(x), ValueView::Float(y)) | (ValueView::Float(y), ValueView::Int(x)) => {
            (x as f64) == y
        }
        _ => a == b,
    }
}

fn vm_lt(a: &Value, b: &Value) -> Result<bool, SemaError> {
    use sema_core::ValueView;
    match (a.view(), b.view()) {
        (ValueView::Int(x), ValueView::Int(y)) => Ok(x < y),
        (ValueView::Float(x), ValueView::Float(y)) => Ok(x < y),
        (ValueView::Int(x), ValueView::Float(y)) => Ok((x as f64) < y),
        (ValueView::Float(x), ValueView::Int(y)) => Ok(x < (y as f64)),
        (ValueView::String(x), ValueView::String(y)) => Ok(x < y),
        _ => Err(SemaError::type_error(
            "comparable values",
            format!("{} and {}", a.type_name(), b.type_name()),
        )),
    }
}

/// Compile Value ASTs with span/source info for debug support (DAP breakpoints).
pub fn compile_program_with_spans(
    vals: &[Value],
    span_map: &sema_core::SpanMap,
    source_file: Option<std::path::PathBuf>,
) -> Result<CompiledProgram, SemaError> {
    let source_file = source_file.map(|p| std::fs::canonicalize(&p).unwrap_or(p));
    let mut resolved = Vec::new();
    let mut total_locals: u16 = 0;
    for val in vals {
        let core = crate::lower::lower(val, Some(span_map))?;
        let core = crate::optimize::optimize(core);
        let (res, n) = crate::resolve::resolve_with_locals(&core)?;
        total_locals = total_locals.max(n);
        resolved.push(res);
    }
    let result = crate::compiler::compile(&resolved, total_locals, None)?;

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
            local_names: Vec::new(),
            source_file,
            local_scopes: Vec::new(),
            cache_offset: 0,
        }),
        upvalues: Vec::new(),
    });

    Ok(CompiledProgram {
        closure,
        functions,
        native_table: Vec::new(),
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
    let mut resolved = Vec::new();
    let mut total_locals: u16 = 0;
    for val in vals {
        let core = crate::lower::lower(val, None)?;
        let core = crate::optimize::optimize(core);
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
            local_names: Vec::new(),
            source_file: None,
            local_scopes: Vec::new(),
            cache_offset: 0,
        }),
        upvalues: Vec::new(),
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
            local_names: vec![],
            source_file: None,
            local_scopes: Vec::new(),
            cache_offset: 0,
        });
        let closure = Rc::new(Closure {
            func: func.clone(),
            upvalues: vec![],
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
    fn test_vm_div_int_returns_float_when_non_whole() {
        let globals = make_test_env();
        let ctx = EvalContext::new();
        let result = eval_str("(/ 3 2)", &globals, &ctx).unwrap();
        assert_eq!(result, Value::float(1.5));
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
        let globals = make_test_env();
        let ctx = EvalContext::new();
        let result = eval_str("(/ 7 3)", &globals, &ctx).unwrap();
        assert!(
            result.as_float().is_some(),
            "expected float, got {:?}",
            result
        );
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
        e.emit_const(Value::int(1));
        e.emit_const(Value::float(1.0));
        e.emit_op(Op::Eq);
        e.emit_op(Op::Return);
        let func = Rc::new(crate::chunk::Function {
            name: None,
            chunk: e.into_chunk(),
            upvalue_descs: vec![],
            upvalue_names: vec![],
            arity: 0,
            has_rest: false,
            local_names: vec![],
            source_file: None,
            local_scopes: Vec::new(),
            cache_offset: 0,
        });
        let closure = Rc::new(Closure {
            func,
            upvalues: vec![],
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
        // resolved native table. In release builds the old debug_assert! was
        // compiled out and the bounds-checked index panicked (DoS). It must now
        // return a SemaError instead.
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
            local_names: vec![],
            source_file: None,
            local_scopes: Vec::new(),
            cache_offset: 0,
        });
        let closure = Rc::new(Closure {
            func,
            upvalues: vec![],
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
            local_names: vec![],
            source_file: None,
            local_scopes: Vec::new(),
            cache_offset: 0,
        });
        let closure = Rc::new(Closure {
            func,
            upvalues: vec![],
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
            local_names: vec![],
            upvalue_names: vec![],
            source_file: Some(source_b.clone()),
            local_scopes: Vec::new(),
            cache_offset: 0,
        });

        let main = Rc::new(Closure {
            func: func_a.clone(),
            upvalues: vec![],
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
        e.emit_const(Value::int(3));
        e.emit_const(Value::int(2));
        e.emit_op(Op::Div);
        e.emit_op(Op::Return);
        let func = Rc::new(crate::chunk::Function {
            name: None,
            chunk: e.into_chunk(),
            upvalue_descs: vec![],
            upvalue_names: vec![],
            arity: 0,
            has_rest: false,
            local_names: vec![],
            source_file: None,
            local_scopes: Vec::new(),
            cache_offset: 0,
        });
        let closure = Rc::new(Closure {
            func,
            upvalues: vec![],
        });
        let mut vm = VM::new(globals, vec![], &[], 0).unwrap();
        let result = vm.execute(closure, &ctx).unwrap();
        assert_eq!(
            result,
            Value::float(1.5),
            "Op::Div 3/2 should return 1.5, not 1"
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
