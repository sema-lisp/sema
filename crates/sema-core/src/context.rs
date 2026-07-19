use std::cell::{Cell, Ref, RefCell, RefMut};
use std::collections::{BTreeMap, HashMap};
use std::ops::{Deref, DerefMut};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

use crate::runtime::{
    apply_dynamic_mutations, DynamicStackIdentities, DynamicTaskState, ModuleTaskState, ScopeId,
    TaskContextHandle,
};
use crate::{CallFrame, Env, Sandbox, SemaError, Span, SpanMap, StackTrace, Value};

const MAX_SPAN_TABLE_ENTRIES: usize = 200_000;

/// Function-pointer type for the full evaluator callback: (ctx, expr, env) -> Result<Value, SemaError>
pub type EvalCallbackFn = fn(&EvalContext, &Value, &Env) -> Result<Value, SemaError>;

/// Function-pointer type for calling a function value with evaluated arguments: (ctx, func, args) -> Result<Value, SemaError>
pub type CallCallbackFn = fn(&EvalContext, &Value, &[Value]) -> Result<Value, SemaError>;

/// Function-pointer type for the owned-args variant of the call callback: the
/// caller passes a buffer it owns and will not reuse, and the callee may move
/// values out of it (leaving nil behind). See [`call_callback_owned`].
pub type CallOwnedCallbackFn = fn(&EvalContext, &Value, &mut [Value]) -> Result<Value, SemaError>;

type SignalTeardownHook = Box<dyn Fn()>;

pub type ContextStackMap = BTreeMap<Value, Vec<Value>>;

/// Legacy-compatible dynamic stack storage whose mutable guard renews opaque
/// entry identities on drop. Direct mutation therefore cannot bypass the ABA
/// protection used by task-root snapshots.
#[derive(Default)]
pub struct ContextStacks {
    values: RefCell<ContextStackMap>,
    identities: RefCell<DynamicStackIdentities>,
}

impl ContextStacks {
    pub fn borrow(&self) -> Ref<'_, ContextStackMap> {
        self.values.borrow()
    }

    pub fn borrow_mut(&self) -> ContextStacksMut<'_> {
        ContextStacksMut {
            values: self.values.borrow_mut(),
            identities: self.identities.borrow_mut(),
        }
    }

    fn snapshot(&self) -> (ContextStackMap, DynamicStackIdentities) {
        (
            self.values.borrow().clone(),
            self.identities.borrow().clone(),
        )
    }

    fn borrow_parts_mut(
        &self,
    ) -> (
        RefMut<'_, ContextStackMap>,
        RefMut<'_, DynamicStackIdentities>,
    ) {
        (self.values.borrow_mut(), self.identities.borrow_mut())
    }
}

pub struct ContextStacksMut<'a> {
    values: RefMut<'a, ContextStackMap>,
    identities: RefMut<'a, DynamicStackIdentities>,
}

impl Deref for ContextStacksMut<'_> {
    type Target = ContextStackMap;

    fn deref(&self) -> &Self::Target {
        &self.values
    }
}

impl DerefMut for ContextStacksMut<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.values
    }
}

impl Drop for ContextStacksMut<'_> {
    fn drop(&mut self) {
        *self.identities = DynamicStackIdentities::from_stacks(&self.values);
    }
}

pub struct EvalContext {
    pub module_cache: RefCell<BTreeMap<PathBuf, BTreeMap<String, Value>>>,
    pub embedded_files: RefCell<BTreeMap<PathBuf, Vec<u8>>>,
    pub current_file: RefCell<Vec<PathBuf>>,
    pub module_exports: RefCell<Vec<Option<Vec<String>>>>,
    pub module_load_stack: RefCell<Vec<PathBuf>>,
    pub call_stack: RefCell<Vec<CallFrame>>,
    pub span_table: RefCell<HashMap<usize, Span>>,
    pub eval_depth: Cell<usize>,
    pub max_eval_depth: Cell<usize>,
    pub eval_step_limit: Cell<usize>,
    pub eval_steps: Cell<usize>,
    /// Optional wall-clock deadline for evaluation. When set,
    /// the bytecode VM periodically checks whether the current
    /// time has passed this instant and, if so, abort with an error. Used by
    /// the notebook engine to bound how long a single cell evaluation can run.
    pub eval_deadline: Cell<Option<Instant>>,
    pub sandbox: Sandbox,
    pub user_context: RefCell<Vec<BTreeMap<Value, Value>>>,
    pub hidden_context: RefCell<Vec<BTreeMap<Value, Value>>>,
    pub context_stacks: ContextStacks,
    /// Interpreter-wide roots for callbacks registered by `sys/on-signal`.
    /// This store is intentionally separate from task-local dynamic context:
    /// any root may dispatch a subscription installed by another root.
    signal_callbacks: RefCell<[Vec<Value>; 3]>,
    /// Weak, non-Value hooks that release process signal leases at interpreter
    /// teardown even when an embedder retains the global environment.
    signal_teardown_hooks: RefCell<Vec<SignalTeardownHook>>,
    pub eval_fn: Cell<Option<EvalCallbackFn>>,
    pub call_fn: Cell<Option<CallCallbackFn>>,
    pub call_owned_fn: Cell<Option<CallOwnedCallbackFn>>,
    pub interactive: Cell<bool>,
    task_context: RefCell<Option<InstalledTaskContext>>,
    runtime_quantum_active: Cell<bool>,
}

#[derive(Clone)]
struct InstalledTaskContext {
    handle: TaskContextHandle,
    // Native dispatch lends the whole TaskContext mutably to NativeCallContext.
    // Cache the typed Rc at installation so EvalContext compatibility methods
    // remain usable during that loan without re-borrowing the context map.
    // The canonical state is still the extension owned by `handle`; every
    // installation refreshes this cached clone of the same Rc.
    dynamic: Option<Rc<DynamicTaskState>>,
    module: Option<Rc<ModuleTaskState>>,
}

impl InstalledTaskContext {
    fn new(handle: TaskContextHandle) -> Self {
        let dynamic = handle.get_rc::<DynamicTaskState>();
        let module = handle.get_rc::<ModuleTaskState>();
        Self {
            handle,
            dynamic,
            module,
        }
    }
}

/// RAII guard for a module-load scope: pops the load stack when dropped, so the
/// stack stays balanced on every exit path (early return, `?`, panic). Created
/// by [`EvalContext::enter_module_load`].
pub struct ModuleLoadGuard<'a> {
    ctx: &'a EvalContext,
    scope: ModuleLoadScope,
}

#[derive(Debug)]
enum ModuleLoadScope {
    Ambient(PathBuf),
    Task {
        state: Rc<ModuleTaskState>,
        scope: ScopeId,
    },
}

pub struct TaskContextGuard<'a> {
    ctx: &'a EvalContext,
    previous: Option<InstalledTaskContext>,
}

impl Drop for TaskContextGuard<'_> {
    fn drop(&mut self) {
        self.ctx.task_context.replace(self.previous.take());
    }
}

impl Drop for ModuleLoadGuard<'_> {
    fn drop(&mut self) {
        match &self.scope {
            ModuleLoadScope::Ambient(path) => self.ctx.end_module_load(path),
            ModuleLoadScope::Task { state, scope } => {
                state.remove_loading(*scope);
            }
        }
    }
}

fn check_module_cycle(stack: &[PathBuf], path: &PathBuf) -> Result<(), SemaError> {
    let Some(pos) = stack.iter().position(|candidate| candidate == path) else {
        return Ok(());
    };
    let mut cycle: Vec<String> = stack[pos..]
        .iter()
        .map(|entry| entry.display().to_string())
        .collect();
    cycle.push(path.display().to_string());
    Err(SemaError::eval(format!(
        "cyclic import detected: {}",
        cycle.join(" -> ")
    )))
}

impl EvalContext {
    pub fn new() -> Self {
        EvalContext {
            module_cache: RefCell::new(BTreeMap::new()),
            embedded_files: RefCell::new(BTreeMap::new()),
            current_file: RefCell::new(Vec::new()),
            module_exports: RefCell::new(Vec::new()),
            module_load_stack: RefCell::new(Vec::new()),
            call_stack: RefCell::new(Vec::new()),
            span_table: RefCell::new(HashMap::new()),
            eval_depth: Cell::new(0),
            max_eval_depth: Cell::new(0),
            eval_step_limit: Cell::new(0),
            eval_steps: Cell::new(0),
            eval_deadline: Cell::new(None),
            sandbox: Sandbox::allow_all(),
            user_context: RefCell::new(vec![BTreeMap::new()]),
            hidden_context: RefCell::new(vec![BTreeMap::new()]),
            context_stacks: ContextStacks::default(),
            signal_callbacks: RefCell::default(),
            signal_teardown_hooks: RefCell::default(),
            eval_fn: Cell::new(None),
            call_fn: Cell::new(None),
            call_owned_fn: Cell::new(None),
            interactive: Cell::new(false),
            task_context: RefCell::new(None),
            runtime_quantum_active: Cell::new(false),
        }
    }

    pub fn new_with_sandbox(sandbox: Sandbox) -> Self {
        EvalContext {
            module_cache: RefCell::new(BTreeMap::new()),
            embedded_files: RefCell::new(BTreeMap::new()),
            current_file: RefCell::new(Vec::new()),
            module_exports: RefCell::new(Vec::new()),
            module_load_stack: RefCell::new(Vec::new()),
            call_stack: RefCell::new(Vec::new()),
            span_table: RefCell::new(HashMap::new()),
            eval_depth: Cell::new(0),
            max_eval_depth: Cell::new(0),
            eval_step_limit: Cell::new(0),
            eval_steps: Cell::new(0),
            eval_deadline: Cell::new(None),
            sandbox,
            user_context: RefCell::new(vec![BTreeMap::new()]),
            hidden_context: RefCell::new(vec![BTreeMap::new()]),
            context_stacks: ContextStacks::default(),
            signal_callbacks: RefCell::default(),
            signal_teardown_hooks: RefCell::default(),
            eval_fn: Cell::new(None),
            call_fn: Cell::new(None),
            call_owned_fn: Cell::new(None),
            interactive: Cell::new(false),
            task_context: RefCell::new(None),
            runtime_quantum_active: Cell::new(false),
        }
    }

    pub fn task_context(&self) -> Option<TaskContextHandle> {
        self.task_context
            .borrow()
            .as_ref()
            .map(|installed| installed.handle.clone())
    }

    pub fn install_task_context(&self, handle: TaskContextHandle) -> Option<TaskContextHandle> {
        self.task_context
            .replace(Some(InstalledTaskContext::new(handle)))
            .map(|installed| installed.handle)
    }

    pub fn scope_task_context(&self, handle: TaskContextHandle) -> TaskContextGuard<'_> {
        TaskContextGuard {
            ctx: self,
            previous: self
                .task_context
                .replace(Some(InstalledTaskContext::new(handle))),
        }
    }

    fn dynamic_task_state(&self) -> Option<Rc<DynamicTaskState>> {
        self.task_context
            .borrow()
            .as_ref()
            .and_then(|installed| installed.dynamic.clone())
    }

    fn module_task_state(&self) -> Option<Rc<ModuleTaskState>> {
        self.task_context
            .borrow()
            .as_ref()
            .and_then(|installed| installed.module.clone())
    }

    pub fn enter_runtime_quantum(&self) -> Result<RuntimeQuantumGuard<'_>, SemaError> {
        if self.runtime_quantum_active.replace(true) {
            return Err(SemaError::eval(
                "internal error: runtime VM quantum is already active",
            ));
        }
        // Mirror the per-ctx flag into a thread-local so ctx-less yielding
        // natives (e.g. `async/sleep`, registered via the value-only ABI) can
        // detect that they run under the unified runtime and should surface a
        // yield the runtime will turn into a native wait.
        let previous_thread_local = crate::in_runtime_quantum();
        crate::set_runtime_quantum(true);
        Ok(RuntimeQuantumGuard {
            ctx: self,
            previous_thread_local,
        })
    }

    pub fn runtime_quantum_active(&self) -> bool {
        self.runtime_quantum_active.get()
    }

    /// TEMPORARY BRIDGE — suspend the "runtime quantum active" flag for the
    /// lifetime of the returned guard, restoring the previous value on `Drop`.
    ///
    /// The unified cooperative runtime forbids entering a *fresh* VM while a
    /// runtime quantum is active (that would re-enter the scheduler off-plan).
    /// But legacy user closures that cross context boundaries are still
    /// dispatched through `sema_core::call_callback`, which runs them on a fresh
    /// foreign VM. Those foreign-run helpers are contractually
    /// SYNCHRONOUS-ONLY (their callback must not yield/await/spawn), so the
    /// nested VM never touches the runtime scheduler — it is safe to run it with
    /// the quantum flag suspended.
    ///
    /// This is a one-way bridge that merely carries current language behavior.
    /// It MUST be deleted together with the Task 04 `NativeOutcome::Call`
    /// migration of legacy callback re-entry, which replaces fresh-VM re-entry
    /// with a scheduler-native call and removes the need to suspend the flag.
    pub fn suspend_runtime_quantum(&self) -> QuantumSuspendGuard<'_> {
        // Suspend BOTH flags `enter_runtime_quantum` set: the per-ctx flag (read
        // by the VM `run` entry guard) AND the thread-local mirror (read by
        // ctx-less yielding natives via `in_runtime_quantum` — `async/sleep`,
        // `mcp/call`, the channel ops). A synchronous nested re-entry must see a
        // FULLY suspended quantum: if only the ctx flag were cleared, a yielding
        // native running on the nested VM would still observe the thread-local as
        // active, surface a runtime yield, and crash the synchronous run with
        // "async yield outside of scheduler context".
        QuantumSuspendGuard {
            ctx: self,
            previous_ctx: self.runtime_quantum_active.replace(false),
            previous_thread_local: crate::in_runtime_quantum(),
        }
        .with_thread_local_suspended()
    }

    pub fn take_task_context(&self) -> Option<TaskContextHandle> {
        self.task_context
            .borrow_mut()
            .take()
            .map(|installed| installed.handle)
    }

    #[doc(hidden)]
    pub fn register_signal_callback(&self, signal_index: usize, callback: Value) {
        self.signal_callbacks.borrow_mut()[signal_index].push(callback);
    }

    #[doc(hidden)]
    pub fn signal_callbacks(&self, signal_index: usize) -> Vec<Value> {
        self.signal_callbacks.borrow()[signal_index].clone()
    }

    #[doc(hidden)]
    pub fn register_signal_teardown_hook(&self, hook: impl Fn() + 'static) {
        self.signal_teardown_hooks.borrow_mut().push(Box::new(hook));
    }

    #[doc(hidden)]
    pub fn try_run_signal_teardown_hooks(&self) -> bool {
        let hooks = match self.signal_teardown_hooks.try_borrow_mut() {
            Ok(mut hooks) => std::mem::take(&mut *hooks),
            Err(_) => return false,
        };
        for hook in hooks {
            hook();
        }
        true
    }

    #[doc(hidden)]
    pub fn clear_signal_callbacks(&self) {
        for callbacks in self.signal_callbacks.borrow_mut().iter_mut() {
            callbacks.clear();
        }
    }

    pub fn push_file_path(&self, path: PathBuf) {
        if let Some(state) = self.module_task_state() {
            state
                .push_current_file(path)
                .expect("module current-file scope identity exhausted");
            return;
        }
        self.current_file.borrow_mut().push(path);
    }

    pub fn pop_file_path(&self) {
        if let Some(state) = self.module_task_state() {
            state.pop_current_file();
            return;
        }
        self.current_file.borrow_mut().pop();
    }

    pub fn current_file_dir(&self) -> Option<PathBuf> {
        self.current_file_path()
            .and_then(|path| path.parent().map(|dir| dir.to_path_buf()))
    }

    pub fn current_file_path(&self) -> Option<PathBuf> {
        if let Some(state) = self.module_task_state() {
            return state.current_file();
        }
        self.current_file.borrow().last().cloned()
    }

    pub fn get_cached_module(&self, path: &PathBuf) -> Option<BTreeMap<String, Value>> {
        self.module_cache.borrow().get(path).cloned()
    }

    pub fn cache_module(&self, path: PathBuf, exports: BTreeMap<String, Value>) {
        self.module_cache.borrow_mut().insert(path, exports);
    }

    pub fn clear_module_cache(&self) {
        self.module_cache.borrow_mut().clear();
    }

    pub fn embedded_file_exists(&self, path: &PathBuf) -> bool {
        self.embedded_files.borrow().contains_key(path)
    }

    pub fn get_embedded_file(&self, path: &PathBuf) -> Option<Vec<u8>> {
        self.embedded_files.borrow().get(path).cloned()
    }

    pub fn set_embedded_file(&self, path: PathBuf, bytes: Vec<u8>) {
        self.embedded_files.borrow_mut().insert(path, bytes);
    }

    pub fn clear_embedded_files(&self) {
        self.embedded_files.borrow_mut().clear();
    }

    pub fn set_module_exports(&self, names: Vec<String>) {
        if let Some(state) = self.module_task_state() {
            state.set_current_exports(names);
            return;
        }
        let mut stack = self.module_exports.borrow_mut();
        if let Some(top) = stack.last_mut() {
            *top = Some(names);
        }
    }

    pub fn clear_module_exports(&self) {
        if let Some(state) = self.module_task_state() {
            state
                .push_exports(None)
                .expect("module export scope identity exhausted");
            return;
        }
        self.module_exports.borrow_mut().push(None);
    }

    pub fn take_module_exports(&self) -> Option<Vec<String>> {
        if let Some(state) = self.module_task_state() {
            return state.pop_exports().flatten();
        }
        self.module_exports.borrow_mut().pop().flatten()
    }

    /// Enter a module-load scope, guarding against import/load cycles. The
    /// returned [`ModuleLoadGuard`] pops the load stack when dropped, keeping it
    /// balanced on any exit path. Errors if `path` is already being loaded.
    pub fn enter_module_load(&self, path: PathBuf) -> Result<ModuleLoadGuard<'_>, SemaError> {
        let scope = self.begin_module_load(&path)?;
        Ok(ModuleLoadGuard { ctx: self, scope })
    }

    fn begin_module_load(&self, path: &PathBuf) -> Result<ModuleLoadScope, SemaError> {
        if let Some(state) = self.module_task_state() {
            check_module_cycle(&state.loading(), path)?;
            let scope = state
                .push_loading(path.clone())
                .map_err(|error| SemaError::eval(format!("module load scope: {error}")))?;
            return Ok(ModuleLoadScope::Task { state, scope });
        }
        check_module_cycle(&self.module_load_stack.borrow(), path)?;
        self.module_load_stack.borrow_mut().push(path.clone());
        Ok(ModuleLoadScope::Ambient(path.clone()))
    }

    fn end_module_load(&self, path: &PathBuf) {
        let mut stack = self.module_load_stack.borrow_mut();
        if matches!(stack.last(), Some(last) if last == path) {
            stack.pop();
        } else if let Some(pos) = stack.iter().rposition(|p| p == path) {
            stack.remove(pos);
        }
    }

    pub fn push_call_frame(&self, frame: CallFrame) {
        self.call_stack.borrow_mut().push(frame);
    }

    pub fn call_stack_depth(&self) -> usize {
        self.call_stack.borrow().len()
    }

    pub fn truncate_call_stack(&self, depth: usize) {
        self.call_stack.borrow_mut().truncate(depth);
    }

    pub fn capture_stack_trace(&self) -> StackTrace {
        let stack = self.call_stack.borrow();
        StackTrace(stack.iter().rev().cloned().collect())
    }

    pub fn merge_span_table(&self, spans: SpanMap) {
        let mut table = self.span_table.borrow_mut();
        if table.len() < MAX_SPAN_TABLE_ENTRIES {
            table.extend(spans);
        }
        // If table is full, skip merging new spans (preserves existing error locations)
    }

    pub fn lookup_span(&self, ptr: usize) -> Option<Span> {
        self.span_table.borrow().get(&ptr).cloned()
    }

    pub fn set_eval_step_limit(&self, limit: usize) {
        self.eval_step_limit.set(limit);
    }

    /// Set a wall-clock deadline after which evaluation should abort.
    /// Passing `None` clears any existing deadline.
    pub fn set_eval_deadline(&self, deadline: Option<Instant>) {
        self.eval_deadline.set(deadline);
    }

    /// Returns true if a deadline is set and has been exceeded.
    #[inline]
    pub fn deadline_exceeded(&self) -> bool {
        match self.eval_deadline.get() {
            Some(d) => Instant::now() >= d,
            None => false,
        }
    }

    /// Returns an `eval` error if a deadline is set and exceeded; otherwise Ok(()).
    #[inline]
    pub fn check_deadline(&self) -> Result<(), SemaError> {
        if self.deadline_exceeded() {
            Err(SemaError::eval(
                "evaluation exceeded time budget (looks like an infinite loop?)".to_string(),
            ))
        } else {
            Ok(())
        }
    }

    /// Per-iteration loop/recursion guard, called by the VM at loop back-edges
    /// and frame transitions. Counts a step and aborts when:
    ///   - the step limit is exceeded (wasm-safe runaway-loop guard — the wall
    ///     clock is unavailable in wasm, so the step counter is the guard there);
    ///   - the wall-clock deadline is exceeded (native);
    ///   - a cancellation has been requested (e.g. the playground Stop button).
    ///
    /// The step compare runs every call (cheap); the clock read and the
    /// cancellation thread-local read run only periodically to keep tight loops
    /// fast. `eval_steps` is reset per top-level eval by the evaluator.
    #[inline]
    pub fn check_loop_interrupt(&self) -> Result<(), SemaError> {
        let steps = self.eval_steps.get().wrapping_add(1);
        self.eval_steps.set(steps);
        let limit = self.eval_step_limit.get();
        if limit != 0 && steps > limit {
            return Err(SemaError::eval(
                "evaluation exceeded step limit (looks like an infinite loop?)".to_string(),
            ));
        }
        if steps & 0x3FFF == 0 {
            if self.deadline_exceeded() {
                return Err(SemaError::eval(
                    "evaluation exceeded time budget (looks like an infinite loop?)".to_string(),
                ));
            }
            if crate::async_signal::check_interrupt() {
                return Err(SemaError::eval("evaluation cancelled".to_string()));
            }
        }
        Ok(())
    }

    // --- User context methods ---

    pub fn context_get(&self, key: &Value) -> Option<Value> {
        if let Some(state) = self.dynamic_task_state() {
            return state.user_get(key);
        }
        let frames = self.user_context.borrow();
        for frame in frames.iter().rev() {
            if let Some(v) = frame.get(key) {
                return Some(v.clone());
            }
        }
        None
    }

    pub fn context_set(&self, key: Value, value: Value) {
        if let Some(state) = self.dynamic_task_state() {
            state.user_set(key, value);
            return;
        }
        let mut frames = self.user_context.borrow_mut();
        if let Some(top) = frames.last_mut() {
            top.insert(key, value);
        }
    }

    pub fn context_has(&self, key: &Value) -> bool {
        if let Some(state) = self.dynamic_task_state() {
            return state.user_get(key).is_some();
        }
        let frames = self.user_context.borrow();
        frames.iter().any(|frame| frame.contains_key(key))
    }

    pub fn context_remove(&self, key: &Value) -> Option<Value> {
        if let Some(state) = self.dynamic_task_state() {
            return state.user_remove(key);
        }
        let mut frames = self.user_context.borrow_mut();
        let mut first_found = None;
        for frame in frames.iter_mut().rev() {
            if let Some(v) = frame.remove(key) {
                if first_found.is_none() {
                    first_found = Some(v);
                }
            }
        }
        first_found
    }

    pub fn context_all(&self) -> BTreeMap<Value, Value> {
        if let Some(state) = self.dynamic_task_state() {
            return state.user_all();
        }
        let frames = self.user_context.borrow();
        let mut merged = BTreeMap::new();
        for frame in frames.iter() {
            for (k, v) in frame {
                merged.insert(k.clone(), v.clone());
            }
        }
        merged
    }

    pub fn context_push_frame(&self) {
        if let Some(state) = self.dynamic_task_state() {
            state
                .push_user_frame(BTreeMap::new())
                .expect("dynamic user-context scope identity exhausted");
            return;
        }
        self.user_context.borrow_mut().push(BTreeMap::new());
    }

    pub fn context_push_frame_with(&self, bindings: BTreeMap<Value, Value>) {
        if let Some(state) = self.dynamic_task_state() {
            state
                .push_user_frame(bindings)
                .expect("dynamic user-context scope identity exhausted");
            return;
        }
        self.user_context.borrow_mut().push(bindings);
    }

    pub fn context_pop_frame(&self) {
        if let Some(state) = self.dynamic_task_state() {
            state.pop_user_frame();
            return;
        }
        let mut frames = self.user_context.borrow_mut();
        if frames.len() > 1 {
            frames.pop();
        }
    }

    pub fn context_clear(&self) {
        if let Some(state) = self.dynamic_task_state() {
            state.user_clear();
            return;
        }
        let mut frames = self.user_context.borrow_mut();
        frames.clear();
        frames.push(BTreeMap::new());
    }

    // --- Hidden context methods ---

    pub fn hidden_get(&self, key: &Value) -> Option<Value> {
        if let Some(state) = self.dynamic_task_state() {
            return state.hidden_get(key);
        }
        let frames = self.hidden_context.borrow();
        for frame in frames.iter().rev() {
            if let Some(v) = frame.get(key) {
                return Some(v.clone());
            }
        }
        None
    }

    pub fn hidden_set(&self, key: Value, value: Value) {
        if let Some(state) = self.dynamic_task_state() {
            state.hidden_set(key, value);
            return;
        }
        let mut frames = self.hidden_context.borrow_mut();
        if let Some(top) = frames.last_mut() {
            top.insert(key, value);
        }
    }

    pub fn hidden_has(&self, key: &Value) -> bool {
        if let Some(state) = self.dynamic_task_state() {
            return state.hidden_get(key).is_some();
        }
        let frames = self.hidden_context.borrow();
        frames.iter().any(|frame| frame.contains_key(key))
    }

    pub fn hidden_push_frame(&self) {
        if let Some(state) = self.dynamic_task_state() {
            state
                .push_hidden_frame(BTreeMap::new())
                .expect("dynamic hidden-context scope identity exhausted");
            return;
        }
        self.hidden_context.borrow_mut().push(BTreeMap::new());
    }

    pub fn hidden_pop_frame(&self) {
        if let Some(state) = self.dynamic_task_state() {
            state.pop_hidden_frame();
            return;
        }
        let mut frames = self.hidden_context.borrow_mut();
        if frames.len() > 1 {
            frames.pop();
        }
    }

    // --- Stack methods ---

    /// Capture an independent root-authority snapshot of the interpreter-wide
    /// dynamic state. Stack entry identities are shared with the publication
    /// baseline so a stale root cannot remove an equal replacement entry.
    #[doc(hidden)]
    pub fn snapshot_dynamic_task_state(&self) -> DynamicTaskState {
        let user_frames = self.user_context.borrow().clone();
        let hidden_frames = self.hidden_context.borrow().clone();
        let (stacks, identities) = self.context_stacks.snapshot();
        DynamicTaskState::root_with_stack_identities(
            user_frames,
            hidden_frames,
            stacks,
            &identities,
        )
    }

    /// Capture the transient module-resolution scopes inherited by a new root.
    /// These scopes are task-private and never publish back at settlement.
    #[doc(hidden)]
    pub fn snapshot_module_task_state(&self) -> ModuleTaskState {
        ModuleTaskState::from_snapshot(
            self.current_file.borrow().clone(),
            self.module_load_stack.borrow().clone(),
            self.module_exports.borrow().clone(),
        )
    }

    /// Publish and drain one root snapshot's mutation journal. Child snapshots
    /// carry no journal and return `false` without changing interpreter state.
    #[doc(hidden)]
    pub fn publish_dynamic_task_state(&self, state: &DynamicTaskState) -> bool {
        let mut user_frames = self.user_context.borrow_mut();
        let mut hidden_frames = self.hidden_context.borrow_mut();
        let (mut stacks, mut identities) = self.context_stacks.borrow_parts_mut();
        assert!(
            identities.matches_stacks(&stacks),
            "dynamic stack identities must match their value entries"
        );
        let Some(mutations) = state.drain_mutations() else {
            return false;
        };
        apply_dynamic_mutations(
            &mut user_frames,
            &mut hidden_frames,
            &mut stacks,
            &mut identities,
            &mutations,
        );
        true
    }

    pub fn context_stack_push(&self, key: Value, value: Value) {
        if let Some(state) = self.dynamic_task_state() {
            state
                .stack_push(key, value)
                .expect("dynamic context-stack scope identity exhausted");
            return;
        }
        let (mut stacks, mut identities) = self.context_stacks.borrow_parts_mut();
        stacks.entry(key.clone()).or_default().push(value);
        identities.push(key);
    }

    pub fn context_stack_get(&self, key: &Value) -> Vec<Value> {
        if let Some(state) = self.dynamic_task_state() {
            return state.stack_get(key);
        }
        self.context_stacks
            .borrow()
            .get(key)
            .cloned()
            .unwrap_or_default()
    }

    pub fn context_stack_pop(&self, key: &Value) -> Option<Value> {
        if let Some(state) = self.dynamic_task_state() {
            return state.stack_pop(key);
        }
        let (mut stacks, mut identities) = self.context_stacks.borrow_parts_mut();
        let stack = stacks.get_mut(key)?;
        let val = stack.pop();
        if val.is_some() {
            assert!(
                identities.pop(key),
                "dynamic stack identity must have a matching value entry"
            );
        }
        if stack.is_empty() {
            stacks.remove(key);
            identities.remove(key);
        }
        val
    }
}

pub struct RuntimeQuantumGuard<'a> {
    ctx: &'a EvalContext,
    previous_thread_local: bool,
}

impl Drop for RuntimeQuantumGuard<'_> {
    fn drop(&mut self) {
        self.ctx.runtime_quantum_active.set(false);
        crate::set_runtime_quantum(self.previous_thread_local);
    }
}

/// Guard returned by [`EvalContext::suspend_runtime_quantum`]. TEMPORARY bridge
/// — restores the prior `runtime_quantum_active` value on `Drop`. Deleted with
/// the Task 04 `NativeOutcome::Call` migration (see `suspend_runtime_quantum`).
pub struct QuantumSuspendGuard<'a> {
    ctx: &'a EvalContext,
    previous_ctx: bool,
    previous_thread_local: bool,
}

impl QuantumSuspendGuard<'_> {
    fn with_thread_local_suspended(self) -> Self {
        crate::set_runtime_quantum(false);
        self
    }
}

impl Drop for QuantumSuspendGuard<'_> {
    fn drop(&mut self) {
        self.ctx.runtime_quantum_active.set(self.previous_ctx);
        crate::set_runtime_quantum(self.previous_thread_local);
    }
}

impl Default for EvalContext {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use crate::{Caps, Sandbox, Value};

    #[test]
    fn task_context_handle_lifecycle_and_child_inheritance() {
        let context = EvalContext::new();
        assert!(context.task_context().is_none());
        assert!(EvalContext::default().task_context().is_none());
        assert!(EvalContext::new_with_sandbox(Sandbox::deny(Caps::FS_READ))
            .task_context()
            .is_none());

        let handle = crate::runtime::TaskContextHandle::default();
        context.install_task_context(handle.clone());
        let clone = context.task_context().unwrap();
        clone
            .borrow_mut()
            .insert(std::rc::Rc::new(TestTaskLocal(4)));
        assert_eq!(handle.borrow().get::<TestTaskLocal>().unwrap().0, 4);

        let child = handle.inherit_for_child();
        assert_eq!(child.borrow().get::<TestTaskLocal>().unwrap().0, 0);
        assert_eq!(handle.borrow().get::<TestTaskLocal>().unwrap().0, 4);
        assert!(context.take_task_context().is_some());
        assert!(context.task_context().is_none());
    }

    #[test]
    fn installed_dynamic_state_is_accessible_while_task_context_is_borrowed() {
        let context = EvalContext::new();
        let handle = crate::runtime::TaskContextHandle::default();
        let dynamic = Rc::new(DynamicTaskState::root(
            vec![BTreeMap::new()],
            vec![BTreeMap::new()],
            BTreeMap::new(),
        ));
        handle.borrow_mut().insert(Rc::clone(&dynamic));
        let _scope = context.scope_task_context(handle.clone());

        let held_by_native_call = handle.borrow_mut();
        context.context_set(Value::keyword("key"), Value::int(42));
        assert_eq!(
            context.context_get(&Value::keyword("key")),
            Some(Value::int(42))
        );
        drop(held_by_native_call);

        assert_eq!(
            dynamic.user_get(&Value::keyword("key")),
            Some(Value::int(42))
        );
    }

    #[test]
    fn installed_module_state_is_accessible_while_task_context_is_borrowed() {
        let context = EvalContext::new();
        context.push_file_path(PathBuf::from("ambient/entry.sema"));
        let handle = crate::runtime::TaskContextHandle::default();
        let module = Rc::new(context.snapshot_module_task_state());
        handle.borrow_mut().insert(Rc::clone(&module));
        let _scope = context.scope_task_context(handle.clone());

        let held_by_native_call = handle.borrow_mut();
        assert_eq!(
            context.current_file_path(),
            Some(PathBuf::from("ambient/entry.sema"))
        );
        context.push_file_path(PathBuf::from("task/module.sema"));
        context.clear_module_exports();
        context.set_module_exports(vec!["answer".to_string()]);
        let load = context
            .enter_module_load(PathBuf::from("task/module.sema"))
            .expect("task-local module-load scope");

        assert_eq!(
            context.current_file_path(),
            Some(PathBuf::from("task/module.sema"))
        );
        assert_eq!(
            context.take_module_exports(),
            Some(vec!["answer".to_string()])
        );
        assert_eq!(module.loading(), vec![PathBuf::from("task/module.sema")]);
        drop(load);
        assert!(module.loading().is_empty());
        context.pop_file_path();
        assert_eq!(
            context.current_file_path(),
            Some(PathBuf::from("ambient/entry.sema"))
        );
        drop(held_by_native_call);

        assert_eq!(
            context.current_file.borrow().as_slice(),
            &[PathBuf::from("ambient/entry.sema")]
        );
        assert!(context.module_exports.borrow().is_empty());
        assert!(context.module_load_stack.borrow().is_empty());
    }

    #[test]
    fn module_load_guards_do_not_cross_task_states_with_colliding_scope_ids() {
        let context_a = EvalContext::new();
        let context_b = EvalContext::new();
        let state_a = Rc::new(ModuleTaskState::default());
        let state_b = Rc::new(ModuleTaskState::default());
        let handle_a = crate::runtime::TaskContextHandle::default();
        let handle_b = crate::runtime::TaskContextHandle::default();
        handle_a.borrow_mut().insert(Rc::clone(&state_a));
        handle_b.borrow_mut().insert(Rc::clone(&state_b));
        let _scope_a = context_a.scope_task_context(handle_a);
        let _scope_b = context_b.scope_task_context(handle_b);

        let guard_a = context_a
            .enter_module_load(PathBuf::from("same.sema"))
            .expect("task A load scope");
        let guard_b = context_b
            .enter_module_load(PathBuf::from("same.sema"))
            .expect("task B load scope");
        assert_eq!(state_a.loading(), state_b.loading());

        drop(guard_a);
        assert!(state_a.loading().is_empty());
        assert_eq!(state_b.loading(), vec![PathBuf::from("same.sema")]);
        drop(guard_b);
        assert!(state_b.loading().is_empty());
    }

    #[test]
    fn dynamic_snapshot_publication_rejects_an_equal_value_aba_pop() {
        let context = EvalContext::new();
        let key = Value::keyword("stack");
        let value = Value::keyword("same");
        context.context_stack_push(key.clone(), value.clone());
        let stale = context.snapshot_dynamic_task_state();
        let recreating = context.snapshot_dynamic_task_state();

        assert_eq!(stale.stack_pop(&key), Some(value.clone()));
        assert_eq!(recreating.stack_pop(&key), Some(value.clone()));
        recreating
            .stack_push(key.clone(), value.clone())
            .expect("scope ID available");

        assert!(context.publish_dynamic_task_state(&recreating));
        assert!(context.publish_dynamic_task_state(&stale));
        assert_eq!(context.context_stack_get(&key), vec![value]);
    }

    #[test]
    fn legacy_stack_mutation_renews_identity_seen_by_later_snapshots() {
        let context = EvalContext::new();
        let key = Value::keyword("stack");
        let value = Value::keyword("same");
        context.context_stack_push(key.clone(), value.clone());
        let stale = context.snapshot_dynamic_task_state();
        assert_eq!(stale.stack_pop(&key), Some(value.clone()));

        assert_eq!(context.context_stack_pop(&key), Some(value.clone()));
        context.context_stack_push(key.clone(), value.clone());
        assert!(context.publish_dynamic_task_state(&stale));

        assert_eq!(context.context_stack_get(&key), vec![value]);
    }

    #[test]
    fn direct_equal_stack_replacement_invalidates_stale_snapshot_identity() {
        let context = EvalContext::new();
        let key = Value::keyword("stack");
        let value = Value::keyword("same");
        context.context_stack_push(key.clone(), value.clone());
        let stale = context.snapshot_dynamic_task_state();
        assert_eq!(stale.stack_pop(&key), Some(value.clone()));

        context
            .context_stacks
            .borrow_mut()
            .insert(key.clone(), vec![value.clone()]);
        assert!(context.publish_dynamic_task_state(&stale));

        assert_eq!(context.context_stack_get(&key), vec![value]);
    }

    #[test]
    fn popping_a_direct_empty_stack_keeps_publication_sidecar_aligned() {
        let context = EvalContext::new();
        let key = Value::keyword("empty-stack");
        context
            .context_stacks
            .borrow_mut()
            .insert(key.clone(), Vec::new());
        let root = context.snapshot_dynamic_task_state();
        root.user_set(Value::keyword("published"), Value::int(1));

        assert_eq!(context.context_stack_pop(&key), None);
        assert!(context.publish_dynamic_task_state(&root));
        assert_eq!(
            context.context_get(&Value::keyword("published")),
            Some(Value::int(1))
        );
    }

    #[test]
    fn publication_borrow_conflict_leaves_the_root_journal_retryable() {
        let context = EvalContext::new();
        let key = Value::keyword("retry");
        let root = context.snapshot_dynamic_task_state();
        root.user_set(key.clone(), Value::int(42));
        let held = context.user_context.borrow_mut();

        let conflicted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            context.publish_dynamic_task_state(&root)
        }));
        assert!(conflicted.is_err());
        drop(held);

        assert!(context.publish_dynamic_task_state(&root));
        assert_eq!(context.context_get(&key), Some(Value::int(42)));
    }

    #[test]
    fn scoped_task_context_restores_the_exact_handle_after_panic() {
        let context = EvalContext::new();
        let outer = crate::runtime::TaskContextHandle::default();
        let inner = crate::runtime::TaskContextHandle::default();
        outer
            .borrow_mut()
            .insert(std::rc::Rc::new(TestTaskLocal(1)));
        inner
            .borrow_mut()
            .insert(std::rc::Rc::new(TestTaskLocal(2)));
        context.install_task_context(outer);

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _scope = context.scope_task_context(inner.clone());
            assert_eq!(
                context
                    .task_context()
                    .unwrap()
                    .borrow()
                    .get::<TestTaskLocal>()
                    .unwrap()
                    .0,
                2
            );
            panic!("test unwind");
        }));

        assert!(result.is_err());
        assert_eq!(
            context
                .task_context()
                .unwrap()
                .borrow()
                .get::<TestTaskLocal>()
                .unwrap()
                .0,
            1
        );
    }

    #[test]
    fn runtime_quantum_guard_restores_outer_context_thread_local() {
        let outer = EvalContext::new();
        let inner = EvalContext::new();
        assert!(!crate::in_runtime_quantum());

        let outer_guard = outer.enter_runtime_quantum().unwrap();
        assert!(outer.runtime_quantum_active());
        assert!(crate::in_runtime_quantum());

        {
            let _inner_guard = inner.enter_runtime_quantum().unwrap();
            assert!(inner.runtime_quantum_active());
            assert!(crate::in_runtime_quantum());
        }

        assert!(outer.runtime_quantum_active());
        assert!(crate::in_runtime_quantum());
        drop(outer_guard);
        assert!(!outer.runtime_quantum_active());
        assert!(!crate::in_runtime_quantum());
    }

    struct TestTaskLocal(u32);

    impl crate::runtime::Trace for TestTaskLocal {
        fn trace(&self, _sink: &mut dyn FnMut(crate::cycle::GcEdge<'_>)) -> bool {
            true
        }
    }

    impl crate::runtime::TaskLocalValue for TestTaskLocal {
        fn inherit(&self) -> std::rc::Rc<dyn crate::runtime::TaskLocalValue> {
            std::rc::Rc::new(Self(0))
        }

        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    // --- File path tracking ---

    #[test]
    fn test_push_pop_file_path() {
        let ctx = EvalContext::new();
        let path = PathBuf::from("/foo/bar/baz.sema");
        ctx.push_file_path(path.clone());
        assert_eq!(ctx.current_file_path(), Some(path));
        ctx.pop_file_path();
        assert_eq!(ctx.current_file_path(), None);
    }

    #[test]
    fn test_current_file_dir() {
        let ctx = EvalContext::new();
        ctx.push_file_path(PathBuf::from("/foo/bar/baz.sema"));
        assert_eq!(ctx.current_file_dir(), Some(PathBuf::from("/foo/bar")));
    }

    #[test]
    fn test_current_file_dir_empty() {
        let ctx = EvalContext::new();
        assert_eq!(ctx.current_file_dir(), None);
    }

    #[test]
    fn test_nested_file_paths() {
        let ctx = EvalContext::new();
        let first = PathBuf::from("/a/first.sema");
        let second = PathBuf::from("/b/second.sema");
        ctx.push_file_path(first.clone());
        ctx.push_file_path(second.clone());
        assert_eq!(ctx.current_file_path(), Some(second));
        ctx.pop_file_path();
        assert_eq!(ctx.current_file_path(), Some(first));
    }

    // --- Module caching ---

    #[test]
    fn test_cache_module() {
        let ctx = EvalContext::new();
        let path = PathBuf::from("/lib/math.sema");
        let mut exports = BTreeMap::new();
        exports.insert("add".to_string(), Value::int(1));
        ctx.cache_module(path.clone(), exports.clone());
        let cached = ctx.get_cached_module(&path).unwrap();
        assert_eq!(cached.len(), 1);
        assert_eq!(cached.get("add"), Some(&Value::int(1)));
    }

    #[test]
    fn test_get_cached_module_miss() {
        let ctx = EvalContext::new();
        let path = PathBuf::from("/nonexistent.sema");
        assert_eq!(ctx.get_cached_module(&path), None);
    }

    #[test]
    fn test_cache_module_overwrites() {
        let ctx = EvalContext::new();
        let path = PathBuf::from("/lib/math.sema");

        let mut first = BTreeMap::new();
        first.insert("old".to_string(), Value::int(1));
        ctx.cache_module(path.clone(), first);

        let mut second = BTreeMap::new();
        second.insert("new".to_string(), Value::int(2));
        ctx.cache_module(path.clone(), second);

        let cached = ctx.get_cached_module(&path).unwrap();
        assert!(!cached.contains_key("old"));
        assert_eq!(cached.get("new"), Some(&Value::int(2)));
    }

    // --- Module exports ---

    #[test]
    fn test_module_exports_roundtrip() {
        let ctx = EvalContext::new();
        ctx.clear_module_exports(); // pushes None onto stack
        ctx.set_module_exports(vec!["foo".to_string(), "bar".to_string()]);
        let taken = ctx.take_module_exports();
        assert_eq!(taken, Some(vec!["foo".to_string(), "bar".to_string()]));
    }

    #[test]
    fn test_take_module_exports_empty() {
        let ctx = EvalContext::new();
        // Nothing has been pushed, so take should return None
        assert_eq!(ctx.take_module_exports(), None);
    }

    // --- Cyclic import detection ---

    #[test]
    fn test_begin_module_load_ok() {
        let ctx = EvalContext::new();
        let path = PathBuf::from("/lib/a.sema");
        assert!(ctx.begin_module_load(&path).is_ok());
    }

    #[test]
    fn test_begin_module_load_cycle() {
        let ctx = EvalContext::new();
        let path = PathBuf::from("/lib/a.sema");
        ctx.begin_module_load(&path).unwrap();
        let result = ctx.begin_module_load(&path);
        assert!(result.is_err());
        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("cyclic import"),
            "error should mention cyclic import: {msg}"
        );
    }

    #[test]
    fn test_end_module_load() {
        let ctx = EvalContext::new();
        let path = PathBuf::from("/lib/a.sema");
        ctx.begin_module_load(&path).unwrap();
        ctx.end_module_load(&path);
        // Stack is now empty, so beginning the same path again should succeed
        assert!(ctx.begin_module_load(&path).is_ok());
    }

    #[test]
    fn test_nested_module_loads() {
        let ctx = EvalContext::new();
        let a = PathBuf::from("/lib/a.sema");
        let b = PathBuf::from("/lib/b.sema");
        ctx.begin_module_load(&a).unwrap();
        ctx.begin_module_load(&b).unwrap();
        ctx.end_module_load(&b);
        // A should still be in the stack — beginning A again should fail
        let result = ctx.begin_module_load(&a);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("cyclic import"),
            "A should still be loading: {msg}"
        );
    }

    // --- Sandbox integration ---

    #[test]
    fn test_new_with_sandbox() {
        let sandbox = Sandbox::deny(Caps::NETWORK);
        let ctx = EvalContext::new_with_sandbox(sandbox);
        // Verify the sandbox is set by checking a denied capability
        let result = ctx.sandbox.check(Caps::NETWORK, "http/get");
        assert!(result.is_err());
        // Allowed capability should pass
        let result = ctx.sandbox.check(Caps::FS_READ, "file/read");
        assert!(result.is_ok());
    }
}

thread_local! {
    static STDLIB_CTX: EvalContext = EvalContext::new();
}

/// Get a reference to the shared stdlib EvalContext.
/// Use this for stdlib callback invocations instead of creating throwaway contexts.
pub fn with_stdlib_ctx<F, R>(f: F) -> R
where
    F: FnOnce(&EvalContext) -> R,
{
    STDLIB_CTX.with(f)
}

/// Register the full evaluator callback. Called by `sema-eval` during interpreter init.
/// Stores into both `ctx` and the shared `STDLIB_CTX` so that stdlib simple-fn closures
/// (which lack a ctx parameter) can still invoke the evaluator.
pub fn set_eval_callback(ctx: &EvalContext, f: EvalCallbackFn) {
    ctx.eval_fn.set(Some(f));
    STDLIB_CTX.with(|stdlib| stdlib.eval_fn.set(Some(f)));
}

/// Register the call-value callback. Called by `sema-eval` during interpreter init.
/// Stores into both `ctx` and the shared `STDLIB_CTX`.
pub fn set_call_callback(ctx: &EvalContext, f: CallCallbackFn) {
    ctx.call_fn.set(Some(f));
    STDLIB_CTX.with(|stdlib| stdlib.call_fn.set(Some(f)));
}

/// Evaluate an expression using the registered evaluator.
/// Returns an error if no evaluator has been registered.
pub fn eval_callback(ctx: &EvalContext, expr: &Value, env: &Env) -> Result<Value, SemaError> {
    let f = ctx.eval_fn.get().ok_or_else(|| {
        SemaError::eval("eval callback not registered — Interpreter::new() must be called first")
    })?;
    f(ctx, expr, env)
}

/// Call a function value with arguments using the registered callback.
/// Returns an error if no callback has been registered.
pub fn call_callback(ctx: &EvalContext, func: &Value, args: &[Value]) -> Result<Value, SemaError> {
    let f = ctx.call_fn.get().ok_or_else(|| {
        SemaError::eval("call callback not registered — Interpreter::new() must be called first")
    })?;
    f(ctx, func, args)
}

/// Register the owned-args call callback. Called by `sema-eval` during
/// interpreter init, alongside [`set_call_callback`].
pub fn set_call_owned_callback(ctx: &EvalContext, f: CallOwnedCallbackFn) {
    ctx.call_owned_fn.set(Some(f));
    STDLIB_CTX.with(|stdlib| stdlib.call_owned_fn.set(Some(f)));
}

/// Call a function value with an args buffer the CALLER owns and will not
/// reuse after the call: the callee may move values out of it (leaving nil
/// behind). This is the refcount-shedding variant of [`call_callback`] — a
/// fold-style accumulator handed off this way stays uniquely owned across the
/// callback boundary, so the stdlib's `strong_count == 1` in-place fast paths
/// (e.g. `assoc` on a map) can fire inside the callback. Falls back to the
/// borrowed protocol (args intact) when no owned callback is registered.
pub fn call_callback_owned(
    ctx: &EvalContext,
    func: &Value,
    args: &mut [Value],
) -> Result<Value, SemaError> {
    if let Some(f) = ctx.call_owned_fn.get() {
        return f(ctx, func, args);
    }
    call_callback(ctx, func, args)
}
