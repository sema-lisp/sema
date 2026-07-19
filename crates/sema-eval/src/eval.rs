use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::{Rc, Weak};

use sema_core::{
    intern, resolve, Env, EvalContext, Macro, MultiMethod, NativeFn, SemaError, Spur, Thunk, Value,
    ValueView,
};

use crate::special_forms;

/// Trampoline for tail-call optimization.
pub enum Trampoline {
    Value(Value),
    Eval(Value, Env),
}

pub type EvalResult = Result<Value, SemaError>;

/// Create an isolated module env: child of root (global/stdlib) env
pub fn create_module_env(env: &Env) -> Env {
    // Walk parent chain to find root
    let mut current = env.clone();
    loop {
        let parent = current.parent.clone();
        match parent {
            Some(p) => current = (*p).clone(),
            None => break,
        }
    }
    Env::with_parent(Rc::new(current))
}

/// Collect the names of all native functions in an environment.
/// Used to tell the bytecode compiler which globals can use CallNative.
fn collect_native_names(env: &Env) -> HashSet<Spur> {
    // A "known native" tells the compiler it may emit a direct native-call for
    // that global. Prelude functions are VM closures *wrapped* in a `NativeFn`
    // (a `VmClosurePayload`), so `is_native_fn()` is true for them too — but they
    // must be CALLED IN-VM (as a bytecode frame), not through the `NativeFn`
    // wrapper: the wrapper's synchronous nested-run path suspends the runtime
    // quantum and turns any `async/spawn`/`await`/channel yield inside the
    // closure into "async yield outside of scheduler context". Exclude VM
    // closures so a call to a prelude function (e.g. the owned-concurrency
    // helpers `__spawn-thunks`/`__owned-all`) dispatches through the ordinary
    // VM-closure path, keeping its yields on the scheduler.
    env.all_names()
        .into_iter()
        .filter(|&spur| {
            env.get(spur)
                .is_some_and(|v| v.is_native_fn() && sema_vm::extract_vm_closure(&v).is_none())
        })
        .collect()
}

/// The interpreter holds the global environment and state.
pub struct Interpreter {
    pub global_env: Rc<Env>,
    /// Shared evaluation context. Held behind an `Rc` so the unified runtime can
    /// share the *same* context the interpreter registered its eval/call
    /// callbacks and module cache onto (see `run_exprs_via_runtime`); a fresh
    /// context would route the VM's `call_callback` through unregistered
    /// callbacks and an empty module cache. All fields are interior-mutable
    /// (`RefCell`/`Cell`), so shared `&` access is sufficient for mutation.
    pub ctx: Rc<EvalContext>,
    /// Single, persistent unified runtime, constructed once and shared across
    /// every runtime-backed eval (`eval_via_runtime`/`eval_str_via_runtime`).
    /// Each call submits a fresh ROOT to this same runtime, so detached
    /// `async/spawn` tasks, timers, promises and channels survive *between*
    /// top-level evals (a per-call runtime rebuilt that state every time and
    /// silently dropped anything not settled within one call). `Option` so
    /// `Drop` can `take()` and shut it down BEFORE the global-env teardown
    /// collection (see `Drop`); it is always `Some` outside of `Drop`.
    runtime: Option<Runtime>,
}

use sema_vm::runtime::Runtime;

/// Build the interpreter-owned persistent runtime that shares `ctx`. Uses the
/// real thread-pool executor so genuinely-blocking external operations submitted
/// from within a runtime quantum run off the VM thread and overlap; the drive
/// loop (`run_exprs_via_runtime`) services their completions by block-waiting on
/// the runtime inbox. Fresh runtime construction does not fail in practice, so
/// this is infallible — a failure would mean the wait-runtime could not allocate
/// identity, which only happens under a corrupt/exhausted global counter.
fn build_runtime(ctx: &Rc<EvalContext>) -> Runtime {
    // Native builds attach to THE process-wide tokio pool (via sema-io, which
    // hides the tokio edge): its async tier runs `reqwest`/`tokio::process`
    // futures on a real reactor and its blocking tier is admission-controlled, so
    // concurrent external ops overlap without a per-op worker ceiling.
    #[cfg(not(target_arch = "wasm32"))]
    let executor = sema_io::process_executor();
    // wasm32-unknown-unknown has no OS threads, so the default runtime cannot
    // construct a ThreadPoolExecutor. NullExecutor rejects External waits; a
    // browser host that supports them must use [`from_parts_with_executor`]
    // with an event-loop-backed IoExecutor, as `sema-wasm` does.
    #[cfg(target_arch = "wasm32")]
    let executor: std::sync::Arc<dyn sema_core::runtime::IoExecutor> =
        std::sync::Arc::new(sema_vm::runtime::NullExecutor);
    build_runtime_with(ctx, executor)
}

fn build_runtime_with(
    ctx: &Rc<EvalContext>,
    executor: std::sync::Arc<dyn sema_core::runtime::IoExecutor>,
) -> Runtime {
    use sema_vm::runtime::MonotonicClock;
    Runtime::new(Rc::clone(ctx), Rc::new(MonotonicClock), executor)
        .expect("fresh unified runtime construction cannot fail")
}

fn drive_runtime_root(
    runtime: &sema_vm::runtime::Runtime,
    budget: &sema_vm::runtime::DriveBudget,
    root: sema_core::runtime::RootId,
) -> Result<sema_vm::runtime::DriveState, sema_vm::runtime::RuntimeFault> {
    #[cfg(target_arch = "wasm32")]
    {
        runtime.drive_roots(budget, &[root])
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = root;
        runtime.drive(budget)
    }
}

impl Default for Interpreter {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Interpreter {
    fn drop(&mut self) {
        // Skip the teardown collection while unwinding: a panic anywhere in
        // the collector would be a panic-in-destructor-during-cleanup, which
        // aborts the whole process instead of unwinding. Nothing is lost —
        // the candidates stay registered, so the next safe point on this
        // thread reclaims the env. Signal leases are the exception: restoring
        // a process disposition is non-evaluating and must not depend on a
        // later GC. The persistent runtime field is left in place; its own
        // `Drop` (`close_for_interpreter_drop`) cancels every task and closes
        // the inbox WITHOUT driving any VM quantum, so it is bounded and
        // panic-safe (no re-entrant evaluation while unwinding).
        if std::thread::panicking() {
            let _ = self.ctx.try_run_signal_teardown_hooks();
            return;
        }
        // Tear down the persistent unified runtime BEFORE the global-env
        // teardown collection below. Its task VMs / promises / channels hold
        // `Rc<Env>` and `Value` edges (a still-parked/detached `async/spawn`
        // task keeps a whole VM alive); if those outlive the collect, the env
        // stays externally referenced and trial deletion frees nothing. A
        // BOUNDED `shutdown` (finite deadline + host drive budget) cancels and
        // reaps all tasks — it can never hang — and dropping the runtime then
        // releases its state so the collection reclaims the env.
        if let Some(runtime) = self.runtime.take() {
            let options = sema_vm::runtime::ShutdownOptions {
                // `web_time::Instant`, not `std::time::Instant`: on wasm32 the
                // latter's `now()` panics (see sema-vm's runtime module,
                // which `ShutdownOptions` belongs to); `web_time` is a
                // transparent re-export of `std::time` everywhere else.
                deadline: web_time::Instant::now() + std::time::Duration::from_secs(2),
                drive_budget: sema_vm::runtime::DriveBudget::host_default(),
            };
            let _ = runtime.shutdown(&options);
            drop(runtime);
        }
        // Process signal leases belong to the interpreter lifetime, not the
        // reachability of its global env. Embedders may retain `global_env`, so
        // run the registry's weak teardown hooks before clearing Value roots.
        let _ = self.ctx.try_run_signal_teardown_hooks();
        // `self.ctx` outlives this Drop body (fields drop after it), and its
        // caches hold Values: module-cache export closures keep their module
        // envs — and via the parent chain the ENTIRE global env — externally
        // referenced, so with them held the teardown collect would free
        // nothing. Clear every ctx-held Value store first.
        self.ctx.module_cache.borrow_mut().clear();
        self.ctx.user_context.borrow_mut().clear();
        self.ctx.hidden_context.borrow_mut().clear();
        self.ctx.context_stacks.borrow_mut().clear();
        self.ctx.clear_signal_callbacks();
        // Release this interpreter's own strong ref to the env BEFORE the
        // teardown collection: with it held, the env wrapper carries an
        // external count and trial deletion (correctly) keeps the whole env.
        // Once released, the only refs left are the Env⇄Closure cycle edges
        // from top-level `define`s, which the collector severs. No pins — the
        // dying env is exactly what must be traced. Anything still externally
        // held (e.g. a user-kept `global_env` clone) survives without retaining
        // process signal ownership.
        drop(std::mem::replace(&mut self.global_env, Rc::new(Env::new())));
        sema_core::gc_collect(&[], sema_core::GcTrigger::InterpreterDrop);
    }
}

impl Interpreter {
    pub fn new() -> Self {
        let (global_env, ctx) = Self::new_parts();
        Self::from_parts(global_env, ctx)
    }

    /// Like [`new`](Self::new), but the caller supplies the runtime's
    /// `IoExecutor` (see [`from_parts_with_executor`](Self::from_parts_with_executor))
    /// instead of the platform default. The one-shot constructor sema-wasm
    /// needs: everything `new()` sets up (stdlib, callbacks, prelude), but
    /// wired to a real external-wait tier instead of wasm32's default
    /// `NullExecutor`.
    pub fn new_with_executor(executor: std::sync::Arc<dyn sema_core::runtime::IoExecutor>) -> Self {
        let (global_env, ctx) = Self::new_parts();
        Self::from_parts_with_executor(global_env, ctx, executor)
    }

    /// Build the (global env, ctx) pair `new`/`new_with_executor` share —
    /// stdlib registration, eval/call callbacks, LLM builtins (native only),
    /// VM delegates, and the prelude — everything short of picking the
    /// runtime's executor.
    fn new_parts() -> (Rc<Env>, Rc<EvalContext>) {
        let env = Env::new();
        let ctx = EvalContext::new();
        // Register eval/call callbacks so stdlib can invoke the real evaluator
        sema_core::set_eval_callback(&ctx, eval_value_vm);
        sema_core::set_call_callback(&ctx, call_value);
        sema_core::set_call_owned_callback(&ctx, call_value_owned);
        // Register stdlib
        sema_stdlib::register_stdlib(&env, &sema_core::Sandbox::allow_all());
        // Register LLM builtins
        #[cfg(not(target_arch = "wasm32"))]
        {
            sema_llm::builtins::reset_runtime_state();
            sema_llm::builtins::register_llm_builtins(&env, &sema_core::Sandbox::allow_all());
        }
        let global_env = Rc::new(env);
        let ctx = Rc::new(ctx);
        register_vm_delegates(&global_env, &ctx);
        load_prelude(&ctx, &global_env);
        (global_env, ctx)
    }

    pub fn new_with_sandbox(sandbox: &sema_core::Sandbox) -> Self {
        let env = Env::new();
        let ctx = EvalContext::new_with_sandbox(sandbox.clone());
        sema_core::set_eval_callback(&ctx, eval_value_vm);
        sema_core::set_call_callback(&ctx, call_value);
        sema_core::set_call_owned_callback(&ctx, call_value_owned);
        sema_stdlib::register_stdlib(&env, sandbox);
        #[cfg(not(target_arch = "wasm32"))]
        {
            sema_llm::builtins::reset_runtime_state();
            sema_llm::builtins::register_llm_builtins(&env, sandbox);
        }
        let global_env = Rc::new(env);
        let ctx = Rc::new(ctx);
        register_vm_delegates(&global_env, &ctx);
        load_prelude(&ctx, &global_env);
        Self::from_parts(global_env, ctx)
    }

    /// Assemble an interpreter from an already-populated global env + context,
    /// constructing the persistent runtime once. Embedders that build the env
    /// and context by hand (the `sema` crate's builder, the wasm bindings) MUST
    /// go through here rather than the struct literal so they get the runtime.
    pub fn from_parts(global_env: Rc<Env>, ctx: Rc<EvalContext>) -> Self {
        let runtime = build_runtime(&ctx);
        Interpreter {
            global_env,
            ctx,
            runtime: Some(runtime),
        }
    }

    /// Like [`from_parts`](Self::from_parts), but the caller supplies the
    /// runtime's [`IoExecutor`](sema_vm::runtime::IoExecutor) instead of the
    /// platform default `build_runtime` picks. The escape hatch a host needs
    /// when the default executor is wrong for its target — e.g. wasm32's
    /// default is a `NullExecutor` (no real threads to run one on), but a
    /// browser host that wants a working `WaitKind::External` tier (real
    /// `fetch`/timer completions delivered from JS callbacks, not an OS
    /// thread pool) supplies its own `IoExecutor` here instead.
    pub fn from_parts_with_executor(
        global_env: Rc<Env>,
        ctx: Rc<EvalContext>,
        executor: std::sync::Arc<dyn sema_core::runtime::IoExecutor>,
    ) -> Self {
        let runtime = build_runtime_with(&ctx, executor);
        Interpreter {
            global_env,
            ctx,
            runtime: Some(runtime),
        }
    }

    /// Evaluate a single expression on the VM. M6: the VM is the sole evaluator.
    ///
    /// NOTE (deliberate behavior change): all eval
    /// entry points now run in the global env, so top-level `define`s persist
    /// across calls. The old `eval`/`eval_str` child-env isolation is gone —
    /// maintaining two env semantics was the dual-evaluator complexity being
    /// removed. Use a fresh `Interpreter` for an isolated evaluation.
    pub fn eval(&self, expr: &Value) -> EvalResult {
        self.eval_in_global(expr)
    }

    /// Evaluate a synchronous expression through the unified cooperative runtime
    /// — the Task 03/04 integration toe-hold. Compiles against this
    /// interpreter's globals, submits a real VM-backed root, and drives it to
    /// settlement.
    ///
    /// Only synchronous programs are supported here: the runtime is built with a
    /// `NullExecutor`, so a program that suspends on I/O cannot progress and is
    /// reported as an error. Async programs still use `eval`. A single
    /// interpreter-owned, shared-context runtime (`ctx: Rc<EvalContext>` with
    /// proper drop ordering, constructed once) is the next integration slice.
    pub fn eval_via_runtime(&self, expr: &Value) -> EvalResult {
        self.run_exprs_via_runtime(std::slice::from_ref(expr))
    }

    /// Parse a whole program (one or more top-level forms) and evaluate it as a
    /// single VM-backed root on the unified runtime. Mirrors
    /// [`eval_str_in_global`](Self::eval_str_in_global) but drives through the
    /// runtime; `define`s land in and persist across this interpreter's globals.
    ///
    /// Like [`eval_via_runtime`](Self::eval_via_runtime), only synchronous
    /// programs are supported (the runtime uses a `NullExecutor`).
    pub fn eval_str_via_runtime(&self, input: &str) -> EvalResult {
        let (exprs, spans) = sema_reader::read_many_with_spans(input)?;
        self.ctx.merge_span_table(spans);
        if exprs.is_empty() {
            return Ok(Value::nil());
        }
        self.run_exprs_via_runtime(&exprs)
    }

    /// Macro-expand, compile, and drive a sequence of top-level forms as one
    /// root on the unified runtime. Shared by the runtime eval entry points.
    fn run_exprs_via_runtime(&self, exprs: &[Value]) -> EvalResult {
        self.ensure_synchronous_runtime_entry_allowed()?;
        let vm = self.build_vm_for_exprs(exprs)?;
        self.drive_vm_on_runtime(vm)
    }

    fn ensure_synchronous_runtime_entry_allowed(&self) -> Result<(), SemaError> {
        #[cfg(target_arch = "wasm32")]
        if self.runtime().is_debug_paused() {
            return Err(SemaError::eval(
                "synchronous WebAssembly evaluation cannot run while a debugger is paused",
            )
            .with_hint("Resume or stop the debugger before starting a synchronous evaluation"));
        }
        Ok(())
    }

    /// Macro-expand and compile a sequence of top-level forms into a
    /// seeded-but-not-yet-submitted VM. Shared by every entry point that
    /// builds a root from source/data forms — [`run_exprs_via_runtime`],
    /// [`submit_str`](Self::submit_str), and
    /// [`submit_value`](Self::submit_value).
    fn build_vm_for_exprs(&self, exprs: &[Value]) -> Result<sema_vm::VM, SemaError> {
        let expanded = expand_for_vm_batch(&self.ctx, &self.global_env, exprs)?;
        let known_natives = collect_native_names(&self.global_env);
        let span_map = self.ctx.span_table.borrow().clone();
        let prog = sema_vm::compile_program_with_spans_and_natives(
            &expanded,
            &span_map,
            None,
            Some(known_natives),
        )?;
        let mut vm = sema_vm::VM::new(
            self.global_env.clone(),
            prog.functions,
            &prog.native_table,
            prog.main_cache_slots,
        )?;
        vm.seed_main_frame(prog.closure);
        Ok(vm)
    }

    /// Parse `src` (one or more top-level forms), compile it against this
    /// interpreter's globals, and submit it as a fresh root on the
    /// interpreter's persistent runtime — WITHOUT driving it. Pair with
    /// [`drive_until_settled`](Self::drive_until_settled) or repeated
    /// [`drive_turn`](Self::drive_turn) calls to run it. `define`s land in
    /// the global env only once the root actually runs (driving is what
    /// executes the program), exactly as for [`eval_str`](Self::eval_str).
    pub fn submit_str(
        &self,
        src: &str,
        opts: sema_vm::runtime::RootOptions,
    ) -> Result<sema_vm::runtime::RootHandle, SemaError> {
        self.submit_str_guarded(src, opts, || Ok(()))
    }

    /// Parse and compile `src`, then run `before_submit` immediately before
    /// creating its runtime root. Hosts whose macro expansion can re-enter
    /// user callbacks use this seam to revalidate admission after every
    /// user-code-capable preparation step without holding host state across
    /// expansion.
    pub fn submit_str_guarded<F>(
        &self,
        src: &str,
        opts: sema_vm::runtime::RootOptions,
        before_submit: F,
    ) -> Result<sema_vm::runtime::RootHandle, SemaError>
    where
        F: FnOnce() -> Result<(), SemaError>,
    {
        let (exprs, spans) = sema_reader::read_many_with_spans(src)?;
        self.ctx.merge_span_table(spans);
        self.submit_exprs_guarded(&exprs, opts, before_submit)
    }

    /// Compile an already-parsed expression against this interpreter's
    /// globals and submit it as a fresh root — WITHOUT driving it. See
    /// [`submit_str`](Self::submit_str).
    pub fn submit_value(
        &self,
        expr: Value,
        opts: sema_vm::runtime::RootOptions,
    ) -> Result<sema_vm::runtime::RootHandle, SemaError> {
        self.submit_exprs(std::slice::from_ref(&expr), opts)
    }

    /// Build a root from a deserialized `.semac` program and submit it to this
    /// interpreter's persistent runtime without driving it. The program uses
    /// this interpreter's global environment, so top-level definitions become
    /// visible to later evaluations once the host drives the root.
    ///
    /// Nested module loading continues to use [`execute_compile_result`], whose
    /// synchronous execution boundary is intentionally separate from this host
    /// API.
    pub fn submit_compile_result(
        &self,
        result: sema_vm::CompileResult,
        opts: sema_vm::runtime::RootOptions,
    ) -> Result<sema_vm::runtime::RootHandle, SemaError> {
        let (mut vm, closure) = build_vm_for_compile_result(Rc::clone(&self.global_env), result)?;
        vm.seed_main_frame(closure);
        let runtime = self
            .runtime
            .as_ref()
            .expect("runtime is present outside of Drop");
        runtime
            .submit_root_with_options(vm, &opts)
            .map_err(|e| SemaError::eval(format!("root submission failed: {e:?}")))
    }

    fn submit_exprs(
        &self,
        exprs: &[Value],
        opts: sema_vm::runtime::RootOptions,
    ) -> Result<sema_vm::runtime::RootHandle, SemaError> {
        self.submit_exprs_guarded(exprs, opts, || Ok(()))
    }

    fn submit_exprs_guarded<F>(
        &self,
        exprs: &[Value],
        opts: sema_vm::runtime::RootOptions,
        before_submit: F,
    ) -> Result<sema_vm::runtime::RootHandle, SemaError>
    where
        F: FnOnce() -> Result<(), SemaError>,
    {
        let nil_placeholder = [Value::nil()];
        let exprs = if exprs.is_empty() {
            &nil_placeholder[..]
        } else {
            exprs
        };
        let vm = self.build_vm_for_exprs(exprs)?;
        before_submit()?;
        let runtime = self
            .runtime
            .as_ref()
            .expect("runtime is present outside of Drop");
        runtime
            .submit_root_with_options(vm, &opts)
            .map_err(|e| SemaError::eval(format!("root submission failed: {e:?}")))
    }

    /// Drive an already-submitted root (from [`submit_str`](Self::submit_str)
    /// / [`submit_value`](Self::submit_value)) to settlement. Shares the
    /// exact drive-loop semantics of [`drive_vm_on_runtime`](Self::drive_vm_on_runtime)
    /// (block-wait on external completions, sleep out a bare timer, drain
    /// ready detached work at exit, settle a genuine deadlock) — that
    /// function is now a thin wrapper: submit, then call this.
    pub fn drive_until_settled(&self, root: &sema_vm::runtime::RootHandle) -> EvalResult {
        self.drive_handle_to_settlement(root)
    }

    /// Drive the interpreter's persistent runtime for exactly one bounded
    /// turn (the wasm/notebook/debugger shape — a host that wants to observe
    /// progress between quanta rather than block to settlement). Never
    /// blocks on a timer or the external-completion inbox; a caller that
    /// needs to wait for those should inspect the returned
    /// [`DriveState::Idle`](sema_vm::runtime::DriveState::Idle) itself.
    pub fn drive_turn(&self) -> Result<sema_vm::runtime::DriveState, SemaError> {
        let runtime = self
            .runtime
            .as_ref()
            .expect("runtime is present outside of Drop");
        let budget = sema_vm::runtime::DriveBudget::host_default();
        runtime
            .drive(&budget)
            .map_err(|e| SemaError::eval(format!("runtime fault: {e:?}")))
    }

    /// Drive one bounded turn while executing VM quanta only for `roots`.
    /// Runtime-wide completion and cancellation bookkeeping still advances,
    /// but another host's ready root cannot execute under this host's policy.
    pub fn drive_roots(
        &self,
        roots: &[sema_core::runtime::RootId],
    ) -> Result<sema_vm::runtime::DriveState, SemaError> {
        let runtime = self
            .runtime
            .as_ref()
            .expect("runtime is present outside of Drop");
        let budget = sema_vm::runtime::DriveBudget::host_default();
        runtime
            .drive_roots(&budget, roots)
            .map_err(|e| SemaError::eval(format!("runtime fault: {e:?}")))
    }

    /// Drain every [`OutputEvent`](sema_vm::runtime::OutputEvent) captured so
    /// far from roots submitted with `capture_output: true`. A root that
    /// didn't opt in still writes straight to process stdout/stderr, exactly
    /// as before this API existed — this only ever returns output from
    /// capturing roots.
    ///
    /// Ordering: events come back in global execution order (the order the
    /// underlying prints actually happened across every capturing root on
    /// this runtime, interleaved as the scheduler ran them), which preserves
    /// each root's own events in FIFO order too. Each call drains only what
    /// accumulated since the previous drain — reading is destructive, never
    /// a peek. The captured output itself is retained regardless of the
    /// owning root's lifecycle: it survives the root settling, being
    /// reaped, and interpreter `shutdown`, so a caller can still drain a
    /// root's final output after polling it to completion.
    pub fn take_output(&self) -> Vec<sema_vm::runtime::OutputEvent> {
        let runtime = self
            .runtime
            .as_ref()
            .expect("runtime is present outside of Drop");
        runtime.take_captured_output()
    }

    /// Shut down the interpreter's persistent runtime: cancel every live
    /// root/task and drain teardown until quiescent or `opts.deadline`.
    /// Wraps [`sema_vm::runtime::Runtime::shutdown`], surfacing a runtime
    /// fault as a `SemaError` instead of a bare `RuntimeFault` — every other
    /// `Interpreter` entry point already reports failure this way, and a
    /// fault mid-shutdown (id-space exhaustion, an invariant violation) is
    /// exactly the kind of thing a host must not silently swallow into an
    /// always-`Ok` report.
    pub fn shutdown(
        &self,
        opts: sema_vm::runtime::ShutdownOptions,
    ) -> Result<sema_vm::runtime::ShutdownReport, SemaError> {
        self.runtime()
            .shutdown(&opts)
            .map_err(|fault| SemaError::eval(format!("runtime fault during shutdown: {fault:?}")))
    }

    /// Submit an already-seeded VM as a fresh root on this interpreter's
    /// persistent runtime and drive it to settlement. Shared by every entry
    /// point that builds a VM directly — top-level source evaluation
    /// ([`run_exprs_via_runtime`](Self::run_exprs_via_runtime)) and pre-compiled
    /// `.semac` bytecode runners. The VM must already have its main frame seeded
    /// (`seed_main_frame`).
    /// The number of tasks the interpreter's persistent cooperative runtime is
    /// currently holding (live + settled-not-yet-reaped). A test/observability
    /// oracle: after a program that cancels a task, `0` proves the cancelled task
    /// and every descendant it transitively cancelled were settled + reaped, not
    /// orphaned. The unified-runtime analogue of the retired legacy
    /// `sema_vm::scheduler_task_count()`.
    pub fn runtime_live_task_count(&self) -> usize {
        self.runtime
            .as_ref()
            .map_or(0, sema_vm::runtime::Runtime::live_task_count)
    }

    /// Number of live per-resource runtime gates. Terminal resource teardown
    /// returns this count to its previous baseline.
    pub fn runtime_resource_gate_count(&self) -> usize {
        self.runtime
            .as_ref()
            .map_or(0, sema_vm::runtime::Runtime::resource_gate_count)
    }

    /// The interpreter's single persistent unified runtime. Present outside of
    /// `Drop`. A host that needs finer control than [`drive_vm_on_runtime`] — the
    /// cooperative (headless) debugger, which drives bounded turns and maps
    /// `DriveState::DebugStopped` to a stopped event rather than driving to
    /// settlement — submits its root and drives here directly.
    ///
    /// [`drive_vm_on_runtime`]: Self::drive_vm_on_runtime
    pub fn runtime(&self) -> &sema_vm::runtime::Runtime {
        self.runtime
            .as_ref()
            .expect("runtime is present outside of Drop")
    }

    /// A `Send + Sync` handle for cancelling roots on this interpreter's
    /// runtime from another thread (a signal handler, a watchdog, a
    /// notebook server's request handler) — see
    /// [`RuntimeCommandHandle`](sema_vm::runtime::RuntimeCommandHandle).
    pub fn command_handle(&self) -> sema_vm::runtime::RuntimeCommandHandle {
        self.runtime().command_handle()
    }

    pub fn drive_vm_on_runtime(&self, vm: sema_vm::VM) -> EvalResult {
        // Submit as a fresh ROOT to the interpreter's single persistent runtime
        // (constructed once over THIS interpreter's context, so the VM's
        // `call_value`/`eval_value` re-entry resolves the registered callbacks
        // and the live module cache / current-file / dynamic context persist).
        // Detached tasks from prior evals remain in this runtime and are driven
        // fairly alongside the new root; `poll_result` only settles when the
        // requested root itself settles, not when every detached task does.
        let runtime = self
            .runtime
            .as_ref()
            .expect("runtime is present outside of Drop");
        self.ensure_synchronous_runtime_entry_allowed()?;
        let handle = runtime
            .submit_root(vm)
            .map_err(|e| SemaError::eval(format!("root submission failed: {e:?}")))?;
        self.drive_handle_to_settlement(&handle)
    }

    /// The drive loop shared by [`drive_vm_on_runtime`](Self::drive_vm_on_runtime)
    /// (which submits then drives) and the public
    /// [`drive_until_settled`](Self::drive_until_settled) (which drives an
    /// already-submitted [`RootHandle`](sema_vm::runtime::RootHandle)).
    fn drive_handle_to_settlement(&self, handle: &sema_vm::runtime::RootHandle) -> EvalResult {
        use sema_vm::runtime::{DriveBudget, DriveState, RootPoll};

        let runtime = self
            .runtime
            .as_ref()
            .expect("runtime is present outside of Drop");
        let budget = DriveBudget::host_default();
        loop {
            match handle.poll_result() {
                RootPoll::Ready(settlement) => {
                    let result = match &settlement.outcome {
                        sema_core::runtime::TaskOutcome::Returned(value) => Ok(value.clone()),
                        sema_core::runtime::TaskOutcome::Failed(error) => Err(error.clone()),
                        sema_core::runtime::TaskOutcome::Cancelled(reason) => {
                            Err(SemaError::eval(format!("evaluation cancelled: {reason:?}")))
                        }
                    };
                    // Drain any READY detached work before returning (legacy
                    // "top-level (async …) drains the scheduler at exit"): a
                    // cooperative child spawned by this program does not run until
                    // its spawner suspends/returns, so at this point a fire-and-
                    // forget `(async (println …))` is still Ready. Run all such
                    // ready work to a quiescent point, but never block on a timer
                    // or an external inbox — genuinely-parked detached tasks
                    // persist to later evals, exactly as before the flip. Bounded:
                    // stops as soon as the runtime stops making ready progress.
                    //
                    // Pending-cancellation teardown counts as progress too: a task
                    // cancelled during this program but still parked on an in-flight
                    // External/IO/ResourceSlot wait must have its abort flushed
                    // before returning, not deferred to `Interpreter::drop`
                    // (ASYNC-TIMEOUT-CANCEL-1). Request-time delivery (C2) makes
                    // that the common case; this keeps the drain going for any
                    // teardown the drive scan still owes.
                    loop {
                        match drive_runtime_root(runtime, &budget, handle.id())
                            .map_err(|e| SemaError::eval(format!("runtime fault: {e:?}")))?
                        {
                            DriveState::Progress {
                                ready_remaining: true,
                                ..
                            } => continue,
                            DriveState::Progress { .. }
                                if runtime.has_pending_cancel_teardown() =>
                            {
                                continue
                            }
                            _ => break,
                        }
                    }
                    return result;
                }
                RootPoll::Pending => {}
                RootPoll::Aborted(fault) => {
                    return Err(SemaError::eval(format!("root aborted: {fault:?}")));
                }
                RootPoll::RuntimeDropped | RootPoll::InvariantViolation => {
                    return Err(SemaError::eval("runtime invariant violation"));
                }
            }
            match drive_runtime_root(runtime, &budget, handle.id())
                .map_err(|e| SemaError::eval(format!("runtime fault: {e:?}")))?
            {
                DriveState::Progress { .. } => {}
                #[cfg(target_arch = "wasm32")]
                DriveState::Idle { .. } => {
                    // A synchronous JS call stack cannot let fetch/timer
                    // callbacks run. Cancel this exact root before returning so
                    // its parked VM, continuation, and wait registration cannot
                    // leak into the interpreter's next evaluation.
                    runtime.cancel_root(handle.id(), sema_core::runtime::CancelReason::HostStop);
                    for _ in 0..10_000 {
                        if !matches!(handle.poll_result(), RootPoll::Pending) {
                            break;
                        }
                        if !matches!(
                            drive_runtime_root(runtime, &budget, handle.id()).map_err(|e| {
                                SemaError::eval(format!(
                                    "runtime fault while cancelling suspended WASM root: {e:?}"
                                ))
                            })?,
                            DriveState::Progress { .. }
                        ) {
                            break;
                        }
                    }
                    return Err(SemaError::eval(
                        "synchronous WebAssembly evaluation cannot suspend",
                    )
                    .with_hint(
                        "Use evalPromise or another Promise-based entry point for async work",
                    ));
                }
                // A task is parked on an external operation running on a worker
                // thread (a blocking op submitted to the executor). Block-wait on
                // the completion inbox — bounded by the earliest timer deadline if
                // one exists — then drive again so `drain_completion` delivers the
                // worker's result and resumes the task. Wakeable and never
                // busy-spins: an arriving completion returns immediately.
                #[cfg(not(target_arch = "wasm32"))]
                DriveState::Idle {
                    next_deadline,
                    inbox_wakeup_required: true,
                    ..
                } => {
                    if !runtime.block_on_inbox(next_deadline) && next_deadline.is_none() {
                        // The inbox closed with no completion and no timer to fall
                        // back on: the parked task can never be resumed.
                        return Err(SemaError::eval(
                            "eval_via_runtime: external wait cannot be completed (executor inbox closed)",
                        ));
                    }
                }
                // The root is parked purely on a timer (`async/sleep`): the only
                // pending work is a future deadline. Block on the same inbox a
                // parked external wait would (bounded by that deadline) rather
                // than a raw `thread::sleep` — no external op is registered, so
                // no completion can arrive, but a cross-thread
                // `RuntimeCommandHandle::cancel_root`/`cancel_all` (a host's
                // Ctrl-C handler, a watchdog) rides the SAME inbox and must wake
                // this thread promptly instead of waiting out the full timer.
                // Times out and returns `false` at `deadline` in the ordinary
                // no-command case — equivalent to the sleep it replaces — then
                // drives again so `fire_timer` wakes the VM (or the drained
                // command cancels it first).
                //
                // A native host can park the calling thread until the timer or
                // a cross-thread cancellation command wakes the shared inbox.
                // WASM is handled by the fail-fast arm above because a
                // synchronous JS stack cannot pump browser callbacks.
                #[cfg(not(target_arch = "wasm32"))]
                DriveState::Idle {
                    next_deadline: Some(deadline),
                    inbox_wakeup_required: false,
                    ..
                } => {
                    runtime.block_on_inbox(Some(deadline));
                }
                // Fully idle: no task made progress this turn, no timer deadline,
                // and no pending external completion — nothing can ever change
                // the runtime's state. The requested root is parked on an
                // intra-runtime wait (channel/promise) that no runnable task can
                // satisfy: a genuine deadlock. Settle the root Failed with the
                // legacy-parity error (`channel/recv: channel is empty` /
                // `channel/send: channel is full` for a top-level channel op, or
                // the generic all-blocked deadlock otherwise) so the next
                // `poll_result` returns it. Bounded — never hangs.
                #[cfg(not(target_arch = "wasm32"))]
                DriveState::Idle {
                    next_deadline: None,
                    inbox_wakeup_required: false,
                } => {
                    if !runtime
                        .settle_deadlocked_root(handle.id())
                        .map_err(|e| SemaError::eval(format!("runtime fault: {e:?}")))?
                    {
                        return Err(SemaError::eval(
                            "eval_via_runtime: root did not settle (unsupported suspension on the runtime path)",
                        ));
                    }
                }
                _ => {
                    return Err(SemaError::eval(
                        "eval_via_runtime: root did not settle (unsupported suspension on the runtime path)",
                    ));
                }
            }
        }
    }

    /// Parse and evaluate on the VM (global env; `define`s persist — see `eval`).
    pub fn eval_str(&self, input: &str) -> EvalResult {
        self.eval_str_in_global(input)
    }

    /// Evaluate in the global environment so that `define` persists across calls.
    ///
    /// Routes through the unified cooperative runtime (`run_exprs_via_runtime`):
    /// the interpreter's single persistent runtime IS the sole evaluator for
    /// every real eval entry point (this, `eval_str_in_global`, and
    /// `eval_str_compiled` all share it).
    pub fn eval_in_global(&self, expr: &Value) -> EvalResult {
        self.run_exprs_via_runtime(std::slice::from_ref(expr))
    }

    /// Parse and evaluate in the global environment so that `define` persists across calls.
    ///
    /// Routes through the unified cooperative runtime — see
    /// [`eval_in_global`](Self::eval_in_global).
    pub fn eval_str_in_global(&self, input: &str) -> EvalResult {
        let (exprs, spans) = sema_reader::read_many_with_spans(input)?;
        self.ctx.merge_span_table(spans);
        if exprs.is_empty() {
            return Ok(Value::nil());
        }
        self.run_exprs_via_runtime(&exprs)
    }

    /// Parse a program and evaluate it in the global env (`define`s persist),
    /// driven through the unified cooperative runtime. Backs `common::eval` and
    /// the CLI `-e` path; behaves identically to
    /// [`eval_str_in_global`](Self::eval_str_in_global).
    pub fn eval_str_compiled(&self, input: &str) -> EvalResult {
        let (exprs, spans) = sema_reader::read_many_with_spans(input)?;
        self.ctx.merge_span_table(spans);
        if exprs.is_empty() {
            return Ok(Value::nil());
        }
        self.run_exprs_via_runtime(&exprs)
    }

    /// Compile source code to bytecode without executing.
    /// Handles macro expansion (defmacro + macro calls) before compilation.
    pub fn compile_to_bytecode(&self, input: &str) -> Result<sema_vm::CompileResult, SemaError> {
        let (exprs, spans) = sema_reader::read_many_with_spans(input)?;
        self.ctx.merge_span_table(spans);

        let mut expanded = Vec::new();
        for expr in &exprs {
            let exp = self.expand_for_vm(expr)?;
            if !exp.is_nil() {
                expanded.push(exp);
            }
        }

        if expanded.is_empty() {
            expanded.push(Value::nil());
        }

        let prog = sema_vm::compile_program(&expanded, None)?;
        Ok(sema_vm::CompileResult::new(
            prog.closure.func.chunk.clone(),
            prog.functions.iter().map(|f| (**f).clone()).collect(),
        ))
    }

    /// Pre-process a top-level expression for VM compilation: register any
    /// `defmacro` forms, then expand macro calls in all other forms.
    pub fn expand_for_vm(&self, expr: &Value) -> EvalResult {
        expand_for_vm_in(&self.ctx, &self.global_env, expr)
    }

    /// Expand a multi-form program with cross-form define shadowing — use this
    /// (not per-form `expand_for_vm`) whenever all forms expand before any runs.
    pub fn expand_for_vm_batch(&self, exprs: &[Value]) -> Result<Vec<Value>, SemaError> {
        expand_for_vm_batch(&self.ctx, &self.global_env, exprs)
    }
}

/// Lexical names that shadow macros during expansion (a linked stack of
/// frames, innermost first). Macro expansion is name-based and runs before
/// scope resolution; without this, a user binding named after a prelude macro
/// (`step`, `phase`, ...) is rewritten as a macro call — in a define-sugar
/// head that is a hard compile error, and for `phase`-shaped macros it
/// silently clobbers the runtime binding the template calls.
struct Shadow<'a> {
    names: HashSet<Spur>,
    parent: Option<&'a Shadow<'a>>,
}

impl<'a> Shadow<'a> {
    fn child(&'a self, names: HashSet<Spur>) -> Shadow<'a> {
        Shadow {
            names,
            parent: Some(self),
        }
    }

    fn contains(&self, s: Spur) -> bool {
        self.names.contains(&s) || self.parent.is_some_and(|p| p.contains(s))
    }
}

/// Collect every symbol in a binding pattern (a param list, a let binding
/// name, a match/destructure pattern). Deliberately conservative: any symbol
/// anywhere in the pattern counts as bound. Over-collecting only suppresses
/// macro expansion where a same-named binding plausibly exists — the safe
/// direction.
fn collect_pattern_symbols(pattern: &Value, out: &mut HashSet<Spur>) {
    if let Some(s) = pattern.as_symbol_spur() {
        out.insert(s);
        return;
    }
    if let Some(items) = pattern.as_list() {
        for item in items {
            collect_pattern_symbols(item, out);
        }
        return;
    }
    match pattern.view() {
        ValueView::Vector(items) => {
            for item in items.iter() {
                collect_pattern_symbols(item, out);
            }
        }
        ValueView::Map(map) => {
            for (k, v) in map.iter() {
                collect_pattern_symbols(k, out);
                collect_pattern_symbols(v, out);
            }
        }
        _ => {}
    }
}

/// Names a form defines at its sequence level (top level or a body), for
/// letrec*-style shadowing: `define` (sugar + plain), `define-values`,
/// `defmulti`, `deftool`, `defagent`, and `define-record-type` (constructor,
/// predicate, and accessors included). Recurses into `begin`/`progn`.
fn collect_defined_names(expr: &Value, out: &mut HashSet<Spur>) {
    let Some(items) = expr.as_list() else { return };
    let Some(head) = items.first().and_then(|v| v.as_symbol_spur()) else {
        return;
    };
    match resolve(head).as_str() {
        "begin" | "progn" => {
            for item in &items[1..] {
                collect_defined_names(item, out);
            }
        }
        "define" | "defmulti" | "deftool" | "defagent" => {
            if let Some(target) = items.get(1) {
                if let Some(s) = target.as_symbol_spur() {
                    out.insert(s);
                } else if let Some(sugar) = target.as_list() {
                    // Sugar head: (define (name . params) ...) defines `name`.
                    if let Some(s) = sugar.first().and_then(|v| v.as_symbol_spur()) {
                        out.insert(s);
                    }
                }
            }
        }
        "define-values" => {
            if let Some(formals) = items.get(1) {
                collect_pattern_symbols(formals, out);
            }
        }
        "define-record-type" => {
            // (define-record-type Name (ctor field...) pred (field accessor [setter])...)
            for part in &items[1..] {
                collect_pattern_symbols(part, out);
            }
        }
        _ => {}
    }
}

/// Expand the forms of a body sequence: names defined anywhere in the body
/// shadow macros throughout it (letrec* semantics, matching the resolver).
fn expand_body(
    ctx: &EvalContext,
    env: &Env,
    body: &[Value],
    shadow: &Shadow,
) -> Result<Vec<Value>, SemaError> {
    let mut defined = HashSet::new();
    for form in body {
        collect_defined_names(form, &mut defined);
    }
    let inner = shadow.child(defined);
    body.iter()
        .map(|form| expand_macros_in(ctx, env, form, &inner))
        .collect()
}

/// Rebuild a list form only if any element changed, preserving Rc pointer
/// identity otherwise (span lookups are keyed by pointer).
fn rebuilt_list(original: &Value, items: &[Value], expanded: Vec<Value>) -> Value {
    let changed = expanded
        .iter()
        .zip(items.iter())
        .any(|(a, b)| a.raw_bits() != b.raw_bits())
        || expanded.len() != items.len();
    if changed {
        Value::list(expanded)
    } else {
        original.clone()
    }
}

/// Pre-process a top-level expression for VM compilation, expanding macro calls
/// and eagerly registering `defmacro` forms — against `env` rather than a fixed
/// global env. For top-level code `env` is the global env (unchanged behavior);
/// for a `load`ed module body it is the same shared global env, so a `defmacro`
/// registers where `expand_macros_in` looks it up and inherited macros still
/// resolve via the parent chain.
///
/// A form's own `define`s shadow same-named macros inside it. For a multi-form
/// program use [`expand_for_vm_batch`], which lets a top-level
/// `(define step ...)` shadow the macro in sibling forms too.
pub fn expand_for_vm_in(ctx: &EvalContext, env: &Env, expr: &Value) -> EvalResult {
    let mut defined = HashSet::new();
    collect_defined_names(expr, &mut defined);
    let shadow = Shadow {
        names: defined,
        parent: None,
    };
    expand_top_form(ctx, env, expr, &shadow)
}

/// Expand a whole multi-form program: names defined by ANY top-level form
/// shadow same-named macros in EVERY form (mirroring the compiler's
/// redefined-globals rule for intrinsics), so `(define (step n) n) (step 3)`
/// calls the user's function rather than expanding the prelude macro.
pub fn expand_for_vm_batch(
    ctx: &EvalContext,
    env: &Env,
    exprs: &[Value],
) -> Result<Vec<Value>, SemaError> {
    let mut defined = HashSet::new();
    for expr in exprs {
        collect_defined_names(expr, &mut defined);
    }
    let shadow = Shadow {
        names: defined,
        parent: None,
    };
    exprs
        .iter()
        .map(|expr| expand_top_form(ctx, env, expr, &shadow))
        .collect()
}

fn expand_top_form(ctx: &EvalContext, env: &Env, expr: &Value, shadow: &Shadow) -> EvalResult {
    if let Some(items) = expr.as_list() {
        if let Some(s) = items.first().and_then(|v| v.as_symbol_spur()) {
            let name = resolve(s);
            if name == "defmacro" {
                // Register the macro directly (pure destructure) — the VM macro
                // path is direct.
                register_defmacro(items, env)?;
                return Ok(Value::nil());
            }
            if name == "define-syntax" {
                // Register the R7RS syntax-rules transformer directly (pure
                // destructure), mirroring the `defmacro` branch.
                register_define_syntax(items, env)?;
                return Ok(Value::nil());
            }
            if name == "begin" || name == "progn" {
                let mut new_items = vec![Value::symbol_from_spur(s)];
                let mut changed = false;
                for item in &items[1..] {
                    let expanded = expand_top_form(ctx, env, item, shadow)?;
                    if expanded.raw_bits() != item.raw_bits() {
                        changed = true;
                    }
                    new_items.push(expanded);
                }
                if !changed {
                    return Ok(expr.clone());
                }
                return Ok(Value::list(new_items));
            }
        }
    }
    expand_macros_in(ctx, env, expr, shadow)
}

/// Recursively expand macro calls, resolving macros via `env` (walking the
/// parent chain). Scope-aware: binding positions (define-sugar heads, params,
/// let names, match patterns) never expand, and a head symbol that a lexical
/// binding shadows is treated as an ordinary call. Preserves Rc pointer
/// identity when no expansion occurs so span lookups (keyed by Rc pointer)
/// remain valid.
fn expand_macros_in(ctx: &EvalContext, env: &Env, expr: &Value, shadow: &Shadow) -> EvalResult {
    if let Some(items) = expr.as_list() {
        if !items.is_empty() {
            if let Some(s) = items.first().and_then(|v| v.as_symbol_spur()) {
                let name = resolve(s);
                if name == "quote" {
                    return Ok(expr.clone());
                }
                // Binding forms expand structurally so their bound names
                // shadow macros in exactly the scopes the resolver gives them.
                match name.as_str() {
                    "define" => return expand_define_form(ctx, env, expr, items, shadow),
                    "fn" | "lambda" => return expand_lambda_form(ctx, env, expr, items, shadow),
                    "let" | "let*" | "letrec" | "let-values" | "let*-values" => {
                        return expand_let_form(ctx, env, expr, items, shadow, &name)
                    }
                    "do" => return expand_do_form(ctx, env, expr, items, shadow),
                    "try" => return expand_try_form(ctx, env, expr, items, shadow),
                    "match" | "match*" => return expand_match_form(ctx, env, expr, items, shadow),
                    "define-values" => {
                        // Formals are a binding position; only the value expands.
                        let mut expanded: Vec<Value> = items[..items.len().min(2)].to_vec();
                        for item in items.iter().skip(2) {
                            expanded.push(expand_macros_in(ctx, env, item, shadow)?);
                        }
                        return Ok(rebuilt_list(expr, items, expanded));
                    }
                    _ => {}
                }
                if !shadow.contains(s) {
                    if let Some(mac_val) = env.get(s) {
                        if let Some(mac) = mac_val.as_macro_rc() {
                            if mac.syntax_rules.is_some() {
                                // R7RS syntax-rules: pattern-match + template expand.
                                let expanded = crate::syntax_rules::expand(&mac, &items[1..], env)?;
                                return expand_macros_in(ctx, env, &expanded, shadow);
                            }
                            // VM-native expansion: apply the transformer on the VM.
                            let expanded = apply_macro_vm(ctx, &mac, &items[1..], env)?;
                            return expand_macros_in(ctx, env, &expanded, shadow);
                        }
                    }
                }
            }
            let expanded: Vec<Value> = items
                .iter()
                .map(|v| expand_macros_in(ctx, env, v, shadow))
                .collect::<Result<_, _>>()?;
            return Ok(rebuilt_list(expr, items, expanded));
        }
    }

    match expr.view() {
        ValueView::Vector(items) => {
            let expanded: Vec<Value> = items
                .iter()
                .map(|v| expand_macros_in(ctx, env, v, shadow))
                .collect::<Result<_, _>>()?;
            let changed = expanded
                .iter()
                .zip(items.iter())
                .any(|(a, b)| a.raw_bits() != b.raw_bits());
            if changed {
                Ok(Value::vector(expanded))
            } else {
                Ok(expr.clone())
            }
        }
        ValueView::Map(map) => {
            let mut changed = false;
            let mut expanded = BTreeMap::new();
            for (key, value) in map.iter() {
                let expanded_key = expand_macros_in(ctx, env, key, shadow)?;
                let expanded_value = expand_macros_in(ctx, env, value, shadow)?;
                changed |= expanded_key.raw_bits() != key.raw_bits()
                    || expanded_value.raw_bits() != value.raw_bits();
                expanded.insert(expanded_key, expanded_value);
            }
            if changed {
                Ok(Value::map(expanded))
            } else {
                Ok(expr.clone())
            }
        }
        _ => Ok(expr.clone()),
    }
}

/// `(define name expr)` / `(define (name . params) body...)`: the head is a
/// binding position (never expanded); the defined name and any params shadow
/// macros in the value/body.
fn expand_define_form(
    ctx: &EvalContext,
    env: &Env,
    expr: &Value,
    items: &[Value],
    shadow: &Shadow,
) -> EvalResult {
    let Some(target) = items.get(1) else {
        return Ok(expr.clone());
    };
    let mut bound = HashSet::new();
    collect_pattern_symbols(target, &mut bound);
    let inner = shadow.child(bound);
    let mut expanded: Vec<Value> = items[..2.min(items.len())].to_vec();
    expanded.extend(expand_body(ctx, env, &items[2..], &inner)?);
    Ok(rebuilt_list(expr, items, expanded))
}

/// `(fn params body...)`: params are a binding position and shadow the body.
fn expand_lambda_form(
    ctx: &EvalContext,
    env: &Env,
    expr: &Value,
    items: &[Value],
    shadow: &Shadow,
) -> EvalResult {
    let Some(params) = items.get(1) else {
        return Ok(expr.clone());
    };
    let mut bound = HashSet::new();
    collect_pattern_symbols(params, &mut bound);
    let inner = shadow.child(bound);
    let mut expanded: Vec<Value> = items[..2.min(items.len())].to_vec();
    expanded.extend(expand_body(ctx, env, &items[2..], &inner)?);
    Ok(rebuilt_list(expr, items, expanded))
}

/// The `let` family, named `let` included. Init scoping follows the form:
/// `let`/`let-values` inits see the outer scope, the starred forms see the
/// bindings accumulated so far, `letrec` inits see everything.
fn expand_let_form(
    ctx: &EvalContext,
    env: &Env,
    expr: &Value,
    items: &[Value],
    shadow: &Shadow,
    form: &str,
) -> EvalResult {
    // Named let: (let name ((v init)...) body...)
    let named = form == "let" && items.get(1).is_some_and(|v| v.as_symbol_spur().is_some());
    let bindings_idx = if named { 2 } else { 1 };
    let Some(bindings_form) = items.get(bindings_idx) else {
        return Ok(expr.clone());
    };
    let pairs: Vec<Value> = if let Some(l) = bindings_form.as_list() {
        l.to_vec()
    } else if let ValueView::Vector(v) = bindings_form.view() {
        v.to_vec()
    } else {
        // Malformed; let the lowering report it. Expand generically.
        let expanded: Vec<Value> = items
            .iter()
            .map(|v| expand_macros_in(ctx, env, v, shadow))
            .collect::<Result<_, _>>()?;
        return Ok(rebuilt_list(expr, items, expanded));
    };

    let mut bound = HashSet::new();
    if named {
        collect_pattern_symbols(&items[1], &mut bound);
    }
    if form == "letrec" {
        for pair in &pairs {
            if let Some(p) = pair.as_list().and_then(|p| p.first().cloned()) {
                collect_pattern_symbols(&p, &mut bound);
            }
        }
    }

    let sequential = form == "let*" || form == "let*-values";
    let mut new_pairs = Vec::with_capacity(pairs.len());
    for pair in &pairs {
        let Some(pair_items) = pair.as_list() else {
            new_pairs.push(pair.clone());
            continue;
        };
        let init_scope = shadow.child(bound.clone());
        let mut new_pair: Vec<Value> = Vec::with_capacity(pair_items.len());
        for (i, part) in pair_items.iter().enumerate() {
            if i == 0 {
                // The binding pattern itself never expands.
                new_pair.push(part.clone());
            } else {
                new_pair.push(expand_macros_in(ctx, env, part, &init_scope)?);
            }
        }
        if sequential || form == "letrec" {
            if let Some(p) = pair_items.first() {
                collect_pattern_symbols(p, &mut bound);
            }
            new_pairs.push(rebuilt_list(pair, pair_items, new_pair));
        } else {
            new_pairs.push(rebuilt_list(pair, pair_items, new_pair));
        }
    }
    if !sequential && form != "letrec" {
        // Plain let / let-values: all names bind only in the body.
        for pair in &pairs {
            if let Some(p) = pair.as_list().and_then(|p| p.first().cloned()) {
                collect_pattern_symbols(&p, &mut bound);
            }
        }
    }

    let body_scope = shadow.child(bound);
    let mut expanded: Vec<Value> = items[..bindings_idx].to_vec();
    expanded.push(Value::list(new_pairs));
    expanded.extend(expand_body(
        ctx,
        env,
        &items[bindings_idx + 1..],
        &body_scope,
    )?);
    Ok(rebuilt_list(expr, items, expanded))
}

/// Scheme `do`: `(do ((var init step)...) (test result...) body...)` — vars
/// bind in steps, the test/result, and the body; inits see the outer scope.
fn expand_do_form(
    ctx: &EvalContext,
    env: &Env,
    expr: &Value,
    items: &[Value],
    shadow: &Shadow,
) -> EvalResult {
    let Some(specs) = items.get(1).and_then(|v| v.as_list().map(|l| l.to_vec())) else {
        let expanded: Vec<Value> = items
            .iter()
            .map(|v| expand_macros_in(ctx, env, v, shadow))
            .collect::<Result<_, _>>()?;
        return Ok(rebuilt_list(expr, items, expanded));
    };
    let mut bound = HashSet::new();
    for spec in &specs {
        if let Some(p) = spec.as_list().and_then(|p| p.first().cloned()) {
            collect_pattern_symbols(&p, &mut bound);
        }
    }
    let inner = shadow.child(bound);
    let mut new_specs = Vec::with_capacity(specs.len());
    for spec in &specs {
        let Some(spec_items) = spec.as_list() else {
            new_specs.push(spec.clone());
            continue;
        };
        let mut new_spec = Vec::with_capacity(spec_items.len());
        for (i, part) in spec_items.iter().enumerate() {
            match i {
                0 => new_spec.push(part.clone()),
                1 => new_spec.push(expand_macros_in(ctx, env, part, shadow)?),
                _ => new_spec.push(expand_macros_in(ctx, env, part, &inner)?),
            }
        }
        new_specs.push(rebuilt_list(spec, spec_items, new_spec));
    }
    let mut expanded: Vec<Value> = vec![items[0].clone(), Value::list(new_specs)];
    for item in items.iter().skip(2) {
        expanded.push(expand_macros_in(ctx, env, item, &inner)?);
    }
    Ok(rebuilt_list(expr, items, expanded))
}

/// `try`: catch clauses bind their error variable over the handler body.
fn expand_try_form(
    ctx: &EvalContext,
    env: &Env,
    expr: &Value,
    items: &[Value],
    shadow: &Shadow,
) -> EvalResult {
    let mut expanded: Vec<Value> = vec![items[0].clone()];
    for item in items.iter().skip(1) {
        let is_catch = item
            .as_list()
            .and_then(|l| l.first().and_then(|h| h.as_symbol_spur()))
            .is_some_and(|h| resolve(h) == "catch");
        if is_catch {
            let clause = item.as_list().unwrap();
            let mut bound = HashSet::new();
            if let Some(var) = clause.get(1) {
                collect_pattern_symbols(var, &mut bound);
            }
            let inner = shadow.child(bound);
            let mut new_clause: Vec<Value> = clause[..2.min(clause.len())].to_vec();
            for part in clause.iter().skip(2) {
                new_clause.push(expand_macros_in(ctx, env, part, &inner)?);
            }
            expanded.push(rebuilt_list(item, clause, new_clause));
        } else {
            expanded.push(expand_macros_in(ctx, env, item, shadow)?);
        }
    }
    Ok(rebuilt_list(expr, items, expanded))
}

/// `match`/`match*`: each clause's pattern is a binding position (never
/// expanded) whose symbols shadow the clause's guard and body.
fn expand_match_form(
    ctx: &EvalContext,
    env: &Env,
    expr: &Value,
    items: &[Value],
    shadow: &Shadow,
) -> EvalResult {
    let mut expanded: Vec<Value> = vec![items[0].clone()];
    if let Some(scrutinee) = items.get(1) {
        expanded.push(expand_macros_in(ctx, env, scrutinee, shadow)?);
    }
    for clause in items.iter().skip(2) {
        let parts: Option<Vec<Value>> = if let Some(l) = clause.as_list() {
            Some(l.to_vec())
        } else if let ValueView::Vector(v) = clause.view() {
            Some(v.to_vec())
        } else {
            None
        };
        let Some(parts) = parts else {
            expanded.push(expand_macros_in(ctx, env, clause, shadow)?);
            continue;
        };
        if parts.is_empty() {
            expanded.push(clause.clone());
            continue;
        }
        let mut bound = HashSet::new();
        collect_pattern_symbols(&parts[0], &mut bound);
        let inner = shadow.child(bound);
        let mut new_parts = vec![parts[0].clone()];
        for part in parts.iter().skip(1) {
            new_parts.push(expand_macros_in(ctx, env, part, &inner)?);
        }
        let changed = new_parts
            .iter()
            .zip(parts.iter())
            .any(|(a, b)| a.raw_bits() != b.raw_bits());
        if !changed {
            expanded.push(clause.clone());
        } else if clause.as_list().is_some() {
            expanded.push(Value::list(new_parts));
        } else {
            expanded.push(Value::vector(new_parts));
        }
    }
    Ok(rebuilt_list(expr, items, expanded))
}

/// Convert a deserialized `.semac` payload into the form shared by fresh-VM
/// execution and cooperative callable execution.
///
/// The bytecode format intentionally omits the in-process direct-native table,
/// so loaded programs resolve native calls through their global environment.
fn compiled_program_from_result(result: sema_vm::CompileResult) -> sema_vm::CompiledProgram {
    let functions: Vec<Rc<sema_vm::Function>> = result.functions.into_iter().map(Rc::new).collect();
    let main_cache_slots = result.chunk.n_global_cache_slots;
    let closure = Rc::new(sema_vm::Closure {
        func: Rc::new(sema_vm::Function {
            name: None,
            chunk: result.chunk,
            upvalue_descs: Vec::new(),
            upvalue_names: Vec::new(),
            arity: 0,
            has_rest: false,
            param_names: Vec::new().into(),
            local_names: Vec::new(),
            local_scopes: Vec::new(),
            source_file: None,
            cache_offset: 0,
        }),
        upvalues: Vec::new(),
        globals: None,
        functions: None,
    });
    sema_vm::CompiledProgram {
        closure,
        functions,
        native_table: Vec::new(),
        main_cache_slots,
    }
}

/// Build a fresh VM and main closure for synchronous `.semac` execution.
fn build_vm_for_compile_result(
    globals: Rc<Env>,
    result: sema_vm::CompileResult,
) -> Result<(sema_vm::VM, Rc<sema_vm::Closure>), SemaError> {
    let program = compiled_program_from_result(result);
    let closure = Rc::clone(&program.closure);
    let vm = sema_vm::VM::new(
        globals,
        program.functions,
        &program.native_table,
        program.main_cache_slots,
    )?;
    Ok((vm, closure))
}

/// Run deserialized bytecode (a `.semac` payload) on a fresh VM rooted at
/// `globals`. Used to `load`/`import` precompiled bytecode modules (e.g.
/// embedded in a standalone-executable or web-archive VFS) the same way
/// `eval_module_body_vm` runs source modules. Does NOT (re)initialize the
/// async scheduler — callers nest this inside an already-running program and
/// reuse the scheduler installed by the top-level VM driver.
pub fn execute_compile_result(
    ctx: &EvalContext,
    globals: Rc<Env>,
    result: sema_vm::CompileResult,
) -> Result<Value, SemaError> {
    let (mut vm, closure) = build_vm_for_compile_result(globals, result)?;
    vm.execute(closure, ctx)
}

/// Compile and run a `load`ed module body on the VM, one top-level form at a
/// time so a `defmacro` / nested `load` that registers a macro is visible to
/// later forms before they compile. `env` is the caller's shared global env, so
/// defines land in the global scope (matching `load` semantics). Returns the
/// value of the last form (nil for an empty body).
///
/// Only used for `load` (not `import`): `load` shares the global env, so module
/// functions resolve their globals against the same env every VM uses — avoiding
/// the per-module-globals problem that makes VM-backed `import` incorrect (see
/// docs/plans/2026-06-16-vm-module-loading.md). Does NOT (re)initialize the async
/// scheduler — it reuses the one installed by the top-level VM driver.
pub fn eval_module_body_vm(
    ctx: &EvalContext,
    env: &Env,
    exprs: &[Value],
    span_map: &sema_core::SpanMap,
    source_file: Option<std::path::PathBuf>,
) -> EvalResult {
    let mut result = Value::nil();
    for expr in exprs {
        let expanded = expand_for_vm_in(ctx, env, expr)?;
        // `defmacro` (and forms that expand to nothing) are applied by expansion;
        // there is nothing to compile/run for them.
        if expanded.is_nil() {
            continue;
        }
        let prog = sema_vm::compile_program_with_spans(
            std::slice::from_ref(&expanded),
            span_map,
            source_file.clone(),
        )?;
        let globals = Rc::new(env.clone());
        let mut vm = sema_vm::VM::new(
            globals,
            prog.functions,
            &prog.native_table,
            prog.main_cache_slots,
        )?;
        result = vm.execute(prog.closure, ctx)?;
    }
    // Each per-form VM ran on `Rc::new(env.clone())`; the clone shares both
    // `env`'s bindings map and its version cell (`Env::version` is `Rc`-held),
    // so any global the body (re)defined or `set!`d already bumped the version
    // the calling VM's inline cache is keyed on — no explicit re-bump needed.
    Ok(result)
}

/// VM-native evaluation for callback consumers (e.g. sema-llm tool handlers):
/// macro-expand, compile, and run `expr` on a fresh bytecode VM rooted at `env`.
/// This is used to keep the
/// eval-callback path on the VM. Each call builds a
/// throwaway VM over a clone of `env` (sharing its bindings), so it is suited to
/// one-shot evaluation rather than a persistent define-accumulating session.
pub fn eval_value_vm(ctx: &EvalContext, expr: &Value, env: &Env) -> EvalResult {
    let env_rc = Rc::new(env.clone());
    let expanded = expand_for_vm_in(ctx, &env_rc, expr)?;
    if expanded.is_nil() {
        return Ok(Value::nil());
    }
    let prog = sema_vm::compile_program(std::slice::from_ref(&expanded), None)?;
    let mut vm = sema_vm::VM::new(env_rc, prog.functions, &[], prog.main_cache_slots)?;
    vm.execute(prog.closure, ctx)
}

/// Call a function value with already-evaluated arguments.
/// This is the public API for stdlib functions that need to invoke callbacks.
///
/// For lambdas, this delegates to `apply_lambda` + a trampoline loop so that
/// subsequent evaluation happens iteratively rather than adding Rust stack
/// frames.  This is critical for WASM where the call stack is limited (~5 MB).
pub fn call_value(ctx: &EvalContext, func: &Value, args: &[Value]) -> EvalResult {
    match func.view() {
        ValueView::NativeFn(native) => (native.func)(ctx, args),
        ValueView::Lambda(_) => {
            // Raw `Lambda` values never occur on the VM path (user lambdas are
            // NativeFn-wrapped VM closures).
            Err(SemaError::eval(
                "internal: raw lambda value reached call_value (VM closures are native-fn-wrapped)"
                    .to_string(),
            ))
        }
        ValueView::Keyword(spur) => {
            if args.len() != 1 {
                let name = resolve(spur);
                return Err(SemaError::arity(format!(":{name}"), "1", args.len()));
            }
            let key = Value::keyword_from_spur(spur);
            match args[0].view() {
                ValueView::Map(map) => Ok(map.get(&key).cloned().unwrap_or(Value::nil())),
                ValueView::HashMap(map) => Ok(map.get(&key).cloned().unwrap_or(Value::nil())),
                _ => Err(SemaError::type_error_with_value(
                    "map",
                    args[0].type_name(),
                    &args[0],
                )),
            }
        }
        ValueView::MultiMethod(mm) => call_multimethod(ctx, &mm, args),
        _ => Err(
            SemaError::eval(format!("not callable: {} ({})", func, func.type_name()))
                .with_hint("expected a function, lambda, or keyword"),
        ),
    }
}

/// Like [`call_value`], but the caller passes an args buffer it OWNS and will
/// not reuse: a VM-closure callee moves the values into its frame slots (the
/// buffer is left holding nils), so a uniquely-owned accumulator stays
/// uniquely owned across the callback boundary — the enabler for the stdlib's
/// `strong_count == 1` in-place fast paths inside fold callbacks. Every other
/// callable falls back to the borrowed protocol (args intact).
pub fn call_value_owned(ctx: &EvalContext, func: &Value, args: &mut [Value]) -> EvalResult {
    if let Some(result) = sema_vm::call_closure_owned(func, ctx, args) {
        return result;
    }
    call_value(ctx, func, args)
}

/// Call a multimethod: dispatch on args, look up handler, call it. Handler
/// resolution is shared with the VM's runtime-quantum-aware direct-call sites
/// (`sema_core::resolve_multimethod_handler`) so both paths pick the exact
/// same handler for the exact same dispatch value.
fn call_multimethod(ctx: &EvalContext, mm: &Rc<MultiMethod>, args: &[Value]) -> EvalResult {
    let handler = sema_core::resolve_multimethod_handler(ctx, mm, args)?;
    call_value(ctx, &handler, args)
}

/// Run a trampoline to completion iteratively.
/// Used by `call_value` so that stdlib HOF callbacks (map, for-each, etc.)
/// don't grow the Rust call stack for every evaluation step.
/// Apply a macro by evaluating its body on the **bytecode VM**.
///
/// The macro's
/// (unevaluated) arguments are bound — together with a possible rest list — as
/// *globals* in a transient child env of `caller_env`; the transformer body is
/// then compiled fresh per call site (so auto-gensym stays hygienic — a cached
/// transformer would reuse the same gensym across call sites) and run on a VM
/// rooted at that env. Rooting at `caller_env` lets transformer bodies call
/// global helpers and reference module-level bindings, and binding params as
/// globals lets the compiled body resolve them via `GetGlobal`.
///
/// Used by the VM macro pre-expansion path (`expand_macros_in`) and
/// `__vm-macroexpand`.
pub fn apply_macro_vm(
    ctx: &EvalContext,
    mac: &sema_core::Macro,
    args: &[Value],
    caller_env: &Env,
) -> Result<Value, SemaError> {
    let env = Rc::new(Env::with_parent(Rc::new(caller_env.clone())));

    // Bind parameters to unevaluated forms.
    if let Some(rest) = mac.rest_param {
        if args.len() < mac.params.len() {
            return Err(SemaError::arity(
                resolve(mac.name),
                format!("{}+", mac.params.len()),
                args.len(),
            ));
        }
        for (param, arg) in mac.params.iter().zip(args.iter()) {
            env.set(*param, arg.clone());
        }
        env.set(rest, Value::list(args[mac.params.len()..].to_vec()));
    } else {
        if args.len() != mac.params.len() {
            return Err(SemaError::arity(
                resolve(mac.name),
                mac.params.len().to_string(),
                args.len(),
            ));
        }
        for (param, arg) in mac.params.iter().zip(args.iter()) {
            env.set(*param, arg.clone());
        }
    }

    // Compile and run each body form on the VM, fresh per call site (no cache)
    // to keep auto-gensym hygienic. The body is the *transformer* code; it is
    // NOT macro-pre-expanded here — quasiquote templates inside it (which may
    // legitimately mention the macro's own name, as the recursive threading
    // macros do) must be compiled as data, not re-expanded. Any macro call the
    // transformer *produces* is re-expanded by the caller (`expand_macros_in`
    // recurses on the returned form). `compile_program` lowers quasiquote /
    // unquote / unquote-splicing directly.
    let mut result = Value::nil();
    for expr in &mac.body {
        let prog = sema_vm::compile_program(std::slice::from_ref(expr), None)?;
        let mut vm = sema_vm::VM::new(env.clone(), prog.functions, &[], prog.main_cache_slots)?;
        result = vm.execute(prog.closure, ctx)?;
    }
    Ok(result)
}

/// Register a `defmacro` form's macro in `env` — a
/// pure destructure mirroring `special_forms::eval_defmacro`. Used by the VM
/// pre-expansion path so registering a macro is direct.
fn register_defmacro(items: &[Value], env: &Env) -> Result<(), SemaError> {
    // items[0] is the `defmacro` symbol; the rest are name, params, body…
    let args = &items[1..];
    if args.len() < 3 {
        return Err(SemaError::arity("defmacro", "3+", args.len()));
    }
    let name_spur = args[0]
        .as_symbol_spur()
        .ok_or_else(|| SemaError::eval("defmacro: name must be a symbol"))?;
    let param_list = args[1]
        .as_list()
        .ok_or_else(|| SemaError::eval("defmacro: params must be a list"))?;
    let param_names: Vec<sema_core::Spur> = param_list
        .iter()
        .map(|v| {
            v.as_symbol_spur()
                .ok_or_else(|| SemaError::eval("defmacro: parameter must be a symbol"))
        })
        .collect::<Result<_, _>>()?;
    let (params, rest_param) = special_forms::parse_params(&param_names);
    let body = args[2..].to_vec();
    env.set(
        name_spur,
        Value::macro_val(Macro {
            params,
            rest_param,
            body,
            name: name_spur,
            syntax_rules: None,
        }),
    );
    Ok(())
}

/// Register a `define-syntax` form's R7RS `syntax-rules` transformer in `env`
/// (pure destructure) — the syntax-rules counterpart of [`register_defmacro`].
/// `items[0]` is the `define-syntax` symbol; the rest are name + transformer.
fn register_define_syntax(items: &[Value], env: &Env) -> Result<(), SemaError> {
    let args = &items[1..];
    if args.len() != 2 {
        return Err(SemaError::eval(
            "define-syntax: expected (define-syntax name (syntax-rules ...))",
        ));
    }
    let name_spur = args[0]
        .as_symbol_spur()
        .ok_or_else(|| SemaError::eval("define-syntax: name must be a symbol"))?;
    let sr = parse_syntax_rules(&args[1])?;
    env.set(
        name_spur,
        Value::macro_val(Macro {
            params: Vec::new(),
            rest_param: None,
            body: Vec::new(),
            name: name_spur,
            syntax_rules: Some(Rc::new(sr)),
        }),
    );
    Ok(())
}

/// Parse a `(syntax-rules (literals...) (pattern template)...)` transformer form
/// — with an optional custom-ellipsis symbol before the literals list — into a
/// [`sema_core::SyntaxRules`].
fn parse_syntax_rules(form: &Value) -> Result<sema_core::SyntaxRules, SemaError> {
    let elems = form.as_list().ok_or_else(|| {
        SemaError::eval("define-syntax: transformer must be a (syntax-rules ...) form")
    })?;
    let head_ok = elems
        .first()
        .and_then(|v| v.as_symbol_spur())
        .is_some_and(|s| resolve(s) == "syntax-rules");
    if !head_ok {
        return Err(SemaError::eval(
            "define-syntax: transformer must be a (syntax-rules ...) form",
        ));
    }
    if elems.len() < 2 {
        return Err(SemaError::eval(
            "syntax-rules: malformed — expected (syntax-rules (literals...) rules...)",
        ));
    }
    // Optional custom ellipsis: a symbol in the slot where the literals list is
    // otherwise expected.
    let mut idx = 1;
    let ellipsis = if elems[idx].as_symbol_spur().is_some() {
        let e = elems[idx].as_symbol_spur().unwrap();
        idx += 1;
        e
    } else {
        intern("...")
    };
    let literals_val = elems
        .get(idx)
        .ok_or_else(|| SemaError::eval("syntax-rules: malformed — missing literals list"))?;
    let literals_list = literals_val
        .as_list()
        .ok_or_else(|| SemaError::eval("syntax-rules: literals must be a list"))?;
    let literals: Vec<Spur> = literals_list
        .iter()
        .map(|v| {
            v.as_symbol_spur()
                .ok_or_else(|| SemaError::eval("syntax-rules: each literal must be a symbol"))
        })
        .collect::<Result<_, _>>()?;
    idx += 1;
    let mut rules = Vec::new();
    for rule in &elems[idx..] {
        let rl = rule
            .as_list()
            .ok_or_else(|| SemaError::eval("syntax-rules: each rule must be (pattern template)"))?;
        if rl.len() < 2 {
            return Err(SemaError::eval(
                "syntax-rules: each rule must be (pattern template)",
            ));
        }
        let pattern = rl[0].clone();
        // R7RS rules have a single template; tolerate multiple by wrapping them
        // in an implicit `begin`.
        let template = if rl.len() == 2 {
            rl[1].clone()
        } else {
            let mut begin = vec![Value::symbol("begin")];
            begin.extend(rl[1..].iter().cloned());
            Value::list(begin)
        };
        rules.push((pattern, template));
    }
    Ok(sema_core::SyntaxRules {
        literals,
        ellipsis,
        rules,
    })
}

/// Register `__vm-*` native functions that the bytecode VM calls back into
/// the evaluator for forms that cannot be fully compiled.
/// Load built-in macros (threading, when-let, if-let) into the global environment.
pub fn load_prelude(ctx: &EvalContext, env: &Rc<Env>) {
    let exprs = sema_reader::read_many(crate::prelude::PRELUDE)
        .unwrap_or_else(|e| panic!("internal: prelude failed to parse: {e}"));
    // The prelude is mostly `defmacro` forms (which expand to nil, registering the
    // macro as a side effect) plus a few `define` forms (the async agent-loop driver).
    // Register/expand via the VM-native path; a `define`
    // expands to a non-nil form, which we compile + run on
    // the VM (rooted at the global env) so its top-level binding persists.
    for expr in &exprs {
        let expanded = expand_for_vm_in(ctx, env, expr)
            .unwrap_or_else(|e| panic!("internal: prelude failed to load: {e}"));
        if expanded.is_nil() {
            continue;
        }
        let prog = sema_vm::compile_program(std::slice::from_ref(&expanded), None)
            .unwrap_or_else(|e| panic!("internal: prelude failed to compile: {e}"));
        let mut vm = sema_vm::VM::new(env.clone(), prog.functions, &[], prog.main_cache_slots)
            .unwrap_or_else(|e| panic!("internal: prelude VM init failed: {e}"));
        vm.execute(prog.closure, ctx)
            .unwrap_or_else(|e| panic!("internal: prelude failed to evaluate: {e}"));
    }
}

/// Upgrade a delegate's weak env capture. Delegates are only callable through
/// the env that owns them (compiled code resolves `__vm-*` as globals in that
/// env), so a failed upgrade is unreachable in practice — the error is defense,
/// not semantics.
fn upgrade_delegate_env(weak: &Weak<Env>) -> Result<Rc<Env>, SemaError> {
    weak.upgrade()
        .ok_or_else(|| SemaError::eval("evaluator environment has been torn down"))
}

/// Cooperative continuation for nested `eval`'s runtime ABI: the eval'd
/// program is driven as one `NativeOutcome::Call` to a callable wrapping the
/// compiled chunk (`sema_vm::program_as_callable`), and this continuation
/// just forwards whatever it settles with straight through — `eval`'s result
/// IS the program's result. Mirrors `IdentityContinuation` in
/// `sema-stdlib::list` (`apply`'s cooperative path). Holds no state, so
/// nothing to trace.
struct EvalProgramContinuation;

impl sema_core::runtime::Trace for EvalProgramContinuation {
    fn trace(&self, _sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        true
    }
}

impl sema_core::runtime::NativeContinuation for EvalProgramContinuation {
    fn resume(
        self: Box<Self>,
        _context: &mut sema_core::runtime::NativeCallContext<'_>,
        input: sema_core::runtime::ResumeInput,
    ) -> sema_core::runtime::NativeResult {
        use sema_core::runtime::{NativeOutcome, ResumeInput};
        match input {
            ResumeInput::Returned(value) => Ok(NativeOutcome::Return(value)),
            ResumeInput::Failed(error) => Err(error),
            ResumeInput::Cancelled(reason) => {
                Err(SemaError::eval(format!("eval was cancelled ({reason:?})")))
            }
            ResumeInput::Runtime(_) => Err(SemaError::eval(
                "eval continuation received an unexpected runtime response",
            )),
        }
    }
}

fn runtime_module_state(
    context: &sema_core::runtime::NativeCallContext<'_>,
) -> Result<Rc<sema_core::runtime::ModuleTaskState>, SemaError> {
    context
        .task_context
        .get_rc::<sema_core::runtime::ModuleTaskState>()
        .ok_or_else(|| SemaError::eval("runtime module state is unavailable"))
}

fn check_runtime_module_cycle(
    context: &sema_core::runtime::NativeCallContext<'_>,
    identity: &std::path::Path,
) -> Result<(), SemaError> {
    let loading = runtime_module_state(context)?.loading();
    let Some(cycle_start) = loading.iter().position(|candidate| candidate == identity) else {
        return Ok(());
    };
    let mut cycle: Vec<String> = loading[cycle_start..]
        .iter()
        .map(|path| path.display().to_string())
        .collect();
    cycle.push(identity.display().to_string());
    Err(SemaError::eval(format!(
        "cyclic import detected: {}",
        cycle.join(" -> ")
    )))
}

struct ImportGateEntry {
    gate: sema_core::runtime::ResourceGateHandle,
    active_calls: usize,
}

impl Drop for ImportGateEntry {
    fn drop(&mut self) {
        let _ = self.gate.close();
    }
}

/// Per-interpreter single-flight gates serialize cache misses for one exact
/// module identity. The map owns no Sema values or environments; the fallback
/// env is weak, so the payload adds no cycle-collector edges.
struct ImportRuntimeState {
    import_env: Weak<Env>,
    self_ref: Weak<Self>,
    gates: RefCell<HashMap<std::path::PathBuf, ImportGateEntry>>,
}

struct ImportGateLease {
    state: Weak<ImportRuntimeState>,
    identity: std::path::PathBuf,
    gate: sema_core::runtime::ResourceGateId,
}

impl Drop for ImportGateLease {
    fn drop(&mut self) {
        let Some(state) = self.state.upgrade() else {
            return;
        };
        let mut gates = state.gates.borrow_mut();
        let remove = gates.get_mut(&self.identity).is_some_and(|entry| {
            if entry.gate.id() != self.gate {
                return false;
            }
            entry.active_calls = entry.active_calls.saturating_sub(1);
            entry.active_calls == 0
        });
        if remove {
            gates.remove(&self.identity);
        }
    }
}

impl ImportRuntimeState {
    fn new(import_env: Weak<Env>) -> Rc<Self> {
        Rc::new_cyclic(|self_ref| Self {
            import_env,
            self_ref: self_ref.clone(),
            gates: RefCell::new(HashMap::new()),
        })
    }

    fn active_gate(
        &self,
        identity: &std::path::Path,
    ) -> Result<Option<(sema_core::runtime::ResourceGateHandle, ImportGateLease)>, SemaError> {
        let Some(runtime) = sema_core::current_root().map(|root| root.runtime()) else {
            return Ok(None);
        };
        let mut gates = self.gates.borrow_mut();
        let Some(entry) = gates.get_mut(identity) else {
            return Ok(None);
        };
        if entry.gate.id().runtime() != runtime {
            return Err(SemaError::eval(
                "import: module is already loading in another runtime",
            ));
        }
        entry.active_calls += 1;
        Ok(Some((
            entry.gate.clone(),
            ImportGateLease {
                state: self.self_ref.clone(),
                identity: identity.to_path_buf(),
                gate: entry.gate.id(),
            },
        )))
    }

    fn reject_legacy_overlap(&self, identity: &std::path::Path) -> Result<(), SemaError> {
        if self
            .gates
            .borrow()
            .get(identity)
            .is_some_and(|entry| entry.active_calls > 0)
        {
            return Err(SemaError::eval(
                "import: module is already active in the cooperative runtime",
            ));
        }
        Ok(())
    }

    fn install_gate(
        &self,
        identity: &std::path::Path,
        gate: sema_core::runtime::ResourceGateHandle,
    ) -> Result<(sema_core::runtime::ResourceGateHandle, ImportGateLease), SemaError> {
        match self.active_gate(identity) {
            Ok(Some(existing)) => {
                let _ = gate.close();
                return Ok(existing);
            }
            Err(error) => {
                let _ = gate.close();
                return Err(error);
            }
            Ok(None) => {}
        }
        let gate_id = gate.id();
        self.gates.borrow_mut().insert(
            identity.to_path_buf(),
            ImportGateEntry {
                gate: gate.clone(),
                active_calls: 1,
            },
        );
        Ok((
            gate,
            ImportGateLease {
                state: self.self_ref.clone(),
                identity: identity.to_path_buf(),
                gate: gate_id,
            },
        ))
    }
}

struct PendingRuntimeImport {
    module: special_forms::ResolvedModuleBytes,
    selective: Vec<String>,
    target: Rc<Env>,
}

impl sema_core::runtime::Trace for PendingRuntimeImport {
    fn trace(&self, sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        sink(sema_core::cycle::GcEdge::Env(&self.target));
        true
    }
}

struct ImportGateCreated {
    state: Weak<ImportRuntimeState>,
    pending: PendingRuntimeImport,
}

impl sema_core::runtime::Trace for ImportGateCreated {
    fn trace(&self, sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        self.pending.trace(sink)
    }
}

impl sema_core::runtime::NativeContinuation for ImportGateCreated {
    fn resume(
        self: Box<Self>,
        _context: &mut sema_core::runtime::NativeCallContext<'_>,
        input: sema_core::runtime::ResumeInput,
    ) -> sema_core::runtime::NativeResult {
        use sema_core::runtime::{ResumeInput, RuntimeResponse};

        let ResumeInput::Runtime(RuntimeResponse::ResourceGate(gate)) = input else {
            return match input {
                ResumeInput::Failed(error) => Err(error),
                ResumeInput::Cancelled(reason) => Err(SemaError::eval(format!(
                    "import gate allocation was cancelled ({reason:?})"
                ))),
                _ => Err(SemaError::eval(
                    "import gate allocation returned an unexpected runtime response",
                )),
            };
        };
        let Some(state) = self.state.upgrade() else {
            let _ = gate.close();
            return Err(SemaError::eval("import runtime state is unavailable"));
        };
        let (gate, lease) = state.install_gate(&self.pending.module.identity, gate)?;
        acquire_import_gate(self.pending, gate.id(), lease)
    }
}

fn acquire_import_gate(
    pending: PendingRuntimeImport,
    gate: sema_core::runtime::ResourceGateId,
    lease: ImportGateLease,
) -> sema_core::runtime::NativeResult {
    use sema_core::runtime::{NativeOutcome, NativeSuspend, WaitKind};

    Ok(NativeOutcome::Suspend(NativeSuspend {
        wait: WaitKind::ResourceSlot(gate),
        continuation: Box::new(ImportGateAcquired {
            pending,
            gate,
            lease,
        }),
    }))
}

struct ImportGateAcquired {
    pending: PendingRuntimeImport,
    gate: sema_core::runtime::ResourceGateId,
    lease: ImportGateLease,
}

impl sema_core::runtime::Trace for ImportGateAcquired {
    fn trace(&self, sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        self.pending.trace(sink)
    }
}

impl sema_core::runtime::NativeContinuation for ImportGateAcquired {
    fn resume(
        self: Box<Self>,
        context: &mut sema_core::runtime::NativeCallContext<'_>,
        input: sema_core::runtime::ResumeInput,
    ) -> sema_core::runtime::NativeResult {
        use sema_core::runtime::{ResumeInput, RuntimeResponse};

        match input {
            ResumeInput::Runtime(RuntimeResponse::Value(_)) => {
                start_import_owner(context, self.pending, self.gate, self.lease)
            }
            ResumeInput::Failed(error) => Err(error),
            ResumeInput::Cancelled(reason) => Err(SemaError::eval(format!(
                "import gate acquisition was cancelled ({reason:?})"
            ))),
            _ => Err(SemaError::eval(
                "import gate acquisition returned an unexpected runtime response",
            )),
        }
    }
}

struct ImportReleaseContinuation {
    outcome: sema_core::runtime::TaskOutcome,
    _lease: ImportGateLease,
}

impl sema_core::runtime::Trace for ImportReleaseContinuation {
    fn trace(&self, sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        self.outcome.trace(sink)
    }
}

impl sema_core::runtime::NativeContinuation for ImportReleaseContinuation {
    fn resume(
        self: Box<Self>,
        _context: &mut sema_core::runtime::NativeCallContext<'_>,
        input: sema_core::runtime::ResumeInput,
    ) -> sema_core::runtime::NativeResult {
        use sema_core::runtime::{NativeOutcome, ResumeInput, RuntimeResponse, TaskOutcome};

        match input {
            ResumeInput::Runtime(RuntimeResponse::Value(_)) => match self.outcome {
                TaskOutcome::Returned(value) => Ok(NativeOutcome::Return(value)),
                TaskOutcome::Failed(error) => Err(error),
                TaskOutcome::Cancelled(reason) => Err(SemaError::eval(format!(
                    "import was cancelled ({reason:?})"
                ))),
            },
            ResumeInput::Failed(error) => Err(error),
            ResumeInput::Cancelled(reason) => Err(SemaError::eval(format!(
                "import gate release was cancelled ({reason:?})"
            ))),
            _ => Err(SemaError::eval(
                "import gate release returned an unexpected runtime response",
            )),
        }
    }
}

fn release_import_gate(
    gate: sema_core::runtime::ResourceGateId,
    outcome: sema_core::runtime::TaskOutcome,
    lease: ImportGateLease,
) -> sema_core::runtime::NativeResult {
    use sema_core::runtime::{NativeOutcome, RuntimeRequest};

    Ok(NativeOutcome::Runtime(
        RuntimeRequest::ReleaseResourceGate {
            gate,
            continuation: Box::new(ImportReleaseContinuation {
                outcome,
                _lease: lease,
            }),
        },
    ))
}

/// Exact task-local module scopes retained across structural calls. Scope IDs
/// let `Drop` remove this invocation's entries even when cancellation unwinds
/// the continuation instead of returning through its normal completion path.
struct RuntimeModuleScope {
    state: Rc<sema_core::runtime::ModuleTaskState>,
    loading: sema_core::runtime::ScopeId,
    current_file: sema_core::runtime::ScopeId,
    exports: Option<sema_core::runtime::ScopeId>,
}

impl RuntimeModuleScope {
    fn enter(
        context: &mut sema_core::runtime::NativeCallContext<'_>,
        identity: &std::path::Path,
        file_path: std::path::PathBuf,
        tracks_exports: bool,
    ) -> Result<Self, SemaError> {
        check_runtime_module_cycle(context, identity)?;
        let state = runtime_module_state(context)?;

        let loading_scope = state
            .push_loading(identity.to_path_buf())
            .map_err(|error| SemaError::eval(format!("module load scope: {error}")))?;
        let current_file = match state.push_current_file(file_path) {
            Ok(scope) => scope,
            Err(error) => {
                state.remove_loading(loading_scope);
                return Err(SemaError::eval(format!(
                    "module current-file scope: {error}"
                )));
            }
        };
        let exports = if tracks_exports {
            match state.push_exports(None) {
                Ok(scope) => Some(scope),
                Err(error) => {
                    state.remove_current_file(current_file);
                    state.remove_loading(loading_scope);
                    return Err(SemaError::eval(format!("module export scope: {error}")));
                }
            }
        } else {
            None
        };

        Ok(Self {
            state,
            loading: loading_scope,
            current_file,
            exports,
        })
    }

    fn take_exports(&mut self) -> Option<Vec<String>> {
        self.exports
            .take()
            .and_then(|scope| self.state.take_exports(scope))
            .flatten()
    }
}

impl Drop for RuntimeModuleScope {
    fn drop(&mut self) {
        if let Some(scope) = self.exports.take() {
            self.state.remove_exports(scope);
        }
        self.state.remove_current_file(self.current_file);
        self.state.remove_loading(self.loading);
    }
}

enum RuntimeModuleCompletion {
    Load,
    Import {
        identity: std::path::PathBuf,
        selective: Vec<String>,
        target: Rc<Env>,
        gate: sema_core::runtime::ResourceGateId,
        lease: ImportGateLease,
    },
}

impl RuntimeModuleCompletion {
    fn trace(&self, sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) {
        if let Self::Import { target, .. } = self {
            sink(sema_core::cycle::GcEdge::Env(target));
        }
    }

    fn fail(self, error: SemaError) -> sema_core::runtime::NativeResult {
        match self {
            Self::Load => Err(error),
            Self::Import { gate, lease, .. } => {
                release_import_gate(gate, sema_core::runtime::TaskOutcome::Failed(error), lease)
            }
        }
    }

    fn cancelled(
        self,
        reason: sema_core::runtime::CancelReason,
    ) -> sema_core::runtime::NativeResult {
        drop(self);
        Err(SemaError::eval(format!(
            "module evaluation was cancelled ({reason:?})"
        )))
    }
}

/// State common to source and bytecode module execution. Import cache/export
/// publication happens only in `finish`, after the complete body has returned.
struct RuntimeModuleRun {
    scope: RuntimeModuleScope,
    globals: Rc<Env>,
    completion: RuntimeModuleCompletion,
}

impl RuntimeModuleRun {
    fn finish(mut self, context: &EvalContext, value: Value) -> sema_core::runtime::NativeResult {
        use sema_core::runtime::NativeOutcome;

        match self.completion {
            RuntimeModuleCompletion::Load => Ok(NativeOutcome::Return(value)),
            RuntimeModuleCompletion::Import {
                identity,
                selective,
                target,
                gate,
                lease,
            } => {
                let declared = self.scope.take_exports();
                let exports =
                    special_forms::collect_module_exports(&self.globals, declared.as_deref());
                context.cache_module(identity, exports.clone());
                let outcome =
                    match special_forms::copy_exports_to_env(&exports, &selective, &target) {
                        Ok(()) => sema_core::runtime::TaskOutcome::Returned(Value::nil()),
                        Err(error) => sema_core::runtime::TaskOutcome::Failed(error),
                    };
                release_import_gate(gate, outcome, lease)
            }
        }
    }

    fn fail(self, error: SemaError) -> sema_core::runtime::NativeResult {
        self.completion.fail(error)
    }

    fn cancelled(
        self,
        reason: sema_core::runtime::CancelReason,
    ) -> sema_core::runtime::NativeResult {
        self.completion.cancelled(reason)
    }

    fn trace(&self, sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) {
        sink(sema_core::cycle::GcEdge::Env(&self.globals));
        self.completion.trace(sink);
    }
}

struct RuntimeSourceModule {
    run: RuntimeModuleRun,
    expressions: Vec<Value>,
    spans: sema_core::SpanMap,
    source_file: std::path::PathBuf,
    next: usize,
    last: Value,
}

impl RuntimeSourceModule {
    fn drive(
        mut self: Box<Self>,
        context: &mut sema_core::runtime::NativeCallContext<'_>,
    ) -> sema_core::runtime::NativeResult {
        use sema_core::runtime::{NativeCall, NativeOutcome};

        while let Some(expression) = self.expressions.get(self.next) {
            self.next += 1;
            let expanded =
                match expand_for_vm_in(context.eval_context, &self.run.globals, expression) {
                    Ok(expanded) => expanded,
                    Err(error) => return self.run.fail(error),
                };
            if expanded.is_nil() {
                continue;
            }
            let program = match sema_vm::compile_program_with_spans(
                std::slice::from_ref(&expanded),
                &self.spans,
                Some(self.source_file.clone()),
            ) {
                Ok(program) => program,
                Err(error) => return self.run.fail(error),
            };
            let callable = match sema_vm::program_as_callable(program, Rc::clone(&self.run.globals))
            {
                Ok(callable) => callable,
                Err(error) => return self.run.fail(error),
            };
            return Ok(NativeOutcome::Call(NativeCall {
                callable,
                args: Vec::new(),
                continuation: self,
            }));
        }

        let Self { run, last, .. } = *self;
        run.finish(context.eval_context, last)
    }
}

impl sema_core::runtime::Trace for RuntimeSourceModule {
    fn trace(&self, sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        self.run.trace(sink);
        for expression in &self.expressions {
            sink(sema_core::cycle::GcEdge::Value(expression));
        }
        sink(sema_core::cycle::GcEdge::Value(&self.last));
        true
    }
}

impl sema_core::runtime::NativeContinuation for RuntimeSourceModule {
    fn resume(
        mut self: Box<Self>,
        context: &mut sema_core::runtime::NativeCallContext<'_>,
        input: sema_core::runtime::ResumeInput,
    ) -> sema_core::runtime::NativeResult {
        use sema_core::runtime::ResumeInput;

        match input {
            ResumeInput::Returned(value) => self.last = value,
            ResumeInput::Failed(error) => return self.run.fail(error),
            ResumeInput::Cancelled(reason) => return self.run.cancelled(reason),
            ResumeInput::Runtime(_) => {
                return Err(SemaError::eval(
                    "module continuation received an unexpected runtime response",
                ))
            }
        }
        self.drive(context)
    }
}

struct RuntimeBytecodeModule {
    run: RuntimeModuleRun,
}

impl sema_core::runtime::Trace for RuntimeBytecodeModule {
    fn trace(&self, sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        self.run.trace(sink);
        true
    }
}

impl sema_core::runtime::NativeContinuation for RuntimeBytecodeModule {
    fn resume(
        self: Box<Self>,
        context: &mut sema_core::runtime::NativeCallContext<'_>,
        input: sema_core::runtime::ResumeInput,
    ) -> sema_core::runtime::NativeResult {
        use sema_core::runtime::ResumeInput;

        match input {
            ResumeInput::Returned(value) => self.run.finish(context.eval_context, value),
            ResumeInput::Failed(error) => self.run.fail(error),
            ResumeInput::Cancelled(reason) => self.run.cancelled(reason),
            ResumeInput::Runtime(_) => Err(SemaError::eval(
                "module continuation received an unexpected runtime response",
            )),
        }
    }
}

fn start_runtime_module(
    context: &mut sema_core::runtime::NativeCallContext<'_>,
    module: special_forms::ResolvedModuleBytes,
    globals: Rc<Env>,
    completion: RuntimeModuleCompletion,
) -> sema_core::runtime::NativeResult {
    use sema_core::runtime::{NativeCall, NativeOutcome};

    let tracks_exports = matches!(completion, RuntimeModuleCompletion::Import { .. });
    let special_forms::ResolvedModuleBytes {
        requested,
        identity,
        file_path,
        bytes,
    } = module;
    let scope =
        match RuntimeModuleScope::enter(context, &identity, file_path.clone(), tracks_exports) {
            Ok(scope) => scope,
            Err(error) => return completion.fail(error),
        };
    let run = RuntimeModuleRun {
        scope,
        globals,
        completion,
    };

    if sema_vm::is_bytecode_file(&bytes) {
        let compiled = match sema_vm::deserialize_from_bytes(&bytes) {
            Ok(compiled) => compiled,
            Err(error) => return run.fail(error),
        };
        let program = compiled_program_from_result(compiled);
        let callable = match sema_vm::program_as_callable(program, Rc::clone(&run.globals)) {
            Ok(callable) => callable,
            Err(error) => return run.fail(error),
        };
        return Ok(NativeOutcome::Call(NativeCall {
            callable,
            args: Vec::new(),
            continuation: Box::new(RuntimeBytecodeModule { run }),
        }));
    }

    let source = match String::from_utf8(bytes) {
        Ok(source) => source,
        Err(error) => {
            let message = if tracks_exports {
                format!("import {requested}: invalid UTF-8 in module: {error}")
            } else {
                format!("load {requested}: invalid UTF-8: {error}")
            };
            return run.fail(SemaError::Io(message));
        }
    };
    let (expressions, spans) = match sema_reader::read_many_with_spans(&source) {
        Ok(parsed) => parsed,
        Err(error) => return run.fail(error),
    };
    context.eval_context.merge_span_table(spans.clone());
    Box::new(RuntimeSourceModule {
        run,
        expressions,
        spans,
        source_file: file_path,
        next: 0,
        last: Value::nil(),
    })
    .drive(context)
}

fn runtime_delegate_target(
    context: &sema_core::runtime::NativeCallContext<'_>,
    fallback: &Weak<Env>,
) -> Result<Rc<Env>, SemaError> {
    context
        .call_env
        .clone()
        .map_or_else(|| upgrade_delegate_env(fallback), Ok)
}

fn legacy_delegate_target(
    context: &EvalContext,
    fallback: &Weak<Env>,
) -> Result<Rc<Env>, SemaError> {
    context
        .legacy_call_env()
        .map_or_else(|| upgrade_delegate_env(fallback), Ok)
}

fn runtime_load(
    fallback: &Weak<Env>,
    context: &mut sema_core::runtime::NativeCallContext<'_>,
    args: &[Value],
) -> sema_core::runtime::NativeResult {
    let target = runtime_delegate_target(context, fallback)?;
    let module = special_forms::prepare_load(args, context.eval_context)?;
    start_runtime_module(context, module, target, RuntimeModuleCompletion::Load)
}

fn import_args(args: &[Value]) -> Result<Vec<Value>, SemaError> {
    if args.len() != 2 {
        return Err(SemaError::arity("import", "2", args.len()));
    }
    let mut prepared = vec![args[0].clone()];
    if let Some(items) = args[1].as_list() {
        prepared.extend(items.iter().cloned());
    }
    Ok(prepared)
}

fn runtime_import(
    state: &ImportRuntimeState,
    context: &mut sema_core::runtime::NativeCallContext<'_>,
    args: &[Value],
) -> sema_core::runtime::NativeResult {
    use sema_core::runtime::{NativeOutcome, RuntimeRequest};

    let target = runtime_delegate_target(context, &state.import_env)?;
    let args = import_args(args)?;
    match special_forms::prepare_import(&args, context.eval_context)? {
        special_forms::ImportPreparation::Cached { exports, selective } => {
            special_forms::copy_exports_to_env(&exports, &selective, &target)?;
            Ok(NativeOutcome::Return(Value::nil()))
        }
        special_forms::ImportPreparation::Uncached { module, selective } => {
            check_runtime_module_cycle(context, &module.identity)?;
            let pending = PendingRuntimeImport {
                module,
                selective,
                target,
            };
            if let Some((gate, lease)) = state.active_gate(&pending.module.identity)? {
                return acquire_import_gate(pending, gate.id(), lease);
            }
            Ok(NativeOutcome::Runtime(RuntimeRequest::CreateResourceGate {
                continuation: Box::new(ImportGateCreated {
                    state: state.self_ref.clone(),
                    pending,
                }),
            }))
        }
    }
}

fn start_import_owner(
    context: &mut sema_core::runtime::NativeCallContext<'_>,
    pending: PendingRuntimeImport,
    gate: sema_core::runtime::ResourceGateId,
    lease: ImportGateLease,
) -> sema_core::runtime::NativeResult {
    if let Some(exports) = context
        .eval_context
        .get_cached_module(&pending.module.identity)
    {
        let outcome =
            match special_forms::copy_exports_to_env(&exports, &pending.selective, &pending.target)
            {
                Ok(()) => sema_core::runtime::TaskOutcome::Returned(Value::nil()),
                Err(error) => sema_core::runtime::TaskOutcome::Failed(error),
            };
        return release_import_gate(gate, outcome, lease);
    }

    let module_env = Rc::new(create_module_env(&pending.target));
    let completion = RuntimeModuleCompletion::Import {
        identity: pending.module.identity.clone(),
        selective: pending.selective,
        target: pending.target,
        gate,
        lease,
    };
    start_runtime_module(context, pending.module, module_env, completion)
}

fn legacy_import(
    state: &ImportRuntimeState,
    context: &EvalContext,
    args: &[Value],
) -> Result<Value, SemaError> {
    let args = import_args(args)?;
    let target = legacy_delegate_target(context, &state.import_env)?;
    let result = match special_forms::prepare_import(&args, context)? {
        special_forms::ImportPreparation::Cached { exports, selective } => {
            special_forms::copy_exports_to_env(&exports, &selective, &target)?;
            Trampoline::Value(Value::nil())
        }
        special_forms::ImportPreparation::Uncached { module, selective } => {
            state.reject_legacy_overlap(&module.identity)?;
            special_forms::eval_prepared_import(module, &selective, &target, context)?
        }
    };
    match result {
        Trampoline::Value(value) => Ok(value),
        Trampoline::Eval(..) => Ok(Value::nil()),
    }
}

struct ForceThunkRoot(Value);

impl sema_core::runtime::Trace for ForceThunkRoot {
    fn trace(&self, sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        sink(sema_core::cycle::GcEdge::Value(&self.0));
        true
    }
}

struct ForceGateEntry {
    thunk: Weak<Thunk>,
    gate: sema_core::runtime::ResourceGateHandle,
    active_calls: usize,
}

impl Drop for ForceGateEntry {
    fn drop(&mut self) {
        let _ = self.gate.close();
    }
}

/// Per-interpreter resource gates serialize concurrent force calls for one
/// thunk. One lease follows each call through its continuations; the last lease
/// removes and closes the gate entry. Weak thunk identities prevent the
/// registry itself from becoming an untraceable `Value` root.
struct ForceRuntimeState {
    force_env: Weak<Env>,
    self_ref: Weak<Self>,
    gates: RefCell<HashMap<sema_core::NodePtr, ForceGateEntry>>,
    legacy_calls: RefCell<HashMap<sema_core::NodePtr, Weak<Thunk>>>,
}

struct ForceGateLease {
    state: Weak<ForceRuntimeState>,
    thunk: sema_core::NodePtr,
    gate: sema_core::runtime::ResourceGateId,
}

struct LegacyForceLease {
    state: Weak<ForceRuntimeState>,
    thunk: sema_core::NodePtr,
}

impl Drop for LegacyForceLease {
    fn drop(&mut self) {
        if let Some(state) = self.state.upgrade() {
            state.legacy_calls.borrow_mut().remove(&self.thunk);
        }
    }
}

impl Drop for ForceGateLease {
    fn drop(&mut self) {
        let Some(state) = self.state.upgrade() else {
            return;
        };
        let mut gates = state.gates.borrow_mut();
        let remove = gates.get_mut(&self.thunk).is_some_and(|entry| {
            if entry.gate.id() != self.gate {
                return false;
            }
            entry.active_calls = entry.active_calls.saturating_sub(1);
            entry.active_calls == 0
        });
        if remove {
            gates.remove(&self.thunk);
        }
    }
}

impl ForceRuntimeState {
    fn new(force_env: Weak<Env>) -> Rc<Self> {
        Rc::new_cyclic(|self_ref| Self {
            force_env,
            self_ref: self_ref.clone(),
            gates: RefCell::new(HashMap::new()),
            legacy_calls: RefCell::new(HashMap::new()),
        })
    }

    fn active_gate(
        &self,
        thunk: &Rc<Thunk>,
    ) -> Result<Option<(sema_core::runtime::ResourceGateHandle, ForceGateLease)>, SemaError> {
        let key = sema_core::NodePtr::of_rc(thunk);
        if self.has_active_legacy_force(thunk) {
            return Err(SemaError::eval(
                "force: delayed promise is already active in the synchronous evaluator",
            ));
        }
        let Some(runtime) = sema_core::current_root().map(|root| root.runtime()) else {
            return Ok(None);
        };
        let mut gates = self.gates.borrow_mut();
        let Some(entry) = gates.get_mut(&key) else {
            return Ok(None);
        };
        if !Weak::ptr_eq(&entry.thunk, &Rc::downgrade(thunk)) {
            gates.remove(&key);
            return Ok(None);
        }
        if entry.gate.id().runtime() != runtime {
            return Err(SemaError::eval(
                "force: delayed promise is already active in another runtime",
            ));
        }
        entry.active_calls += 1;
        Ok(Some((
            entry.gate.clone(),
            ForceGateLease {
                state: self.self_ref.clone(),
                thunk: key,
                gate: entry.gate.id(),
            },
        )))
    }

    fn install_gate(
        &self,
        thunk: &Rc<Thunk>,
        gate: sema_core::runtime::ResourceGateHandle,
    ) -> Result<(sema_core::runtime::ResourceGateHandle, ForceGateLease), SemaError> {
        match self.active_gate(thunk) {
            Ok(Some(existing)) => {
                let _ = gate.close();
                return Ok(existing);
            }
            Err(error) => {
                let _ = gate.close();
                return Err(error);
            }
            Ok(None) => {}
        }
        let key = sema_core::NodePtr::of_rc(thunk);
        let gate_id = gate.id();
        self.gates.borrow_mut().insert(
            key,
            ForceGateEntry {
                thunk: Rc::downgrade(thunk),
                gate: gate.clone(),
                active_calls: 1,
            },
        );
        Ok((
            gate,
            ForceGateLease {
                state: self.self_ref.clone(),
                thunk: key,
                gate: gate_id,
            },
        ))
    }

    fn has_active_force(&self, thunk: &Rc<Thunk>) -> bool {
        let key = sema_core::NodePtr::of_rc(thunk);
        self.gates.borrow().get(&key).is_some_and(|entry| {
            entry.active_calls > 0 && Weak::ptr_eq(&entry.thunk, &Rc::downgrade(thunk))
        })
    }

    fn has_active_legacy_force(&self, thunk: &Rc<Thunk>) -> bool {
        let key = sema_core::NodePtr::of_rc(thunk);
        self.legacy_calls
            .borrow()
            .get(&key)
            .is_some_and(|active| Weak::ptr_eq(active, &Rc::downgrade(thunk)))
    }

    fn begin_legacy_force(&self, thunk: &Rc<Thunk>) -> Result<LegacyForceLease, SemaError> {
        if self.has_active_force(thunk) {
            return Err(SemaError::eval(
                "force: delayed promise is already active in the cooperative runtime",
            ));
        }
        if self.has_active_legacy_force(thunk) {
            return Err(SemaError::eval(
                "force: delayed promise is already active in the synchronous evaluator",
            ));
        }
        let key = sema_core::NodePtr::of_rc(thunk);
        self.legacy_calls
            .borrow_mut()
            .insert(key, Rc::downgrade(thunk));
        Ok(LegacyForceLease {
            state: self.self_ref.clone(),
            thunk: key,
        })
    }
}

struct ForceGateCreated {
    state: Weak<ForceRuntimeState>,
    thunk: ForceThunkRoot,
}

impl sema_core::runtime::Trace for ForceGateCreated {
    fn trace(&self, sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        self.thunk.trace(sink)
    }
}

impl sema_core::runtime::NativeContinuation for ForceGateCreated {
    fn resume(
        self: Box<Self>,
        _context: &mut sema_core::runtime::NativeCallContext<'_>,
        input: sema_core::runtime::ResumeInput,
    ) -> sema_core::runtime::NativeResult {
        use sema_core::runtime::{ResumeInput, RuntimeResponse};

        let ResumeInput::Runtime(RuntimeResponse::ResourceGate(gate)) = input else {
            return match input {
                ResumeInput::Failed(error) => Err(error),
                ResumeInput::Cancelled(reason) => Err(SemaError::eval(format!(
                    "force gate allocation was cancelled ({reason:?})"
                ))),
                _ => Err(SemaError::eval(
                    "force gate allocation returned an unexpected runtime response",
                )),
            };
        };
        let Some(state) = self.state.upgrade() else {
            let _ = gate.close();
            return Err(SemaError::eval("force runtime state is unavailable"));
        };
        let thunk = self.thunk.0.as_thunk_rc().ok_or_else(|| {
            let _ = gate.close();
            SemaError::eval("force gate allocation lost its delayed promise")
        })?;
        let (gate, lease) = state.install_gate(&thunk, gate)?;
        acquire_force_gate(self.thunk, gate.id(), state.force_env.clone(), lease)
    }
}

fn acquire_force_gate(
    thunk: ForceThunkRoot,
    gate: sema_core::runtime::ResourceGateId,
    force_env: Weak<Env>,
    lease: ForceGateLease,
) -> sema_core::runtime::NativeResult {
    use sema_core::runtime::{NativeOutcome, NativeSuspend, WaitKind};

    Ok(NativeOutcome::Suspend(NativeSuspend {
        wait: WaitKind::ResourceSlot(gate),
        continuation: Box::new(ForceGateAcquired {
            thunk,
            gate,
            force_env,
            lease,
        }),
    }))
}

struct ForceGateAcquired {
    thunk: ForceThunkRoot,
    gate: sema_core::runtime::ResourceGateId,
    force_env: Weak<Env>,
    lease: ForceGateLease,
}

impl sema_core::runtime::Trace for ForceGateAcquired {
    fn trace(&self, sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        self.thunk.trace(sink)
    }
}

impl sema_core::runtime::NativeContinuation for ForceGateAcquired {
    fn resume(
        self: Box<Self>,
        context: &mut sema_core::runtime::NativeCallContext<'_>,
        input: sema_core::runtime::ResumeInput,
    ) -> sema_core::runtime::NativeResult {
        use sema_core::runtime::{ResumeInput, RuntimeResponse};

        match input {
            ResumeInput::Runtime(RuntimeResponse::Value(_)) => {
                start_forced_body(context, self.thunk, self.gate, self.force_env, self.lease)
            }
            ResumeInput::Failed(error) => Err(error),
            ResumeInput::Cancelled(reason) => Err(SemaError::eval(format!(
                "force gate acquisition was cancelled ({reason:?})"
            ))),
            _ => Err(SemaError::eval(
                "force gate acquisition returned an unexpected runtime response",
            )),
        }
    }
}

fn release_force_gate(
    gate: sema_core::runtime::ResourceGateId,
    outcome: sema_core::runtime::TaskOutcome,
    lease: ForceGateLease,
) -> sema_core::runtime::NativeResult {
    use sema_core::runtime::{NativeOutcome, RuntimeRequest};

    Ok(NativeOutcome::Runtime(
        RuntimeRequest::ReleaseResourceGate {
            gate,
            continuation: Box::new(ForceReleaseContinuation {
                outcome,
                _lease: lease,
            }),
        },
    ))
}

struct ForceReleaseContinuation {
    outcome: sema_core::runtime::TaskOutcome,
    _lease: ForceGateLease,
}

impl sema_core::runtime::Trace for ForceReleaseContinuation {
    fn trace(&self, sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        self.outcome.trace(sink)
    }
}

impl sema_core::runtime::NativeContinuation for ForceReleaseContinuation {
    fn resume(
        self: Box<Self>,
        _context: &mut sema_core::runtime::NativeCallContext<'_>,
        input: sema_core::runtime::ResumeInput,
    ) -> sema_core::runtime::NativeResult {
        use sema_core::runtime::{NativeOutcome, ResumeInput, RuntimeResponse, TaskOutcome};

        match input {
            ResumeInput::Runtime(RuntimeResponse::Value(_)) => match self.outcome {
                TaskOutcome::Returned(value) => Ok(NativeOutcome::Return(value)),
                TaskOutcome::Failed(error) => Err(error),
                TaskOutcome::Cancelled(reason) => Err(SemaError::eval(format!(
                    "force body was cancelled ({reason:?})"
                ))),
            },
            ResumeInput::Failed(error) => Err(error),
            ResumeInput::Cancelled(reason) => Err(SemaError::eval(format!(
                "force gate release was cancelled ({reason:?})"
            ))),
            _ => Err(SemaError::eval(
                "force gate release returned an unexpected runtime response",
            )),
        }
    }
}

/// Completes the single cooperative evaluation that owns a thunk's force gate.
/// Only a normal return populates the memo cell; every terminal path releases
/// the gate or lets runtime cancellation teardown transfer it to a waiter.
struct ForceContinuation {
    thunk: ForceThunkRoot,
    gate: sema_core::runtime::ResourceGateId,
    lease: ForceGateLease,
}

impl sema_core::runtime::Trace for ForceContinuation {
    fn trace(&self, sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        self.thunk.trace(sink)
    }
}

impl sema_core::runtime::NativeContinuation for ForceContinuation {
    fn resume(
        self: Box<Self>,
        _context: &mut sema_core::runtime::NativeCallContext<'_>,
        input: sema_core::runtime::ResumeInput,
    ) -> sema_core::runtime::NativeResult {
        use sema_core::runtime::ResumeInput;

        match input {
            ResumeInput::Returned(value) => {
                let thunk = self.thunk.0.as_thunk_rc().ok_or_else(|| {
                    SemaError::eval("force continuation lost its delayed promise")
                })?;
                let mut forced = thunk.forced.borrow_mut();
                let value = forced.get_or_insert_with(|| value.clone()).clone();
                drop(forced);
                release_force_gate(
                    self.gate,
                    sema_core::runtime::TaskOutcome::Returned(value),
                    self.lease,
                )
            }
            ResumeInput::Failed(error) => release_force_gate(
                self.gate,
                sema_core::runtime::TaskOutcome::Failed(error),
                self.lease,
            ),
            ResumeInput::Cancelled(reason) => Err(SemaError::eval(format!(
                "force body was cancelled ({reason:?})"
            ))),
            ResumeInput::Runtime(_) => Err(SemaError::eval(
                "force continuation received an unexpected runtime response",
            )),
        }
    }
}

fn force_thunk(args: &[Value]) -> Result<Rc<Thunk>, SemaError> {
    if args.len() != 1 {
        return Err(SemaError::arity("force", "1", args.len()));
    }
    args[0].as_thunk_rc().ok_or_else(|| {
        SemaError::type_error("thunk", args[0].type_name()).with_hint(
            "force: argument must be a (delay ...) or promise — non-promise values are an error",
        )
    })
}

fn force_runtime_call(
    state: &ForceRuntimeState,
    _context: &mut sema_core::runtime::NativeCallContext<'_>,
    args: &[Value],
) -> sema_core::runtime::NativeResult {
    use sema_core::runtime::{NativeOutcome, RuntimeRequest};

    let thunk = force_thunk(args)?;
    if let Some(value) = thunk.forced.borrow().as_ref() {
        return Ok(NativeOutcome::Return(value.clone()));
    }

    if let Some((gate, lease)) = state.active_gate(&thunk)? {
        return acquire_force_gate(
            ForceThunkRoot(args[0].clone()),
            gate.id(),
            state.force_env.clone(),
            lease,
        );
    }

    Ok(NativeOutcome::Runtime(RuntimeRequest::CreateResourceGate {
        continuation: Box::new(ForceGateCreated {
            state: state.self_ref.clone(),
            thunk: ForceThunkRoot(args[0].clone()),
        }),
    }))
}

fn start_forced_body(
    context: &mut sema_core::runtime::NativeCallContext<'_>,
    thunk_root: ForceThunkRoot,
    gate: sema_core::runtime::ResourceGateId,
    force_env: Weak<Env>,
    lease: ForceGateLease,
) -> sema_core::runtime::NativeResult {
    use sema_core::runtime::{NativeCall, NativeOutcome, TaskOutcome};

    let prepared = (|| -> Result<Option<Value>, SemaError> {
        let thunk = thunk_root
            .0
            .as_thunk_rc()
            .ok_or_else(|| SemaError::eval("force gate lost its delayed promise"))?;
        if thunk.forced.borrow().is_some() {
            return Ok(None);
        }

        if thunk.body.as_native_fn_rc().is_some() || thunk.body.as_lambda_rc().is_some() {
            return Ok(Some(thunk.body.clone()));
        }
        let force_env = upgrade_delegate_env(&force_env)?;
        let expanded = expand_for_vm_in(context.eval_context, &force_env, &thunk.body)?;
        if expanded.is_nil() {
            let value = Value::nil();
            *thunk.forced.borrow_mut() = Some(value);
            return Ok(None);
        }
        let program = sema_vm::compile_program(std::slice::from_ref(&expanded), None)?;
        sema_vm::program_as_callable(program, force_env).map(Some)
    })();

    let callable = match prepared {
        Ok(Some(callable)) => callable,
        Ok(None) => {
            let thunk = thunk_root
                .0
                .as_thunk_rc()
                .expect("prepared force retained its thunk");
            let value = thunk.forced.borrow().as_ref().cloned().ok_or_else(|| {
                SemaError::eval("force completed without a callable or memoized value")
            });
            return match value {
                Ok(value) => release_force_gate(gate, TaskOutcome::Returned(value), lease),
                Err(error) => release_force_gate(gate, TaskOutcome::Failed(error), lease),
            };
        }
        Err(error) => return release_force_gate(gate, TaskOutcome::Failed(error), lease),
    };

    Ok(NativeOutcome::Call(NativeCall {
        callable,
        args: Vec::new(),
        continuation: Box::new(ForceContinuation {
            thunk: thunk_root,
            gate,
            lease,
        }),
    }))
}

fn force_legacy(
    state: &ForceRuntimeState,
    context: &EvalContext,
    args: &[Value],
) -> Result<Value, SemaError> {
    let thunk = force_thunk(args)?;
    if let Some(value) = thunk.forced.borrow().as_ref() {
        return Ok(value.clone());
    }
    let _lease = state.begin_legacy_force(&thunk)?;
    let value = if thunk.body.as_native_fn_rc().is_some() || thunk.body.as_lambda_rc().is_some() {
        sema_core::call_callback(context, &thunk.body, &[])?
    } else {
        let force_env = upgrade_delegate_env(&state.force_env)?;
        eval_value_vm(context, &thunk.body, &force_env)?
    };
    *thunk.forced.borrow_mut() = Some(value.clone());
    Ok(value)
}

/// Register the `__vm-*` delegate natives into `env`.
///
/// Invariant I2 (CORE-2): each delegate's boxed closure captures the env it is
/// registered into WEAKLY (`Weak<Env>`), never strongly — a strong capture
/// would form an uncollectable `Env → NativeFn → Box<dyn Fn> → Env` cycle that
/// pins the entire environment past Interpreter teardown. Runtime delegates
/// receive the exact evaluator context and call environment through their
/// invocation context rather than capturing either owner.
pub fn register_vm_delegates(env: &Rc<Env>, _ctx: &Rc<EvalContext>) {
    // __vm-eval: macro-expand, compile, and run the expression on the bytecode
    // VM (rooted at the global env so top-level `define`s persist). The runtime
    // `(eval ...)` meta path is thus VM-native.
    //
    // Dual ABI (Step G): the legacy value ABI (`func`, below) is UNCHANGED —
    // a bare top-level `eval` or one reached from a nested synchronous
    // re-entry keeps running the eval'd form on a fresh, throwaway
    // `VM::execute`, exactly as before. The runtime ABI (`runtime`) only
    // takes over when `__vm-eval` is dispatched inside a live runtime
    // quantum (`dispatch_native`'s `runtime_quantum_active()` gate): macro
    // expansion and compilation stay synchronous (they need `EvalContext`,
    // which the runtime ABI is never handed — `NativeFn::invoke_runtime`
    // only forwards it to the legacy fallback), but EXECUTION of the
    // compiled program is handed to the runtime as an ordinary
    // `NativeOutcome::Call` callee (`sema_vm::program_as_callable`), exactly
    // like a HOF's callback (`MapContinuation` et al. in
    // `sema-stdlib::list`). This lets the runtime host a suspension
    // (`async/await`, `channel/*`, …) inside the eval'd form instead of the
    // fresh VM hitting a dead end with no scheduler attached.
    let eval_env = Rc::downgrade(env);
    let eval_env_runtime = Rc::downgrade(env);
    env.set(
        intern("__vm-eval"),
        Value::native_fn(NativeFn::with_ctx_runtime(
            "__vm-eval",
            move |ctx, args| {
                if args.len() != 1 {
                    return Err(SemaError::arity("eval", "1", args.len()));
                }
                let eval_env = upgrade_delegate_env(&eval_env)?;
                let expanded = expand_for_vm_in(ctx, &eval_env, &args[0])?;
                // A form that expands to nothing (e.g. a `defmacro`) yields nil.
                if expanded.is_nil() {
                    return Ok(Value::nil());
                }
                let prog = sema_vm::compile_program(std::slice::from_ref(&expanded), None)?;
                let mut vm =
                    sema_vm::VM::new(eval_env, prog.functions, &[], prog.main_cache_slots)?;
                vm.execute(prog.closure, ctx)
            },
            move |native_ctx, args| {
                if args.len() != 1 {
                    return Err(SemaError::arity("eval", "1", args.len()));
                }
                let eval_env = runtime_delegate_target(native_ctx, &eval_env_runtime)?;
                let expanded = expand_for_vm_in(native_ctx.eval_context, &eval_env, &args[0])?;
                if expanded.is_nil() {
                    return Ok(sema_core::runtime::NativeOutcome::Return(Value::nil()));
                }
                let prog = sema_vm::compile_program(std::slice::from_ref(&expanded), None)?;
                let callable = sema_vm::program_as_callable(prog, eval_env)?;
                Ok(sema_core::runtime::NativeOutcome::Call(
                    sema_core::runtime::NativeCall {
                        callable,
                        args: Vec::new(),
                        continuation: Box::new(EvalProgramContinuation),
                    },
                ))
            },
        )),
    );

    // __vm-module-exports: register a `(module name (export ...) ...)` form's
    // declared export list with the active module-load scope, so `import`
    // restricts the copied bindings to exactly those names. Without this the VM
    // exported every top-level binding (private helpers leaked). Mirrors the
    // module loader's `set_module_exports` call in eval_module.
    env.set(
        intern("__vm-module-exports"),
        Value::native_fn(NativeFn::with_ctx(
            "__vm-module-exports",
            move |ctx, args| {
                if args.len() != 1 {
                    return Err(SemaError::arity("module-exports", "1", args.len()));
                }
                let names: Vec<String> = match args[0].as_list() {
                    Some(items) => items
                        .iter()
                        .map(|v| {
                            v.as_symbol().map(|s| s.to_string()).ok_or_else(|| {
                                SemaError::eval("module: export names must be symbols")
                            })
                        })
                        .collect::<Result<_, _>>()?,
                    None => return Err(SemaError::type_error("list", args[0].type_name())),
                };
                ctx.set_module_exports(names);
                Ok(Value::nil())
            },
        )),
    );

    // __vm-load resolves and reads synchronously. Its runtime ABI keeps the
    // module path/cycle scopes in task-local state, then invokes each loaded
    // form structurally so timers, channels, and runtime-only natives can park.
    // The value ABI preserves synchronous nested-evaluator compatibility.
    let load_env = Rc::downgrade(env);
    let load_env_runtime = Rc::downgrade(env);
    env.set(
        intern("__vm-load"),
        Value::native_fn(NativeFn::with_ctx_runtime(
            "__vm-load",
            move |ctx, args| {
                if args.len() != 1 {
                    return Err(SemaError::arity("load", "1", args.len()));
                }
                let target = legacy_delegate_target(ctx, &load_env)?;
                match special_forms::eval_load(std::slice::from_ref(&args[0]), &target, ctx)? {
                    Trampoline::Value(value) => Ok(value),
                    Trampoline::Eval(..) => Ok(Value::nil()),
                }
            },
            move |context, args| runtime_load(&load_env_runtime, context, args),
        )),
    );

    // __vm-import uses the same dual ABI. The runtime continuation owns the
    // isolated module env until evaluation settles, then atomically caches and
    // copies its selected exports into the caller env.
    let import_state = ImportRuntimeState::new(Rc::downgrade(env));
    env.set(
        intern("__vm-import"),
        Value::native_fn(NativeFn::with_payload_ctx_runtime(
            "__vm-import",
            import_state,
            legacy_import,
            runtime_import,
        )),
    );

    // __vm-defmacro: register a macro in the environment
    let macro_env = Rc::downgrade(env);
    env.set(
        intern("__vm-defmacro"),
        Value::native_fn(NativeFn::simple("__vm-defmacro", move |args| {
            if args.len() != 4 {
                return Err(SemaError::arity("defmacro", "4", args.len()));
            }
            let name = match args[0].as_symbol_spur() {
                Some(s) => s,
                None => return Err(SemaError::type_error("symbol", args[0].type_name())),
            };
            let params = match args[1].as_list() {
                Some(items) => items
                    .iter()
                    .map(|v| match v.as_symbol_spur() {
                        Some(s) => Ok(s),
                        None => Err(SemaError::type_error("symbol", v.type_name())),
                    })
                    .collect::<Result<Vec<_>, _>>()?,
                None => return Err(SemaError::type_error("list", args[1].type_name())),
            };
            let rest_param = if let Some(s) = args[2].as_symbol_spur() {
                Some(s)
            } else if args[2].is_nil() {
                None
            } else {
                return Err(SemaError::type_error("symbol or nil", args[2].type_name()));
            };
            let body = vec![args[3].clone()];
            let macro_env = upgrade_delegate_env(&macro_env)?;
            macro_env.set(
                name,
                Value::macro_val(Macro {
                    params,
                    rest_param,
                    body,
                    name,
                    syntax_rules: None,
                }),
            );
            Ok(Value::nil())
        })),
    );

    // __vm-defmacro-form: register a complete `(defmacro ...)` form directly
    // (pure destructure). Used for defmacro that
    // reaches compilation (e.g. non-top-level) rather than expand-time
    // registration.
    let dmf_env = Rc::downgrade(env);
    env.set(
        intern("__vm-defmacro-form"),
        Value::native_fn(NativeFn::simple("__vm-defmacro-form", move |args| {
            if args.len() != 1 {
                return Err(SemaError::arity("defmacro-form", "1", args.len()));
            }
            let items = args[0]
                .as_list()
                .ok_or_else(|| SemaError::type_error("list", args[0].type_name()))?;
            let dmf_env = upgrade_delegate_env(&dmf_env)?;
            register_defmacro(items, &dmf_env)?;
            Ok(Value::nil())
        })),
    );

    // __vm-define-syntax: register a complete `(define-syntax ...)` form directly
    // (pure destructure). Used when a define-syntax reaches compilation (e.g.
    // non-top-level) rather than expand-time registration.
    let dsf_env = Rc::downgrade(env);
    env.set(
        intern("__vm-define-syntax"),
        Value::native_fn(NativeFn::simple("__vm-define-syntax", move |args| {
            if args.len() != 1 {
                return Err(SemaError::arity("define-syntax", "1", args.len()));
            }
            let items = args[0]
                .as_list()
                .ok_or_else(|| SemaError::type_error("list", args[0].type_name()))?;
            let dsf_env = upgrade_delegate_env(&dsf_env)?;
            register_define_syntax(items, &dsf_env)?;
            Ok(Value::nil())
        })),
    );

    // __vm-define-record-type: delegate to the evaluator
    let drt_env = Rc::downgrade(env);
    env.set(
        intern("__vm-define-record-type"),
        Value::native_fn(NativeFn::simple("__vm-define-record-type", move |args| {
            if args.len() != 5 {
                return Err(SemaError::arity("define-record-type", "5", args.len()));
            }
            // Build the `(define-record-type ...)` argument list (without the head
            // symbol) and register the type directly via the pure destructure.
            // eval_define_record_type only sets native
            // ctor/predicate/accessor fns in the env; it evaluates no user code.
            let mut ctor_form = vec![args[1].clone()];
            if let Some(fields) = args[3].as_list() {
                ctor_form.extend(fields.iter().cloned());
            }
            let mut dr_args = vec![args[0].clone(), Value::list(ctor_form), args[2].clone()];
            if let Some(specs) = args[4].as_list() {
                for spec in specs.iter() {
                    dr_args.push(spec.clone());
                }
            }
            let drt_env = upgrade_delegate_env(&drt_env)?;
            match special_forms::eval_define_record_type(&dr_args, &drt_env)? {
                Trampoline::Value(v) => Ok(v),
                Trampoline::Eval(..) => Ok(Value::nil()),
            }
        })),
    );

    // __vm-delay: create a thunk with unevaluated body
    env.set(
        intern("__vm-delay"),
        Value::native_fn(NativeFn::simple("__vm-delay", |args| {
            if args.len() != 1 {
                return Err(SemaError::arity("delay", "1", args.len()));
            }
            // args[0] is the unevaluated body expression (passed as a quoted constant)
            Ok(Value::thunk(Thunk {
                body: args[0].clone(),
                forced: RefCell::new(None),
            }))
        })),
    );

    // __vm-force: force a thunk
    let force_state = ForceRuntimeState::new(Rc::downgrade(env));
    env.set(
        intern("__vm-force"),
        Value::native_fn(
            NativeFn::with_payload_ctx_runtime(
                "__vm-force",
                force_state,
                force_legacy,
                force_runtime_call,
            )
            .with_escaping_args(&[0]),
        ),
    );

    // __vm-macroexpand: expand a macro form
    let me_env = Rc::downgrade(env);
    env.set(
        intern("__vm-macroexpand"),
        Value::native_fn(NativeFn::with_ctx("__vm-macroexpand", move |ctx, args| {
            if args.len() != 1 {
                return Err(SemaError::arity("macroexpand", "1", args.len()));
            }
            if let Some(items) = args[0].as_list() {
                if !items.is_empty() {
                    if let Some(spur) = items[0].as_symbol_spur() {
                        // Upgrade lazily: the non-macro passthrough below never
                        // touches the env.
                        let me_env = upgrade_delegate_env(&me_env)?;
                        if let Some(mac_val) = me_env.get(spur) {
                            if let Some(mac) = mac_val.as_macro_rc() {
                                if mac.syntax_rules.is_some() {
                                    return crate::syntax_rules::expand(&mac, &items[1..], &me_env);
                                }
                                // VM-native: expand the transformer on the VM.
                                return apply_macro_vm(ctx, &mac, &items[1..], &me_env);
                            }
                        }
                    }
                }
            }
            Ok(args[0].clone())
        })),
    );

    // __vm-prompt: build Prompt directly from pre-evaluated entries
    env.set(
        intern("__vm-prompt"),
        Value::native_fn(NativeFn::simple("__vm-prompt", |args| {
            use sema_core::{Message, Prompt, Role};
            if args.len() != 1 {
                return Err(SemaError::arity("__vm-prompt", "1", args.len()));
            }
            let entries = args[0]
                .as_list()
                .ok_or_else(|| SemaError::type_error("list", args[0].type_name()))?;
            let mut messages = Vec::new();
            for entry in entries {
                if let Some(msg) = entry.as_message_rc() {
                    messages.push((*msg).clone());
                } else if let Some(pair) = entry.as_list() {
                    if pair.len() == 2 {
                        let role_str = pair[0]
                            .as_str()
                            .ok_or_else(|| SemaError::eval("prompt: expected role string"))?;
                        let role = match role_str {
                            "system" => Role::System,
                            "user" => Role::User,
                            "assistant" => Role::Assistant,
                            "tool" => Role::Tool,
                            other => {
                                return Err(SemaError::eval(format!(
                                    "prompt: unknown role '{other}'"
                                )))
                            }
                        };
                        let parts = pair[1]
                            .as_list()
                            .ok_or_else(|| SemaError::type_error("list", pair[1].type_name()))?;
                        let mut content = String::new();
                        for part in parts {
                            if let Some(s) = part.as_str() {
                                content.push_str(s);
                            } else {
                                content.push_str(&part.to_string());
                            }
                        }
                        messages.push(Message {
                            role,
                            content,
                            images: Vec::new(),
                        });
                    } else {
                        return Err(SemaError::eval(
                            "prompt: expected (role parts) pair or message value",
                        ));
                    }
                } else {
                    return Err(SemaError::eval(
                        "prompt: expected (role parts) pair or message value",
                    ));
                }
            }
            Ok(Value::prompt(Prompt { messages }))
        })),
    );

    // __vm-message: build Message directly from pre-evaluated parts
    env.set(
        intern("__vm-message"),
        Value::native_fn(NativeFn::simple("__vm-message", |args| {
            use sema_core::{Message, Role};
            if args.len() != 2 {
                return Err(SemaError::arity("__vm-message", "2", args.len()));
            }
            let role = if let Some(spur) = args[0].as_keyword_spur() {
                let s = resolve(spur);
                match s.as_str() {
                    "system" => Role::System,
                    "user" => Role::User,
                    "assistant" => Role::Assistant,
                    "tool" => Role::Tool,
                    other => {
                        return Err(SemaError::eval(format!("message: unknown role '{other}'")))
                    }
                }
            } else {
                return Err(SemaError::type_error("keyword", args[0].type_name()));
            };
            let parts = args[1]
                .as_list()
                .ok_or_else(|| SemaError::type_error("list", args[1].type_name()))?;
            let mut content = String::new();
            for part in parts {
                if let Some(s) = part.as_str() {
                    content.push_str(s);
                } else {
                    content.push_str(&part.to_string());
                }
            }
            Ok(Value::message(Message {
                role,
                content,
                images: Vec::new(),
            }))
        })),
    );

    // __vm-deftool: the VM has already evaluated description/parameters/handler
    // and passes them as values, so build the tool directly.
    let tool_env = Rc::downgrade(env);
    env.set(
        intern("__vm-deftool"),
        Value::native_fn(NativeFn::simple("__vm-deftool", move |args| {
            if args.len() != 4 {
                return Err(SemaError::arity("deftool", "4", args.len()));
            }
            let name = args[0]
                .as_symbol()
                .ok_or_else(|| SemaError::eval("deftool: name must be a symbol"))?;
            let tool_env = upgrade_delegate_env(&tool_env)?;
            special_forms::register_tool(
                &name,
                args[1].clone(),
                args[2].clone(),
                args[3].clone(),
                &tool_env,
            )
        })),
    );

    // __vm-defagent: the VM has already evaluated the options map, so build the
    // agent directly.
    let agent_env = Rc::downgrade(env);
    env.set(
        intern("__vm-defagent"),
        Value::native_fn(NativeFn::simple("__vm-defagent", move |args| {
            if args.len() != 2 {
                return Err(SemaError::arity("defagent", "2", args.len()));
            }
            let name = args[0]
                .as_symbol()
                .ok_or_else(|| SemaError::eval("defagent: name must be a symbol"))?;
            let agent_env = upgrade_delegate_env(&agent_env)?;
            special_forms::register_agent(&name, args[1].clone(), &agent_env)
        })),
    );

    // __vm-destructure: strict destructure — errors on shape mismatch
    // (pattern value) -> map of bindings keyed by symbol
    env.set(
        intern("__vm-destructure"),
        Value::native_fn(NativeFn::simple("__vm-destructure", |args| {
            if args.len() != 2 {
                return Err(SemaError::arity("__vm-destructure", "2", args.len()));
            }
            let bindings = crate::destructure::destructure(&args[0], &args[1])?;
            let mut map = std::collections::BTreeMap::new();
            for (spur, val) in bindings {
                map.insert(Value::symbol_from_spur(spur), val);
            }
            Ok(Value::map(map))
        })),
    );

    // __vm-try-match: soft match — returns nil on no match, map of bindings on match
    // (pattern value) -> nil | map of bindings keyed by symbol
    env.set(
        intern("__vm-try-match"),
        Value::native_fn(NativeFn::simple("__vm-try-match", |args| {
            if args.len() != 2 {
                return Err(SemaError::arity("__vm-try-match", "2", args.len()));
            }
            match crate::destructure::try_match(&args[0], &args[1])? {
                Some(bindings) => {
                    let mut map = std::collections::BTreeMap::new();
                    for (spur, val) in bindings {
                        map.insert(Value::symbol_from_spur(spur), val);
                    }
                    Ok(Value::map(map))
                }
                None => Ok(Value::nil()),
            }
        })),
    );

    // __vm-match-failed: the strict `(match ...)` no-clause-matched path. Always
    // raises an :eval error carrying the unmatched value. `match*` never calls
    // this (it returns nil instead).
    env.set(
        intern("__vm-match-failed"),
        Value::native_fn(NativeFn::simple("__vm-match-failed", |args| {
            let val = args.first().cloned().unwrap_or_else(Value::nil);
            Err(
                SemaError::eval(format!("match: no clause matched value: {val}")).with_hint(
                    "add a catch-all `(_ ...)` clause, or use `match*` to return nil on no match",
                ),
            )
        })),
    );

    // __vm-make-multi: create a MultiMethod value
    env.set(
        intern("__vm-make-multi"),
        Value::native_fn(NativeFn::simple("__vm-make-multi", |args| {
            if args.len() != 2 {
                return Err(SemaError::arity("__vm-make-multi", "2", args.len()));
            }
            let name_spur = args[0]
                .as_symbol_spur()
                .ok_or_else(|| SemaError::eval("__vm-make-multi: expected symbol"))?;
            Ok(Value::multimethod(MultiMethod {
                name: name_spur,
                dispatch_fn: args[1].clone(),
                methods: RefCell::new(std::collections::BTreeMap::new()),
                default: RefCell::new(None),
            }))
        })),
    );

    // __vm-defmethod: add a method to an existing MultiMethod
    env.set(
        intern("__vm-defmethod"),
        Value::native_fn(
            NativeFn::simple("__vm-defmethod", |args| {
                if args.len() != 3 {
                    return Err(SemaError::arity("__vm-defmethod", "3", args.len()));
                }
                let mm = args[0].as_multimethod_rc().ok_or_else(|| {
                    SemaError::eval("defmethod: first argument is not a multimethod")
                })?;
                let dispatch_val = &args[1];
                let handler = &args[2];
                if let Some(kw) = dispatch_val.as_keyword_spur() {
                    if resolve(kw) == "default" {
                        *mm.default.borrow_mut() = Some(handler.clone());
                        return Ok(Value::nil());
                    }
                }
                mm.methods
                    .borrow_mut()
                    .insert(dispatch_val.clone(), handler.clone());
                Ok(Value::nil())
            })
            .with_escaping_args(&[1, 2]),
        ),
    );

    // gc/collect: run a full cycle collection now (CORE-2). User-facing —
    // registered here (not sema-stdlib) because pin computation needs the
    // native call environment. Pins skip descent into the live namespace of
    // the executing VM, with the interpreter env as the direct-host fallback;
    // correctness never depends on pins — live objects are protected by their
    // external strong counts.
    let gc_env = Rc::downgrade(env);
    let gc_env_runtime = Rc::downgrade(env);
    env.set(
        intern("gc/collect"),
        Value::native_fn(NativeFn::with_ctx_runtime(
            "gc/collect",
            move |context, args| {
                validate_gc_collect_args(args)?;
                Ok(gc_collect_with_pins(gc_delegate_pins(
                    context.legacy_call_env().as_ref(),
                    &gc_env,
                )))
            },
            move |context, args| {
                validate_gc_collect_args(args)?;
                Ok(sema_core::runtime::NativeOutcome::Return(
                    gc_collect_with_pins(gc_delegate_pins(
                        context.call_env.as_ref(),
                        &gc_env_runtime,
                    )),
                ))
            },
        )),
    );

    // gc/stats: report the last completed collection's stats plus the current
    // candidate-registry size, without collecting.
    env.set(
        intern("gc/stats"),
        Value::native_fn(NativeFn::simple("gc/stats", |args| {
            if !args.is_empty() {
                return Err(SemaError::arity("gc/stats", "0", args.len()));
            }
            let mut map = gc_stats_btree(&sema_core::gc_last_stats());
            map.insert(
                Value::keyword("registry-size"),
                Value::int(sema_core::gc_registry_len() as i64),
            );
            Ok(Value::map(map))
        })),
    );
}

/// `{:candidates N :traced N :collected N :pruned N}` for the gc builtins.
fn gc_stats_btree(stats: &sema_core::GcStats) -> BTreeMap<Value, Value> {
    let mut map = BTreeMap::new();
    map.insert(
        Value::keyword("candidates"),
        Value::int(stats.candidates as i64),
    );
    map.insert(Value::keyword("traced"), Value::int(stats.traced as i64));
    map.insert(
        Value::keyword("collected"),
        Value::int(stats.collected as i64),
    );
    map.insert(Value::keyword("pruned"), Value::int(stats.pruned as i64));
    map
}

fn gc_stats_map(stats: &sema_core::GcStats) -> Value {
    Value::map(gc_stats_btree(stats))
}

fn gc_delegate_pins(call_env: Option<&Rc<Env>>, fallback: &Weak<Env>) -> Vec<sema_core::NodePtr> {
    call_env
        .cloned()
        .or_else(|| fallback.upgrade())
        .map_or_else(Vec::new, |env| sema_core::gc_env_chain_pins(&env))
}

fn validate_gc_collect_args(args: &[Value]) -> Result<(), SemaError> {
    if !args.is_empty() {
        return Err(SemaError::arity("gc/collect", "0", args.len()));
    }
    Ok(())
}

fn gc_collect_with_pins(pins: Vec<sema_core::NodePtr>) -> Value {
    gc_stats_map(&sema_core::gc_collect(
        &pins,
        sema_core::GcTrigger::Explicit,
    ))
}

#[cfg(test)]
mod runtime_eval_tests {
    use super::*;

    use sema_core::runtime::{
        CancelReason, CancellationView, NativeCallContext, NativeOutcome, TaskContextHandle,
        TaskOutcome, TaskSettlement,
    };
    use sema_vm::runtime::{RootHandle, RootOptions, RootPoll};

    fn drive_selected_until_ready(interp: &Interpreter, root: &RootHandle) -> Rc<TaskSettlement> {
        for _ in 0..128 {
            match root.poll_result() {
                RootPoll::Ready(settlement) => return settlement,
                RootPoll::Pending => {
                    interp
                        .drive_roots(&[root.id()])
                        .expect("selected root drive succeeds");
                }
                RootPoll::Aborted(fault) => panic!("selected root aborted: {fault:?}"),
                RootPoll::RuntimeDropped => panic!("runtime dropped while driving selected root"),
                RootPoll::InvariantViolation => {
                    panic!("runtime invariant violation while driving selected root")
                }
            }
        }
        panic!("selected root did not settle within the bounded drive loop")
    }

    fn returned_value(settlement: &TaskSettlement) -> Value {
        match &settlement.outcome {
            TaskOutcome::Returned(value) => value.clone(),
            TaskOutcome::Failed(error) => panic!("root failed instead of returning: {error:?}"),
            TaskOutcome::Cancelled(reason) => {
                panic!("root was cancelled instead of returning: {reason:?}")
            }
        }
    }

    fn assert_runtime_import_not_published(interp: &Interpreter, identity: &str, export: &str) {
        assert!(
            interp
                .ctx
                .get_cached_module(&std::path::PathBuf::from(identity))
                .is_none(),
            "failed import must not populate the module cache",
        );
        assert!(
            interp.global_env.get(intern(export)).is_none(),
            "failed import must not copy exports",
        );
        assert_eq!(interp.ctx.current_file_path(), None);
        assert!(interp.ctx.current_file.borrow().is_empty());
        assert!(interp.ctx.module_exports.borrow().is_empty());
        assert!(interp.ctx.module_load_stack.borrow().is_empty());
    }

    fn invoke_runtime_delegate(
        interp: &Interpreter,
        name: &str,
        call_env: Rc<Env>,
        args: &[Value],
    ) -> NativeOutcome {
        let delegate = interp
            .global_env
            .get_str(name)
            .and_then(|value| value.as_native_fn_rc())
            .unwrap_or_else(|| panic!("missing runtime delegate {name}"));
        let task_context = TaskContextHandle::default();
        task_context
            .borrow_mut()
            .insert(Rc::new(sema_core::runtime::ModuleTaskState::default()));
        let mut context = NativeCallContext {
            eval_context: &interp.ctx,
            task_context,
            call_env: Some(call_env),
            cancellation: CancellationView::default(),
        };
        delegate
            .invoke_runtime(&mut context, args)
            .unwrap_or_else(|error| panic!("runtime delegate {name} failed: {error}"))
    }

    #[test]
    fn direct_runtime_eval_callable_uses_explicit_call_environment() {
        let interp = Interpreter::new();
        let call_env = Rc::new(Env::with_parent(Rc::clone(&interp.global_env)));
        call_env.set_str("call-env-only", Value::int(41));
        let expression = sema_reader::read_many("(+ call-env-only 1)")
            .expect("parse runtime eval expression")
            .remove(0);

        let outcome =
            invoke_runtime_delegate(&interp, "__vm-eval", Rc::clone(&call_env), &[expression]);
        let NativeOutcome::Call(call) = outcome else {
            panic!("runtime eval did not produce a structural call");
        };
        let (closure, _, _) = sema_vm::extract_vm_closure(&call.callable)
            .expect("runtime eval produces a VM closure");
        let home = closure
            .globals
            .as_ref()
            .expect("runtime eval closure has explicit globals");

        assert!(
            Rc::ptr_eq(home, &call_env),
            "runtime eval must compile against the exact caller environment"
        );
    }

    #[test]
    fn direct_runtime_load_callable_uses_explicit_call_environment() {
        let interp = Interpreter::new();
        interp.ctx.set_embedded_file(
            std::path::PathBuf::from("call-env-load.sema"),
            b"(define loaded-into-call-env 42)".to_vec(),
        );
        let call_env = Rc::new(Env::with_parent(Rc::clone(&interp.global_env)));

        let outcome = invoke_runtime_delegate(
            &interp,
            "__vm-load",
            Rc::clone(&call_env),
            &[Value::string("call-env-load.sema")],
        );
        let NativeOutcome::Call(call) = outcome else {
            panic!("runtime load did not produce a structural call");
        };
        let (closure, _, _) = sema_vm::extract_vm_closure(&call.callable)
            .expect("runtime load produces a VM closure");
        let home = closure
            .globals
            .as_ref()
            .expect("runtime load closure has explicit globals");

        assert!(
            Rc::ptr_eq(home, &call_env),
            "runtime load must execute definitions in the exact caller environment"
        );
    }

    #[test]
    fn cached_runtime_imports_with_colliding_root_ids_do_not_cross_environments() {
        fn prepare(value: i64) -> Interpreter {
            let interp = Interpreter::new();
            interp.ctx.set_embedded_file(
                std::path::PathBuf::from("colliding-call-env.sema"),
                format!(
                    "(module colliding-call-env (export imported-answer) (define imported-answer {value}))"
                )
                .into_bytes(),
            );
            interp
                .eval_str_via_runtime(r#"(import "colliding-call-env.sema" imported-answer)"#)
                .expect("seed interpreter-local module cache");
            assert_eq!(
                interp.global_env.take(intern("imported-answer")),
                Some(Value::int(value))
            );
            interp
        }

        let interp_a = prepare(11);
        let interp_b = prepare(22);
        let env_a = Rc::new(Env::with_parent(Rc::clone(&interp_a.global_env)));
        let env_b = Rc::new(Env::with_parent(Rc::clone(&interp_b.global_env)));
        let args = [
            Value::string("colliding-call-env.sema"),
            Value::list(vec![Value::symbol("imported-answer")]),
        ];

        assert!(matches!(
            invoke_runtime_delegate(&interp_a, "__vm-import", Rc::clone(&env_a), &args),
            NativeOutcome::Return(value) if value.is_nil()
        ));
        assert!(matches!(
            invoke_runtime_delegate(&interp_b, "__vm-import", Rc::clone(&env_b), &args),
            NativeOutcome::Return(value) if value.is_nil()
        ));

        assert_eq!(env_a.get_str("imported-answer"), Some(Value::int(11)));
        assert_eq!(env_b.get_str("imported-answer"), Some(Value::int(22)));
        assert!(interp_a.global_env.get_str("imported-answer").is_none());
        assert!(interp_b.global_env.get_str("imported-answer").is_none());
    }

    #[test]
    fn gc_delegate_pins_prefer_exact_call_environment_over_fallback() {
        let fallback = Rc::new(Env::new());
        let call_env = Rc::new(Env::with_parent(Rc::new(Env::new())));
        let fallback_weak = Rc::downgrade(&fallback);

        assert_eq!(
            gc_delegate_pins(Some(&call_env), &fallback_weak),
            sema_core::gc_env_chain_pins(&call_env)
        );
        assert_eq!(
            gc_delegate_pins(None, &fallback_weak),
            sema_core::gc_env_chain_pins(&fallback)
        );
    }

    #[test]
    fn preload_style_module_keeps_nested_load_and_import_in_isolated_environment() {
        let interp = Interpreter::new();
        interp.ctx.set_embedded_file(
            std::path::PathBuf::from("preload-nested-load.sema"),
            b"(define loaded-only 10)".to_vec(),
        );
        interp.ctx.set_embedded_file(
            std::path::PathBuf::from("preload-nested-import.sema"),
            b"(module preload-nested-import (export imported-only) (define imported-only 32))"
                .to_vec(),
        );
        let module_env = Rc::new(Env::with_parent(Rc::clone(&interp.global_env)));
        let (expressions, spans) = sema_reader::read_many_with_spans(
            r#"(load "preload-nested-load.sema")
               (import "preload-nested-import.sema" imported-only)
               (list loaded-only imported-only)"#,
        )
        .expect("parse preload-style module");

        let result = eval_module_body_vm(&interp.ctx, &module_env, &expressions, &spans, None)
            .expect("evaluate preload-style module");

        assert_eq!(result, Value::list(vec![Value::int(10), Value::int(32)]));
        assert_eq!(module_env.get_str("loaded-only"), Some(Value::int(10)));
        assert_eq!(module_env.get_str("imported-only"), Some(Value::int(32)));
        assert!(interp.global_env.get_str("loaded-only").is_none());
        assert!(interp.global_env.get_str("imported-only").is_none());
        assert!(interp.ctx.legacy_call_env().is_none());
    }

    // Full-flip blocker (native-stack): routing eval through the runtime
    // (`drive` → `visit_ready` → `run_quantum`) adds native frames below every
    // VM native call, so freeing a pathologically deep, cycle-free collection
    // used to overflow the OS stack and SIGABRT — an uncatchable abort the VM's
    // frame guard can't turn into a Sema error. `str`/`display` of the same
    // structure is already stack-guarded (`stack::maybe_grow`); the residual
    // overflow was the *iterative* teardown of the nested list, now flattened
    // in `Value`'s drop (`sema-core/src/value.rs`, `drop_last_heap_ref`). The
    // contract is "no abort on either eval path", matching legacy `str`, which
    // returns a value at this depth (integration_test `deep_structure_str_no_abort`).
    #[test]
    fn eval_via_runtime_deep_structure_str_no_abort() {
        // A ~5000-deep nested list, stringified then measured. Builds, displays,
        // and (critically) frees the deep structure entirely on the runtime
        // drive path. Parity with the normal evaluator is the oracle.
        assert_runtime_matches_oracle(
            "(string-length (str (foldl (fn (acc _) (list acc)) (list 1) (range 5000))))",
        );
        // Isolate the teardown path: build and then discard the deep list
        // without ever displaying it, so only the recursive free is exercised.
        assert_runtime_matches_oracle(
            "(begin (foldl (fn (acc _) (list acc)) (list 1) (range 5000)) 0)",
        );
    }

    #[test]
    fn eval_via_runtime_evaluates_a_synchronous_expression() {
        // Acceptance gate: a real interpreter routes a synchronous eval through
        // the unified runtime and returns the correct value.
        let interp = Interpreter::new();
        let (exprs, _spans) = sema_reader::read_many_with_spans("(+ 1 2)").expect("parse");
        let result = interp.eval_via_runtime(&exprs[0]).expect("eval");
        assert_eq!(result, Value::int(3));
    }

    // A legacy user closure (`double`, defined via `eval_str`) called from a
    // runtime quantum re-enters through the `call_value` callback onto a fresh
    // foreign VM. That synchronous nested run is carried by the TEMPORARY
    // `suspend_runtime_quantum` bridge until the Task 04 `NativeOutcome::Call`
    // migration makes legacy callback re-entry scheduler-native.
    #[test]
    fn eval_via_runtime_shares_interpreter_globals() {
        let interp = Interpreter::new();
        interp
            .eval_str("(define (double x) (* x 2))")
            .expect("define");
        let (exprs, _spans) = sema_reader::read_many_with_spans("(double 21)").expect("parse");
        let result = interp.eval_via_runtime(&exprs[0]).expect("eval");
        assert_eq!(result, Value::int(42));
    }

    /// Assert the runtime path returns exactly what the normal `eval_str`
    /// evaluator produces for the same program on a fresh interpreter. The
    /// `eval_str` result is the correctness oracle.
    fn assert_runtime_matches_oracle(program: &str) {
        let oracle_interp = Interpreter::new();
        let expected = oracle_interp
            .eval_str(program)
            .unwrap_or_else(|e| panic!("oracle eval_str failed for {program:?}: {e:?}"));
        let interp = Interpreter::new();
        let got = interp
            .eval_str_via_runtime(program)
            .unwrap_or_else(|e| panic!("eval_str_via_runtime failed for {program:?}: {e:?}"));
        assert_eq!(got, expected, "runtime != oracle for {program:?}");
    }

    #[test]
    fn runtime_force_drives_runtime_only_delayed_body() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime(
                "(let ((ch (channel/new 1))) \
                   (async/spawn (fn () (async/sleep 1) (channel/send ch 42))) \
                   (force (delay (channel/recv ch))))",
            )
            .expect("force drives the delayed body on the active runtime task");

        assert_eq!(result, Value::int(42));
    }

    #[test]
    fn runtime_force_memoizes_success_exactly_once_after_suspension() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime(
                "(begin \
                   (define force-count 0) \
                   (define p \
                     (delay (begin \
                              (async/sleep 1) \
                              (set! force-count (+ force-count 1)) \
                              force-count))) \
                   (list (force p) (force p) force-count (promise-forced? p)))",
            )
            .expect("a successfully forced promise is memoized");

        assert_eq!(result, lit("(list 1 1 1 #t)"));
    }

    #[test]
    fn concurrent_runtime_force_evaluates_the_delayed_body_once() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime(
                "(begin \
                   (define force-count 0) \
                   (define p \
                     (delay (begin \
                              (async/sleep 10) \
                              (set! force-count (+ force-count 1)) \
                              force-count))) \
                   (list \
                     (async/await \
                       (async/all (list (async (force p)) (async (force p))))) \
                     force-count))",
            )
            .expect("concurrent force calls share one delayed evaluation");

        assert_eq!(result, lit("(list (list 1 1) 1)"));
    }

    #[test]
    fn overlapping_force_from_a_shared_foreign_runtime_is_rejected() {
        let (env, ctx) = Interpreter::new_parts();
        let first = Interpreter::from_parts(Rc::clone(&env), Rc::clone(&ctx));
        let second = Interpreter::from_parts(env, ctx);
        first
            .eval_str_via_runtime(
                "(begin
                   (define force-count 0)
                   (define p
                     (delay
                       (begin
                         (set! force-count (+ force-count 1))
                         (async/sleep 20)
                         force-count))))",
            )
            .expect("prepare a shared delayed promise");
        let root = first
            .submit_str("(force p)", RootOptions::default())
            .expect("submit force on the first runtime");
        first
            .drive_roots(&[root.id()])
            .expect("drive the first force to its timer");
        assert!(matches!(root.poll_result(), RootPoll::Pending));

        let error = second
            .eval_str("(force p)")
            .expect_err("a foreign runtime must not close or bypass the live force gate");
        assert!(
            error
                .to_string()
                .contains("already active in another runtime"),
            "unexpected error: {error}"
        );

        std::thread::sleep(std::time::Duration::from_millis(25));
        let settlement = drive_selected_until_ready(&first, &root);
        assert_eq!(returned_value(&settlement), Value::int(1));
        assert_eq!(
            first
                .eval_str_via_runtime("force-count")
                .expect("inspect exactly-once side effect"),
            Value::int(1)
        );
    }

    #[test]
    fn runtime_force_failure_is_not_memoized() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime(
                "(begin \
                   (define force-count 0) \
                   (define p \
                     (delay (begin \
                              (async/sleep 1) \
                              (set! force-count (+ force-count 1)) \
                              (error \"expected force failure\")))) \
                   (define first (try (force p) (catch e :failed))) \
                   (define second (try (force p) (catch e :failed))) \
                   (list first second force-count (promise-forced? p)))",
            )
            .expect("failed forcing remains catchable and retryable");

        assert_eq!(result, lit("(list :failed :failed 2 #f)"));
    }

    #[test]
    fn runtime_force_preserves_memoized_value_identity() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime(
                "(begin \
                   (define p (delay (begin (async/sleep 1) (mutable-array/new)))) \
                   (define first (force p)) \
                   (mutable-array/push! first 99) \
                   (mutable-array/get (force p) 0))",
            )
            .expect("force returns the exact memoized heap value");

        assert_eq!(result, Value::int(99));
    }

    #[test]
    fn runtime_force_cancellation_does_not_memoize() {
        let interp = Interpreter::new();
        interp
            .eval_str_via_runtime(
                "(begin \
                   (define force-gate (channel/new 1)) \
                   (define force-promise (delay (channel/recv force-gate))))",
            )
            .expect("prepare a promise whose body parks");
        let root = interp
            .submit_str("(force force-promise)", RootOptions::default())
            .expect("submit force root");

        interp
            .drive_roots(&[root.id()])
            .expect("drive force body to its channel wait");
        assert!(matches!(root.poll_result(), RootPoll::Pending));
        assert!(root.cancel(CancelReason::Explicit));
        let settlement = drive_selected_until_ready(&interp, &root);
        assert!(matches!(
            settlement.outcome,
            TaskOutcome::Cancelled(CancelReason::Explicit)
        ));
        assert_eq!(
            interp
                .eval_str_via_runtime("(promise-forced? force-promise)")
                .expect("inspect cancelled force"),
            Value::bool(false),
        );
    }

    #[test]
    fn cancelled_force_owner_releases_the_next_waiter_to_retry() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime(
                "(begin \
                   (define entered (channel/new 2)) \
                   (define body-gate (channel/new 1)) \
                   (define force-count 0) \
                   (define p \
                     (delay (begin \
                              (channel/send entered :entered) \
                              (channel/recv body-gate) \
                              (set! force-count (+ force-count 1)) \
                              force-count))) \
                   (define first (async (force p))) \
                   (channel/recv entered) \
                   (define second (async (force p))) \
                   (async/sleep 1) \
                   (async/cancel first) \
                   (channel/send body-gate :continue) \
                   (list \
                     (try (await first) (catch error :cancelled)) \
                     (await second) \
                     force-count \
                     (promise-forced? p)))",
            )
            .expect("a cancelled force owner hands the gate to its waiter");

        assert_eq!(result, lit("(list :cancelled 1 1 #t)"));
    }

    #[test]
    fn cancelling_nested_force_wait_releases_only_the_waited_gate() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime(
                "(begin
                   (define entered (channel/new 1))
                   (define hold (channel/new 1))
                   (define q
                     (delay
                       (begin
                         (channel/send entered :q)
                         (channel/recv hold)
                         7)))
                   (define q-owner (async (force q)))
                   (channel/recv entered)
                   (define p (delay (force q)))
                   (define p-owner (async (force p)))
                   (async/sleep 5)
                   (async/cancel p-owner)
                   (channel/send hold :go)
                   (list
                     (try (await p-owner) (catch error :cancelled))
                     (await q-owner)))",
            )
            .expect("nested force cancellation preserves the independently owned gate");

        assert_eq!(result, lit("(list :cancelled 7)"));
    }

    #[test]
    fn recursive_force_fails_instead_of_waiting_on_its_own_gate() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime(
                "(begin \
                   (define p (delay (force p))) \
                   (list \
                     (try (force p) (catch error :recursive-force)) \
                     (promise-forced? p)))",
            )
            .expect("recursive force remains catchable rather than deadlocking");

        assert_eq!(result, lit("(list :recursive-force #f)"));
    }

    #[test]
    fn recursive_force_from_the_synchronous_bridge_fails_without_recursing() {
        let interp = Interpreter::new();
        interp
            .eval_str_via_runtime(
                "(define legacy-recursive-p
                   (delay (force legacy-recursive-p)))",
            )
            .expect("define a recursively forced promise");
        let (exprs, _) = sema_reader::read_many_with_spans("(force legacy-recursive-p)")
            .expect("parse legacy force expression");

        let error = eval_value_vm(&interp.ctx, &exprs[0], &interp.global_env)
            .expect_err("the synchronous bridge detects recursive force");
        assert!(
            error
                .to_string()
                .contains("already active in the synchronous evaluator"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn deadlocked_force_owner_releases_the_gate_for_a_later_root() {
        let interp = Interpreter::new();
        interp
            .eval_str_via_runtime(
                "(begin \
                   (define force-deadlock-gate (channel/new 1)) \
                   (define force-deadlock-promise \
                     (delay (channel/recv force-deadlock-gate))))",
            )
            .expect("prepare a promise whose first force deadlocks");

        let first = interp
            .eval_str_via_runtime("(force force-deadlock-promise)")
            .expect_err("the first root has no channel producer");
        assert!(first.to_string().contains("channel is empty"));

        let retried = interp
            .eval_str_via_runtime(
                "(begin \
                   (async (channel/send force-deadlock-gate 9)) \
                   (force force-deadlock-promise))",
            )
            .expect("a later root can acquire and retry the deadlocked force");
        assert_eq!(retried, Value::int(9));
    }

    #[test]
    fn force_continuations_trace_their_retained_thunk_root() {
        let thunk = Value::thunk(Thunk {
            body: Value::int(7),
            forced: RefCell::new(None),
        });
        let expected = sema_core::NodePtr::of_value(&thunk).expect("thunk has a GC node");
        let root = ForceThunkRoot(thunk.clone());
        let mut traced = Vec::new();

        assert!(sema_core::runtime::Trace::trace(&root, &mut |edge| {
            if let sema_core::cycle::GcEdge::Value(value) = edge {
                traced.push(sema_core::NodePtr::of_value(value));
            }
        },));
        assert_eq!(traced, vec![Some(expected)]);
    }

    #[test]
    fn active_force_lease_prevents_weak_pruning_and_aba_cleanup() {
        let interp = Interpreter::new();
        let root = interp
            .submit_str("nil", RootOptions::default())
            .expect("mint a runtime-scoped root identity");
        let runtime = root.id().runtime();
        let previous_root = sema_core::set_current_root(Some(root.id()));
        let mut gate_ids = sema_core::runtime::RuntimeScopedIdCounter::<
            sema_core::runtime::ResourceGateId,
        >::new(runtime);
        let closer = Rc::new(|_| Ok(true));
        let first_handle = sema_core::runtime::ResourceGateHandle::new(
            gate_ids.allocate().expect("first gate id"),
            closer.clone(),
        );
        let replacement_handle = sema_core::runtime::ResourceGateHandle::new(
            gate_ids.allocate().expect("replacement gate id"),
            closer,
        );
        let state = ForceRuntimeState::new(Rc::downgrade(&interp.global_env));
        let thunk = Rc::new(Thunk {
            body: Value::int(1),
            forced: RefCell::new(None),
        });
        let key = sema_core::NodePtr::of_rc(&thunk);
        let (_, lease) = state
            .install_gate(&thunk, first_handle)
            .expect("install active force gate");

        drop(thunk);
        let unrelated = Rc::new(Thunk {
            body: Value::int(2),
            forced: RefCell::new(None),
        });
        assert!(state
            .active_gate(&unrelated)
            .expect("unrelated lookup")
            .is_none());
        assert_eq!(
            state.gates.borrow().len(),
            1,
            "an active lease keeps its gate entry after the thunk root drops"
        );

        // A stale lease also carries the exact gate generation. If a map entry
        // were replaced at the same node key, dropping the old lease must not
        // decrement or remove the replacement.
        state.gates.borrow_mut().insert(
            key,
            ForceGateEntry {
                thunk: Rc::downgrade(&unrelated),
                gate: replacement_handle,
                active_calls: 1,
            },
        );
        drop(lease);
        assert_eq!(
            state
                .gates
                .borrow()
                .get(&key)
                .map(|entry| entry.active_calls),
            Some(1)
        );
        state.gates.borrow_mut().remove(&key);
        sema_core::set_current_root(previous_root);
    }

    // Higher-order stdlib functions re-enter the evaluator through the
    // `call_value` callback for each element; each gate asserts parity with the
    // normal evaluator.
    #[test]
    fn eval_via_runtime_map() {
        assert_runtime_matches_oracle("(map (fn (x) (* x 2)) (list 1 2 3))");
    }

    #[test]
    fn eval_via_runtime_filter() {
        assert_runtime_matches_oracle("(filter odd? (list 1 2 3 4))");
    }

    #[test]
    fn eval_via_runtime_foldl() {
        assert_runtime_matches_oracle("(foldl + 0 (list 1 2 3 4))");
    }

    // A recursive user `define` created via the normal evaluator is callable
    // through the runtime, and the recursion (a self tail/non-tail call) runs
    // to completion.
    #[test]
    fn eval_via_runtime_recursion() {
        let interp = Interpreter::new();
        interp
            .eval_str("(define (fact n) (if (< n 2) 1 (* n (fact (- n 1)))))")
            .expect("define");
        let result = interp.eval_str_via_runtime("(fact 5)").expect("eval");
        assert_eq!(result, Value::int(120));
    }

    // A raising program returns `Err(..)` — not a panic and not a wrong `Ok`.
    #[test]
    fn eval_via_runtime_propagates_error_from_error_call() {
        let interp = Interpreter::new();
        let result = interp.eval_str_via_runtime("(error \"boom\")");
        assert!(result.is_err(), "expected Err, got {result:?}");
        let msg = format!("{}", result.unwrap_err());
        assert!(msg.contains("boom"), "error message missing 'boom': {msg}");
    }

    #[test]
    fn eval_via_runtime_propagates_division_by_zero() {
        let interp = Interpreter::new();
        let result = interp.eval_str_via_runtime("(/ 1 0)");
        assert!(result.is_err(), "expected Err, got {result:?}");
    }

    // Multiple top-level forms in one program evaluate as a single root; the
    // last form's value is returned and an intervening `define` is visible.
    #[test]
    fn eval_via_runtime_multiple_top_level_forms() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime("(define x 10) (+ x 5)")
            .expect("eval");
        assert_eq!(result, Value::int(15));
    }

    // A `define` issued through the runtime persists on the interpreter and is
    // visible to a later runtime eval on the same interpreter.
    #[test]
    fn eval_via_runtime_defines_persist_across_calls() {
        let interp = Interpreter::new();
        interp
            .eval_str_via_runtime("(define counter 41)")
            .expect("define");
        let result = interp.eval_str_via_runtime("(+ counter 1)").expect("eval");
        assert_eq!(result, Value::int(42));
    }

    // ACCEPTANCE GATE (Task 03 Step 2 shared context): a multimethod dispatch
    // re-enters the evaluator through the `call_value` callback from inside a
    // runtime quantum. With the runtime sharing the interpreter's context (whose
    // callbacks are registered) this resolves; a fresh context would error with
    // "call callback not registered". The result must equal the `eval_str`
    // oracle (12).
    #[test]
    fn eval_via_runtime_multimethod_dispatch_matches_oracle() {
        assert_runtime_matches_oracle(
            "(defmulti area (fn (s) (:kind s))) \
             (defmethod area :circle (fn (s) (* 3 (:r s) (:r s)))) \
             (area {:kind :circle :r 2})",
        );
    }

    // A user closure dispatched dynamically via `apply` re-enters through the
    // owned-call callback from a runtime quantum.
    #[test]
    fn eval_via_runtime_apply_user_closure_matches_oracle() {
        assert_runtime_matches_oracle("(apply (fn (a b c) (+ a b c)) (list 10 20 12))");
    }

    // ACCEPTANCE GATE (Task 03 Step 2 shared context): a multimethod defined in
    // one runtime eval is dispatchable in a *second* runtime eval on the same
    // interpreter — the shared context's module/global state persists across
    // runtime evals, not just the global env.
    #[test]
    fn eval_via_runtime_multimethod_persists_across_calls() {
        let interp = Interpreter::new();
        interp
            .eval_str_via_runtime(
                "(defmulti area (fn (s) (:kind s))) \
                 (defmethod area :circle (fn (s) (* 3 (:r s) (:r s)))) \
                 (defmethod area :square (fn (s) (* (:side s) (:side s))))",
            )
            .expect("define multimethod");
        let circle = interp
            .eval_str_via_runtime("(area {:kind :circle :r 2})")
            .expect("dispatch circle");
        assert_eq!(circle, Value::int(12));
        let square = interp
            .eval_str_via_runtime("(area {:kind :square :side 5})")
            .expect("dispatch square");
        assert_eq!(square, Value::int(25));
    }

    // A dynamic parameter (`make-parameter`) created in one runtime eval is read
    // and `parameterize`d in a *second* runtime eval on the same interpreter —
    // dynamic context reads/writes go through the shared context that persists
    // across runtime evals.
    #[test]
    fn eval_via_runtime_parameterize_reads_context_across_calls() {
        let interp = Interpreter::new();
        interp
            .eval_str_via_runtime("(define p (make-parameter 1))")
            .expect("define parameter");
        let base = interp.eval_str_via_runtime("(p)").expect("read parameter");
        assert_eq!(base, Value::int(1));
        let dyn_bound = interp
            .eval_str_via_runtime("(parameterize ((p 2)) (+ (p) 100))")
            .expect("parameterize");
        assert_eq!(dyn_bound, Value::int(102));
    }

    // `async/sleep` returns a structural timer suspension. The runtime parks its
    // continuation and resumes the same VM frame with nil when the timer fires.
    #[test]
    fn eval_via_runtime_async_sleep_settles_after_timer_fires() {
        let interp = Interpreter::new();
        // A tiny real duration: the drive loop waits out the deadline on the
        // runtime's MonotonicClock, then the timer fires and the VM resumes.
        let result = interp
            .eval_str_via_runtime("(async/sleep 2)")
            .expect("async/sleep settles through the runtime");
        assert_eq!(result, Value::nil());
    }

    // The VM resumes AFTER the sleep and continues the rest of the program,
    // proving the frame is genuinely parked and resumed (not settled early).
    #[test]
    fn eval_via_runtime_async_sleep_resumes_and_continues() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime("(begin (async/sleep 2) (+ 40 2))")
            .expect("program continues past async/sleep");
        assert_eq!(result, Value::int(42));
    }

    // `async/spawn` + `async/await` round-trip through the runtime: spawn a
    // detached task, await its promise, and get the value. The runtime creates
    // the task from the thunk (a VM closure), settles its promise on
    // completion, and resumes the awaiting frame in place with the value.
    #[test]
    fn eval_via_runtime_await_spawn_returns_value() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime("(await (async/spawn (fn () (+ 40 2))))")
            .expect("await of a spawned task resolves through the runtime");
        assert_eq!(result, Value::int(42));
    }

    // Two spawned tasks run concurrently on the one runtime; awaiting both (as
    // separate awaits) yields both results — proving detached tasks are
    // scheduled fairly and each settles its own promise.
    #[test]
    fn eval_via_runtime_await_two_spawned_tasks() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime(
                "(begin \
                   (define a (async/spawn (fn () (+ 1 2)))) \
                   (define b (async/spawn (fn () (* 4 5)))) \
                   (+ (await a) (await b)))",
            )
            .expect("both spawned tasks resolve through the runtime");
        assert_eq!(result, Value::int(23));
    }

    // A spawned task that itself parks on a timer (`async/sleep`) and resumes:
    // the detached task suspends on the runtime timer, `fire_timer` wakes it,
    // it finishes, its promise settles, and the awaiting root resumes.
    #[test]
    fn eval_via_runtime_await_spawn_that_sleeps() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime("(await (async/spawn (fn () (async/sleep 2) 7)))")
            .expect("a spawned task that sleeps resolves through the runtime");
        assert_eq!(result, Value::int(7));
    }

    // ── PERSISTENT INTERPRETER-OWNED RUNTIME (Task 03 Step 2) ─────────

    // GATE (cross-eval detached survival): a task spawned and PERSISTED (via a
    // global `define`) in ONE `eval_str_via_runtime` call must still exist and
    // be drivable in a SECOND, SEPARATE call that awaits it. With a per-call
    // runtime the spawned task's registry/timer/promise state was rebuilt every
    // call, so the promise `p` referenced in the second call pointed at nothing
    // and `await` could not resolve. With a single interpreter-owned runtime the
    // detached task (parked on its `async/sleep` timer at the end of call one)
    // survives and its timer fires while the second root drives, resolving `p`.
    #[test]
    fn runtime_detached_spawn_survives_across_evals() {
        let interp = Interpreter::new();
        // Call 1: spawn a task that sleeps then yields 42, persist its promise.
        // The root of call 1 settles immediately (it only `define`s p); the
        // spawned task is detached and still parked on its timer afterward.
        interp
            .eval_str_via_runtime("(define p (async/spawn (fn () (async/sleep 2) 42)))")
            .expect("call 1 defines the persisted spawn promise");
        // Call 2: a fresh root on the SAME runtime awaits the promise from call
        // 1. Only survives if the detached task lived on between evals.
        let result = interp
            .eval_str_via_runtime("(await p)")
            .expect("the detached task from call 1 is still drivable in call 2");
        assert_eq!(result, Value::int(42));
    }

    #[test]
    fn pending_roots_resolve_relative_embedded_loads_from_their_submission_paths() {
        let interp = Interpreter::new();
        interp
            .ctx
            .set_embedded_file(std::path::PathBuf::from("left/dep.sema"), b":left".to_vec());
        interp.ctx.set_embedded_file(
            std::path::PathBuf::from("right/dep.sema"),
            b":right".to_vec(),
        );

        interp
            .ctx
            .push_file_path(std::path::PathBuf::from("left/entry.sema"));
        let left = interp
            .submit_str(r#"(load "./dep.sema")"#, RootOptions::default())
            .expect("submit left root");
        interp.ctx.pop_file_path();

        interp
            .ctx
            .push_file_path(std::path::PathBuf::from("right/entry.sema"));
        let right = interp
            .submit_str(r#"(load "./dep.sema")"#, RootOptions::default())
            .expect("submit right root");
        interp.ctx.pop_file_path();

        let left = drive_selected_until_ready(&interp, &left);
        let right = drive_selected_until_ready(&interp, &right);
        assert_eq!(returned_value(&left), Value::keyword("left"));
        assert_eq!(returned_value(&right), Value::keyword("right"));
        assert_eq!(interp.ctx.current_file_path(), None);
    }

    #[test]
    fn spawned_children_inherit_the_root_module_resolution_context() {
        let interp = Interpreter::new();
        interp.ctx.set_embedded_file(
            std::path::PathBuf::from("nested/dep.sema"),
            b":child".to_vec(),
        );

        interp
            .ctx
            .push_file_path(std::path::PathBuf::from("nested/entry.sema"));
        let root = interp
            .submit_str(
                r#"(await (async/spawn (fn () (load "./dep.sema"))))"#,
                RootOptions::default(),
            )
            .expect("submit spawning root");
        interp.ctx.pop_file_path();

        let settlement = drive_selected_until_ready(&interp, &root);
        assert_eq!(returned_value(&settlement), Value::keyword("child"));
        assert_eq!(interp.ctx.current_file_path(), None);
    }

    #[test]
    fn runtime_load_parks_cooperatively_and_a_sibling_can_release_it() {
        let interp = Interpreter::new();
        interp.ctx.set_embedded_file(
            std::path::PathBuf::from("blocking-load.sema"),
            b"(channel/recv runtime-load-gate) (define runtime-loaded 42) runtime-loaded".to_vec(),
        );
        interp
            .eval_str_via_runtime("(define runtime-load-gate (channel/new 1))")
            .expect("create the load gate");

        let load = interp
            .submit_str(r#"(load "blocking-load.sema")"#, RootOptions::default())
            .expect("submit a load that parks in its body");
        interp
            .drive_roots(&[load.id()])
            .expect("drive the loaded body to its channel wait");
        assert!(matches!(load.poll_result(), RootPoll::Pending));

        assert_eq!(
            interp
                .eval_str_via_runtime("(channel/send runtime-load-gate :continue)")
                .expect("a sibling root can run while load is parked"),
            Value::nil(),
        );
        let settlement = drive_selected_until_ready(&interp, &load);
        assert_eq!(returned_value(&settlement), Value::int(42));
        assert_eq!(
            interp
                .eval_str_via_runtime("runtime-loaded")
                .expect("load definitions land in the caller environment"),
            Value::int(42),
        );
    }

    #[test]
    fn runtime_import_parks_then_caches_and_copies_exports_after_completion() {
        let interp = Interpreter::new();
        interp.ctx.set_embedded_file(
            std::path::PathBuf::from("blocking-import.sema"),
            b"(module blocking (export answer) (set! runtime-import-count (+ runtime-import-count 1)) (channel/recv runtime-import-gate) (define answer 42))".to_vec(),
        );
        interp
            .eval_str_via_runtime(
                "(define runtime-import-gate (channel/new 1)) (define runtime-import-count 0)",
            )
            .expect("prepare import synchronization");

        let import = interp
            .submit_str(
                r#"(begin (import "blocking-import.sema" answer) answer)"#,
                RootOptions::default(),
            )
            .expect("submit a suspending import");
        interp
            .drive_roots(&[import.id()])
            .expect("drive the imported module to its channel wait");
        assert!(matches!(import.poll_result(), RootPoll::Pending));
        assert!(
            interp
                .ctx
                .get_cached_module(&std::path::PathBuf::from("blocking-import.sema"))
                .is_none(),
            "a parked import must not expose a partial cache entry",
        );
        assert!(
            interp.global_env.get(intern("answer")).is_none(),
            "a parked import must not copy partial exports",
        );
        assert_eq!(
            interp
                .eval_str_via_runtime("(+ 1 2)")
                .expect("an unrelated sibling runs while import is parked"),
            Value::int(3),
        );
        interp
            .eval_str_via_runtime("(channel/send runtime-import-gate :continue)")
            .expect("release the imported module");

        let settlement = drive_selected_until_ready(&interp, &import);
        assert_eq!(returned_value(&settlement), Value::int(42));
        assert!(
            interp
                .ctx
                .get_cached_module(&std::path::PathBuf::from("blocking-import.sema"))
                .is_some(),
            "a successful import publishes its cache entry at completion",
        );
        assert_eq!(
            interp
                .eval_str_via_runtime(
                    r#"(begin (import "blocking-import.sema" answer) (list answer runtime-import-count))"#,
                )
                .expect("the second import uses the completed cache"),
            Value::list(vec![Value::int(42), Value::int(1)]),
        );
    }

    #[test]
    fn synchronous_import_rejects_an_active_runtime_import_without_reevaluating() {
        let interp = Interpreter::new();
        let identity = std::path::PathBuf::from("sync-runtime-overlap.sema");
        interp.ctx.set_embedded_file(
            identity.clone(),
            b"(module sync-runtime-overlap (export overlap-answer) (set! overlap-count (+ overlap-count 1)) (channel/recv overlap-gate) (define overlap-answer 42))".to_vec(),
        );
        interp
            .eval_str_via_runtime("(define overlap-count 0) (define overlap-gate (channel/new 1))")
            .expect("prepare overlap state");

        let owner = interp
            .submit_str(
                r#"(begin (import "sync-runtime-overlap.sema" overlap-answer) overlap-answer)"#,
                RootOptions::default(),
            )
            .expect("submit structural import owner");
        interp
            .drive_roots(&[owner.id()])
            .expect("park the structural import owner");
        assert!(matches!(owner.poll_result(), RootPoll::Pending));

        let (expressions, _) = sema_reader::read_many_with_spans(
            r#"(import "sync-runtime-overlap.sema" overlap-answer)"#,
        )
        .expect("parse synchronous import");
        let error = eval_value_vm(&interp.ctx, &expressions[0], &interp.global_env)
            .expect_err("the value ABI must reject overlap with a runtime owner");
        assert!(
            error
                .to_string()
                .contains("already active in the cooperative runtime"),
            "unexpected overlap error: {error}",
        );
        assert_eq!(
            interp
                .eval_str_via_runtime("overlap-count")
                .expect("inspect overlap side effects"),
            Value::int(1),
        );
        assert!(interp.ctx.get_cached_module(&identity).is_none());
        assert!(interp.global_env.get(intern("overlap-answer")).is_none());

        interp
            .eval_str_via_runtime("(channel/send overlap-gate :continue)")
            .expect("release structural import owner");
        assert_eq!(
            returned_value(&drive_selected_until_ready(&interp, &owner)),
            Value::int(42),
        );
        assert_eq!(
            interp
                .eval_str_via_runtime("overlap-count")
                .expect("the owner evaluated exactly once"),
            Value::int(1),
        );
    }

    #[test]
    fn runtime_import_from_a_shared_foreign_runtime_is_rejected() {
        let (env, context) = Interpreter::new_parts();
        let first = Interpreter::from_parts(Rc::clone(&env), Rc::clone(&context));
        let second = Interpreter::from_parts(env, context);
        first.ctx.set_embedded_file(
            std::path::PathBuf::from("foreign-runtime-import.sema"),
            b"(module foreign-runtime-import (export foreign-answer) (set! foreign-import-count (+ foreign-import-count 1)) (channel/recv foreign-import-gate) (define foreign-answer 42))".to_vec(),
        );
        first
            .eval_str_via_runtime(
                "(define foreign-import-count 0) (define foreign-import-gate (channel/new 1))",
            )
            .expect("prepare shared import state");

        let owner = first
            .submit_str(
                r#"(begin (import "foreign-runtime-import.sema" foreign-answer) foreign-answer)"#,
                RootOptions::default(),
            )
            .expect("submit import on the first runtime");
        first
            .drive_roots(&[owner.id()])
            .expect("park the first runtime import");

        let error = second
            .eval_str_via_runtime(r#"(import "foreign-runtime-import.sema" foreign-answer)"#)
            .expect_err("the foreign runtime must not bypass the live import gate");
        assert!(
            error
                .to_string()
                .contains("already loading in another runtime"),
            "unexpected foreign-runtime error: {error}",
        );
        assert_eq!(
            first
                .eval_str_via_runtime("foreign-import-count")
                .expect("foreign runtime caused no duplicate side effect"),
            Value::int(1),
        );

        first
            .eval_str_via_runtime("(channel/send foreign-import-gate :continue)")
            .expect("release first runtime import");
        assert_eq!(
            returned_value(&drive_selected_until_ready(&first, &owner)),
            Value::int(42),
        );
    }

    #[test]
    fn concurrent_import_gate_allocations_converge_on_one_identity() {
        let interp = Interpreter::new();
        let root = interp
            .submit_str("nil", RootOptions::default())
            .expect("mint a runtime identity");
        let previous_root = sema_core::set_current_root(Some(root.id()));
        let mut gate_ids = sema_core::runtime::RuntimeScopedIdCounter::<
            sema_core::runtime::ResourceGateId,
        >::new(root.id().runtime());
        let first_id = gate_ids.allocate().expect("allocate first candidate gate");
        let second_id = gate_ids.allocate().expect("allocate second candidate gate");
        let closed = Rc::new(RefCell::new(Vec::new()));
        let closer: Rc<
            dyn Fn(
                sema_core::runtime::ResourceGateId,
            ) -> Result<bool, sema_core::runtime::ResourceGateCloseError>,
        > = {
            let closed = Rc::clone(&closed);
            Rc::new(move |gate| {
                closed.borrow_mut().push(gate);
                Ok(true)
            })
        };
        let state = ImportRuntimeState::new(Rc::downgrade(&interp.global_env));
        let identity = std::path::PathBuf::from("allocation-race.sema");

        let (first, first_lease) = state
            .install_gate(
                &identity,
                sema_core::runtime::ResourceGateHandle::new(first_id, Rc::clone(&closer)),
            )
            .expect("install the first pending allocation");
        let (second, second_lease) = state
            .install_gate(
                &identity,
                sema_core::runtime::ResourceGateHandle::new(second_id, closer),
            )
            .expect("the second pending allocation converges");

        assert_eq!(first.id(), first_id);
        assert_eq!(second.id(), first_id);
        assert_eq!(
            state
                .gates
                .borrow()
                .get(&identity)
                .map(|entry| entry.active_calls),
            Some(2),
        );
        assert_eq!(closed.borrow().as_slice(), &[second_id]);

        drop(first_lease);
        assert!(state.gates.borrow().contains_key(&identity));
        drop(second_lease);
        assert!(!state.gates.borrow().contains_key(&identity));
        assert!(closed.borrow().contains(&first_id));
        sema_core::set_current_root(previous_root);
    }

    #[test]
    fn runtime_import_gate_count_returns_to_baseline_after_terminal_outcomes() {
        let success = Interpreter::new();
        success.ctx.set_embedded_file(
            std::path::PathBuf::from("gate-success.sema"),
            b"(module gate-success (export gate-success-answer) (define gate-success-answer 42))"
                .to_vec(),
        );
        let success_baseline = success.runtime_resource_gate_count();
        success
            .eval_str_via_runtime(r#"(import "gate-success.sema" gate-success-answer)"#)
            .expect("successful import settles");
        assert_eq!(success.runtime_resource_gate_count(), success_baseline);

        let cancelled = Interpreter::new();
        cancelled.ctx.set_embedded_file(
            std::path::PathBuf::from("gate-cancel.sema"),
            b"(channel/recv gate-cancel-channel)".to_vec(),
        );
        cancelled
            .eval_str_via_runtime("(define gate-cancel-channel (channel/new 1))")
            .expect("create cancellation channel");
        let cancel_baseline = cancelled.runtime_resource_gate_count();
        let root = cancelled
            .submit_str(r#"(import "gate-cancel.sema")"#, RootOptions::default())
            .expect("submit cancellable import");
        cancelled
            .drive_roots(&[root.id()])
            .expect("park cancellable import");
        assert!(root.cancel(CancelReason::Explicit));
        assert!(matches!(
            drive_selected_until_ready(&cancelled, &root).outcome,
            TaskOutcome::Cancelled(CancelReason::Explicit),
        ));
        assert_eq!(cancelled.runtime_resource_gate_count(), cancel_baseline);

        let failed = Interpreter::new();
        failed.ctx.set_embedded_file(
            std::path::PathBuf::from("gate-failure.sema"),
            b"(error \"expected import failure\")".to_vec(),
        );
        let failure_baseline = failed.runtime_resource_gate_count();
        failed
            .eval_str_via_runtime(r#"(import "gate-failure.sema")"#)
            .expect_err("failing import settles");
        assert_eq!(failed.runtime_resource_gate_count(), failure_baseline);
    }

    #[test]
    fn concurrent_runtime_source_imports_evaluate_once() {
        let interp = Interpreter::new();
        interp.ctx.set_embedded_file(
            std::path::PathBuf::from("single-flight-source.sema"),
            b"(module single-flight-source (export source-answer) (set! source-import-count (+ source-import-count 1)) (channel/recv source-import-gate) (define source-answer 42))".to_vec(),
        );
        interp
            .eval_str_via_runtime(
                "(define source-import-count 0) (define source-import-gate (channel/new 1))",
            )
            .expect("prepare source single-flight state");

        let first = interp
            .submit_str(
                r#"(begin (import "single-flight-source.sema" source-answer) source-answer)"#,
                RootOptions::default(),
            )
            .expect("submit first source import");
        let second = interp
            .submit_str(
                r#"(begin (import "single-flight-source.sema" source-answer) source-answer)"#,
                RootOptions::default(),
            )
            .expect("submit second source import");
        interp
            .drive_roots(&[first.id()])
            .expect("park the first source import");
        interp
            .drive_roots(&[second.id()])
            .expect("park the second source import behind single-flight");
        assert!(matches!(first.poll_result(), RootPoll::Pending));
        assert!(matches!(second.poll_result(), RootPoll::Pending));
        assert_eq!(
            interp
                .eval_str_via_runtime("source-import-count")
                .expect("inspect source import side effects"),
            Value::int(1),
        );

        interp
            .eval_str_via_runtime("(channel/send source-import-gate :continue)")
            .expect("release the single source evaluator");
        assert_eq!(
            returned_value(&drive_selected_until_ready(&interp, &first)),
            Value::int(42),
        );
        assert_eq!(
            returned_value(&drive_selected_until_ready(&interp, &second)),
            Value::int(42),
        );
        assert_eq!(
            interp
                .eval_str_via_runtime("source-import-count")
                .expect("source import remains exact-once"),
            Value::int(1),
        );
    }

    #[test]
    fn concurrent_runtime_bytecode_imports_evaluate_once() {
        let interp = Interpreter::new();
        interp
            .eval_str_via_runtime(
                "(define bytecode-import-count 0) (define bytecode-import-gate (channel/new 1))",
            )
            .expect("prepare bytecode single-flight state");
        let compiled = interp
            .compile_to_bytecode(
                "(module single-flight-bytecode (export bytecode-answer) (set! bytecode-import-count (+ bytecode-import-count 1)) (channel/recv bytecode-import-gate) (define bytecode-answer 42))",
            )
            .expect("compile bytecode single-flight module");
        let bytes = sema_vm::serialize_to_bytes(&compiled, 0).expect("serialize bytecode module");
        interp.ctx.set_embedded_file(
            std::path::PathBuf::from("single-flight-bytecode.semac"),
            bytes,
        );

        let first = interp
            .submit_str(
                r#"(begin (import "single-flight-bytecode.semac" bytecode-answer) bytecode-answer)"#,
                RootOptions::default(),
            )
            .expect("submit first bytecode import");
        let second = interp
            .submit_str(
                r#"(begin (import "single-flight-bytecode.semac" bytecode-answer) bytecode-answer)"#,
                RootOptions::default(),
            )
            .expect("submit second bytecode import");
        interp
            .drive_roots(&[first.id()])
            .expect("park the first bytecode import");
        interp
            .drive_roots(&[second.id()])
            .expect("park the second bytecode import behind single-flight");
        assert!(matches!(first.poll_result(), RootPoll::Pending));
        assert!(matches!(second.poll_result(), RootPoll::Pending));
        assert_eq!(
            interp
                .eval_str_via_runtime("bytecode-import-count")
                .expect("inspect bytecode import side effects"),
            Value::int(1),
        );

        interp
            .eval_str_via_runtime("(channel/send bytecode-import-gate :continue)")
            .expect("release the single bytecode evaluator");
        assert_eq!(
            returned_value(&drive_selected_until_ready(&interp, &first)),
            Value::int(42),
        );
        assert_eq!(
            returned_value(&drive_selected_until_ready(&interp, &second)),
            Value::int(42),
        );
        assert_eq!(
            interp
                .eval_str_via_runtime("bytecode-import-count")
                .expect("bytecode import remains exact-once"),
            Value::int(1),
        );
    }

    #[test]
    fn cancelled_runtime_import_owner_hands_off_to_one_waiter() {
        let interp = Interpreter::new();
        interp.ctx.set_embedded_file(
            std::path::PathBuf::from("cancelled-owner-import.sema"),
            b"(module cancelled-owner-import (export cancelled-owner-answer) (set! cancelled-owner-count (+ cancelled-owner-count 1)) (if (= cancelled-owner-count 1) (channel/recv cancelled-owner-gate) nil) (define cancelled-owner-answer 42))".to_vec(),
        );
        interp
            .eval_str_via_runtime(
                "(define cancelled-owner-count 0) (define cancelled-owner-gate (channel/new 1))",
            )
            .expect("prepare cancellation handoff state");

        let owner = interp
            .submit_str(
                r#"(import "cancelled-owner-import.sema" cancelled-owner-answer)"#,
                RootOptions::default(),
            )
            .expect("submit import owner");
        let waiter = interp
            .submit_str(
                r#"(begin (import "cancelled-owner-import.sema" cancelled-owner-answer) cancelled-owner-answer)"#,
                RootOptions::default(),
            )
            .expect("submit queued import waiter");
        interp
            .drive_roots(&[owner.id()])
            .expect("park the owner in its module body");
        interp
            .drive_roots(&[waiter.id()])
            .expect("park the waiter on the import gate");
        assert!(matches!(owner.poll_result(), RootPoll::Pending));
        assert!(matches!(waiter.poll_result(), RootPoll::Pending));

        assert!(owner.cancel(CancelReason::Explicit));
        assert!(matches!(
            drive_selected_until_ready(&interp, &owner).outcome,
            TaskOutcome::Cancelled(CancelReason::Explicit)
        ));
        assert_eq!(
            returned_value(&drive_selected_until_ready(&interp, &waiter)),
            Value::int(42),
        );
        assert_eq!(
            interp
                .eval_str_via_runtime("cancelled-owner-count")
                .expect("the waiter alone retries the module"),
            Value::int(2),
        );
    }

    #[test]
    fn failed_runtime_import_owner_hands_off_to_one_waiter() {
        let interp = Interpreter::new();
        interp.ctx.set_embedded_file(
            std::path::PathBuf::from("failed-owner-import.sema"),
            b"(module failed-owner-import (export failed-owner-answer) (set! failed-owner-count (+ failed-owner-count 1)) (if (= failed-owner-count 1) (begin (channel/recv failed-owner-gate) (error \"expected first-owner failure\")) nil) (define failed-owner-answer 42))".to_vec(),
        );
        interp
            .eval_str_via_runtime(
                "(define failed-owner-count 0) (define failed-owner-gate (channel/new 1))",
            )
            .expect("prepare failure handoff state");

        let owner = interp
            .submit_str(
                r#"(import "failed-owner-import.sema" failed-owner-answer)"#,
                RootOptions::default(),
            )
            .expect("submit import owner");
        let waiter = interp
            .submit_str(
                r#"(begin (import "failed-owner-import.sema" failed-owner-answer) failed-owner-answer)"#,
                RootOptions::default(),
            )
            .expect("submit queued import waiter");
        interp
            .drive_roots(&[owner.id()])
            .expect("park the owner before its failure");
        interp
            .drive_roots(&[waiter.id()])
            .expect("park the waiter on the import gate");
        assert!(matches!(owner.poll_result(), RootPoll::Pending));
        assert!(matches!(waiter.poll_result(), RootPoll::Pending));

        interp
            .eval_str_via_runtime("(channel/send failed-owner-gate :continue)")
            .expect("resume the owner into its failure");
        assert!(matches!(
            drive_selected_until_ready(&interp, &owner).outcome,
            TaskOutcome::Failed(_)
        ));
        assert_eq!(
            returned_value(&drive_selected_until_ready(&interp, &waiter)),
            Value::int(42),
        );
        assert_eq!(
            interp
                .eval_str_via_runtime("failed-owner-count")
                .expect("the waiter alone retries after failure"),
            Value::int(2),
        );
    }

    #[test]
    fn cancelling_a_suspended_runtime_load_cleans_its_module_scopes() {
        let interp = Interpreter::new();
        interp.ctx.set_embedded_file(
            std::path::PathBuf::from("cancelled-load.sema"),
            b"(channel/recv cancelled-load-gate) :loaded".to_vec(),
        );
        interp
            .eval_str_via_runtime("(define cancelled-load-gate (channel/new 1))")
            .expect("create the cancellation gate");

        let first = interp
            .submit_str(r#"(load "cancelled-load.sema")"#, RootOptions::default())
            .expect("submit the first load");
        interp
            .drive_roots(&[first.id()])
            .expect("park the first load");
        assert!(matches!(first.poll_result(), RootPoll::Pending));
        assert!(first.cancel(CancelReason::Explicit));
        let cancelled = drive_selected_until_ready(&interp, &first);
        assert!(matches!(
            cancelled.outcome,
            TaskOutcome::Cancelled(CancelReason::Explicit)
        ));

        interp
            .eval_str_via_runtime("(channel/send cancelled-load-gate :retry)")
            .expect("seed the retry receive");
        assert_eq!(
            interp
                .eval_str_via_runtime(r#"(load "cancelled-load.sema")"#)
                .expect("the cancelled load left no false cycle or file scope"),
            Value::keyword("loaded"),
        );
        assert_eq!(interp.ctx.current_file_path(), None);
    }

    #[test]
    fn cancelling_a_suspended_runtime_import_allows_clean_retry() {
        let interp = Interpreter::new();
        let identity = "cancelled-import.sema";
        let export = "cancelled-import-answer";
        interp.ctx.set_embedded_file(
            std::path::PathBuf::from(identity),
            b"(module cancelled-import (export cancelled-import-answer) (channel/recv cancelled-import-gate) (define cancelled-import-answer 42))".to_vec(),
        );
        interp
            .eval_str_via_runtime("(define cancelled-import-gate (channel/new 1))")
            .expect("create cancelled import gate");

        let root = interp
            .submit_str(
                r#"(import "cancelled-import.sema" cancelled-import-answer)"#,
                RootOptions::default(),
            )
            .expect("submit cancellable import");
        interp
            .drive_roots(&[root.id()])
            .expect("park the import body");
        assert!(matches!(root.poll_result(), RootPoll::Pending));
        assert!(root.cancel(CancelReason::Explicit));
        let settlement = drive_selected_until_ready(&interp, &root);
        assert!(matches!(
            settlement.outcome,
            TaskOutcome::Cancelled(CancelReason::Explicit)
        ));
        assert_runtime_import_not_published(&interp, identity, export);

        interp
            .eval_str_via_runtime("(channel/send cancelled-import-gate :retry)")
            .expect("seed retry receive");
        assert_eq!(
            interp
                .eval_str_via_runtime(
                    r#"(begin (import "cancelled-import.sema" cancelled-import-answer) cancelled-import-answer)"#,
                )
                .expect("cancelled import releases single-flight and module scopes"),
            Value::int(42),
        );
    }

    #[test]
    fn runtime_import_parse_failure_cleans_state_for_retry() {
        let interp = Interpreter::new();
        let identity = "parse-failure-import.sema";
        let export = "parse-retry-answer";
        interp.ctx.set_embedded_file(
            std::path::PathBuf::from(identity),
            b"(module incomplete".to_vec(),
        );

        interp
            .eval_str_via_runtime(r#"(import "parse-failure-import.sema")"#)
            .expect_err("invalid source must fail import");
        assert_runtime_import_not_published(&interp, identity, export);

        interp.ctx.set_embedded_file(
            std::path::PathBuf::from(identity),
            b"(module parse-retry (export parse-retry-answer) (define parse-retry-answer 42))"
                .to_vec(),
        );
        assert_eq!(
            interp
                .eval_str_via_runtime(
                    r#"(begin (import "parse-failure-import.sema" parse-retry-answer) parse-retry-answer)"#,
                )
                .expect("parse failure leaves no false cycle or cache entry"),
            Value::int(42),
        );
    }

    #[test]
    fn runtime_import_compile_failure_after_suspension_cleans_state_for_retry() {
        let interp = Interpreter::new();
        let identity = "compile-failure-import.sema";
        let export = "compile-retry-answer";
        interp.ctx.set_embedded_file(
            std::path::PathBuf::from(identity),
            b"(channel/recv compile-failure-import-gate) (if #t)".to_vec(),
        );
        interp
            .eval_str_via_runtime("(define compile-failure-import-gate (channel/new 1))")
            .expect("create compile-failure suspension gate");

        let root = interp
            .submit_str(
                r#"(import "compile-failure-import.sema")"#,
                RootOptions::default(),
            )
            .expect("submit compile-failing import");
        interp
            .drive_roots(&[root.id()])
            .expect("park before compiling the invalid second form");
        assert!(matches!(root.poll_result(), RootPoll::Pending));
        interp
            .eval_str_via_runtime("(channel/send compile-failure-import-gate :continue)")
            .expect("resume the compile-failing import");
        let settlement = drive_selected_until_ready(&interp, &root);
        assert!(matches!(settlement.outcome, TaskOutcome::Failed(_)));
        assert_runtime_import_not_published(&interp, identity, export);

        interp.ctx.set_embedded_file(
            std::path::PathBuf::from(identity),
            b"(module compile-retry (export compile-retry-answer) (define compile-retry-answer 42))".to_vec(),
        );
        assert_eq!(
            interp
                .eval_str_via_runtime(
                    r#"(begin (import "compile-failure-import.sema" compile-retry-answer) compile-retry-answer)"#,
                )
                .expect("compile failure releases single-flight and module scopes"),
            Value::int(42),
        );
    }

    #[test]
    fn runtime_import_callback_failure_after_suspension_cleans_state_for_retry() {
        let interp = Interpreter::new();
        let identity = "callback-failure-import.sema";
        let export = "callback-retry-answer";
        interp.ctx.set_embedded_file(
            std::path::PathBuf::from(identity),
            b"(channel/recv callback-failure-import-gate) (error \"expected module callback failure\")".to_vec(),
        );
        interp
            .eval_str_via_runtime("(define callback-failure-import-gate (channel/new 1))")
            .expect("create callback-failure suspension gate");

        let root = interp
            .submit_str(
                r#"(import "callback-failure-import.sema")"#,
                RootOptions::default(),
            )
            .expect("submit callback-failing import");
        interp
            .drive_roots(&[root.id()])
            .expect("park before the failing callback");
        assert!(matches!(root.poll_result(), RootPoll::Pending));
        interp
            .eval_str_via_runtime("(channel/send callback-failure-import-gate :continue)")
            .expect("resume the callback-failing import");
        let settlement = drive_selected_until_ready(&interp, &root);
        assert!(matches!(settlement.outcome, TaskOutcome::Failed(_)));
        assert_runtime_import_not_published(&interp, identity, export);

        interp.ctx.set_embedded_file(
            std::path::PathBuf::from(identity),
            b"(module callback-retry (export callback-retry-answer) (define callback-retry-answer 42))".to_vec(),
        );
        assert_eq!(
            interp
                .eval_str_via_runtime(
                    r#"(begin (import "callback-failure-import.sema" callback-retry-answer) callback-retry-answer)"#,
                )
                .expect("callback failure releases single-flight and module scopes"),
            Value::int(42),
        );
    }

    #[test]
    fn malformed_runtime_bytecode_import_cleans_state_for_retry() {
        let interp = Interpreter::new();
        let identity = "malformed-import.semac";
        let export = "bytecode-retry-answer";
        interp.ctx.set_embedded_file(
            std::path::PathBuf::from(identity),
            b"\0SEMmalformed".to_vec(),
        );

        interp
            .eval_str_via_runtime(r#"(import "malformed-import.semac")"#)
            .expect_err("malformed bytecode must fail import");
        assert_runtime_import_not_published(&interp, identity, export);

        interp.ctx.set_embedded_file(
            std::path::PathBuf::from(identity),
            b"(module bytecode-retry (export bytecode-retry-answer) (define bytecode-retry-answer 42))".to_vec(),
        );
        assert_eq!(
            interp
                .eval_str_via_runtime(
                    r#"(begin (import "malformed-import.semac" bytecode-retry-answer) bytecode-retry-answer)"#,
                )
                .expect("malformed bytecode releases single-flight and module scopes"),
            Value::int(42),
        );
    }

    #[test]
    fn suspended_import_keeps_relative_resolution_and_export_scope() {
        let interp = Interpreter::new();
        interp.ctx.set_embedded_file(
            std::path::PathBuf::from("relative/inner.sema"),
            b"(module inner (export inner-value) (define inner-value 40))".to_vec(),
        );
        interp.ctx.set_embedded_file(
            std::path::PathBuf::from("relative/outer.sema"),
            b"(module outer (export answer) (async/sleep 2) (import \"./inner.sema\" inner-value) (define answer (+ inner-value 2)))".to_vec(),
        );

        assert_eq!(
            interp
                .eval_str_via_runtime(r#"(begin (import "relative/outer.sema" answer) answer)"#,)
                .expect("relative nested import resumes in the outer module scope"),
            Value::int(42),
        );
        assert_eq!(interp.ctx.current_file_path(), None);
    }

    #[test]
    fn runtime_import_executes_embedded_bytecode_cooperatively() {
        let interp = Interpreter::new();
        let compiled = interp
            .compile_to_bytecode(
                "(module bytecode (export answer) (async/sleep 2) (define answer 42))",
            )
            .expect("compile the embedded module");
        let bytes = sema_vm::serialize_to_bytes(&compiled, 0).expect("serialize the module");
        interp
            .ctx
            .set_embedded_file(std::path::PathBuf::from("runtime-bytecode.semac"), bytes);

        assert_eq!(
            interp
                .eval_str_via_runtime(r#"(begin (import "runtime-bytecode.semac" answer) answer)"#,)
                .expect("embedded bytecode can suspend under import"),
            Value::int(42),
        );
        assert_eq!(interp.ctx.current_file_path(), None);
    }

    #[test]
    fn runtime_module_continuation_traces_envs_and_source_values() {
        let interp = Interpreter::new();
        let root = interp
            .submit_str("nil", RootOptions::default())
            .expect("mint a runtime-scoped gate identity");
        let mut gate_ids = sema_core::runtime::RuntimeScopedIdCounter::<
            sema_core::runtime::ResourceGateId,
        >::new(root.id().runtime());
        let gate = gate_ids.allocate().expect("allocate import gate id");
        let state = Rc::new(sema_core::runtime::ModuleTaskState::default());
        let loading = state
            .push_loading(std::path::PathBuf::from("trace-module.sema"))
            .expect("allocate loading scope");
        let current_file = state
            .push_current_file(std::path::PathBuf::from("trace-module.sema"))
            .expect("allocate current-file scope");
        let exports = state.push_exports(None).expect("allocate export scope");
        let globals = Rc::new(Env::new());
        let target = Rc::new(Env::new());
        let source = RuntimeSourceModule {
            run: RuntimeModuleRun {
                scope: RuntimeModuleScope {
                    state,
                    loading,
                    current_file,
                    exports: Some(exports),
                },
                globals: Rc::clone(&globals),
                completion: RuntimeModuleCompletion::Import {
                    identity: std::path::PathBuf::from("trace-module.sema"),
                    selective: vec!["answer".to_string()],
                    target: Rc::clone(&target),
                    gate,
                    lease: ImportGateLease {
                        state: Weak::new(),
                        identity: std::path::PathBuf::from("trace-module.sema"),
                        gate,
                    },
                },
            },
            expressions: vec![Value::string("expression")],
            spans: sema_core::SpanMap::new(),
            source_file: std::path::PathBuf::from("trace-module.sema"),
            next: 0,
            last: Value::string("last"),
        };
        let mut globals_edges = 0;
        let mut target_edges = 0;
        let mut value_edges = 0;

        assert!(sema_core::runtime::Trace::trace(&source, &mut |edge| {
            match edge {
                sema_core::cycle::GcEdge::Env(env) if Rc::ptr_eq(env, &globals) => {
                    globals_edges += 1;
                }
                sema_core::cycle::GcEdge::Env(env) if Rc::ptr_eq(env, &target) => {
                    target_edges += 1;
                }
                sema_core::cycle::GcEdge::Value(_) => value_edges += 1,
                _ => {}
            }
        }));
        assert_eq!(globals_edges, 1);
        assert_eq!(target_edges, 1);
        assert_eq!(value_edges, 2);
    }

    #[test]
    fn runtime_concurrent_roots_isolate_dynamic_context_until_settlement() {
        let interp = Interpreter::new();
        interp
            .eval_str("(define root-context-gate (channel/new 1))")
            .expect("define root synchronization channel");

        let owner = Value::keyword("root-owner");
        let hidden = Value::keyword("root-hidden");
        let stack = Value::keyword("root-stack");
        let base = Value::keyword("base");
        let base_hidden = Value::keyword("base-hidden");
        let base_stack = Value::keyword("base-stack");
        interp.ctx.context_set(owner.clone(), base.clone());
        interp.ctx.hidden_set(hidden.clone(), base_hidden.clone());
        interp
            .ctx
            .context_stack_push(stack.clone(), base_stack.clone());

        let root_a = interp
            .submit_str(
                r#"(begin
                     (context/set :root-owner :a)
                     (context/set-hidden :root-hidden :a-hidden)
                     (context/push :root-stack :a-stack)
                     (channel/recv root-context-gate)
                     (list (context/get :root-owner)
                           (context/get-hidden :root-hidden)
                           (context/stack :root-stack)))"#,
                RootOptions::default(),
            )
            .expect("submit root A");
        let root_b = interp
            .submit_str(
                r#"(begin
                     (context/set :root-owner :b)
                     (context/set-hidden :root-hidden :b-hidden)
                     (context/push :root-stack :b-stack)
                     (list (context/get :root-owner)
                           (context/get-hidden :root-hidden)
                           (context/stack :root-stack)))"#,
                RootOptions::default(),
            )
            .expect("submit root B");

        interp
            .drive_roots(&[root_a.id()])
            .expect("drive root A to its channel wait");
        assert!(matches!(root_a.poll_result(), RootPoll::Pending));
        assert!(interp.ctx.task_context().is_none());
        assert_eq!(interp.ctx.context_get(&owner), Some(base.clone()));
        assert_eq!(interp.ctx.hidden_get(&hidden), Some(base_hidden.clone()));
        assert_eq!(
            interp.ctx.context_stack_get(&stack),
            vec![base_stack.clone()]
        );

        let settled_b = drive_selected_until_ready(&interp, &root_b);
        let expected_b = interp
            .eval_str("'(:b :b-hidden (:base-stack :b-stack))")
            .expect("evaluate root B oracle");
        assert_eq!(returned_value(&settled_b), expected_b);
        assert!(matches!(root_a.poll_result(), RootPoll::Pending));
        assert!(interp.ctx.task_context().is_none());
        assert_eq!(interp.ctx.context_get(&owner), Some(Value::keyword("b")));
        assert_eq!(
            interp.ctx.hidden_get(&hidden),
            Some(Value::keyword("b-hidden"))
        );
        assert_eq!(
            interp.ctx.context_stack_get(&stack),
            vec![base_stack.clone(), Value::keyword("b-stack")]
        );

        let signal = interp
            .submit_str(
                "(channel/send root-context-gate :go)",
                RootOptions::default(),
            )
            .expect("submit root A wake signal");
        drive_selected_until_ready(&interp, &signal);
        let settled_a = drive_selected_until_ready(&interp, &root_a);
        let expected_a = interp
            .eval_str("'(:a :a-hidden (:base-stack :a-stack))")
            .expect("evaluate root A oracle");
        assert_eq!(returned_value(&settled_a), expected_a);
        assert_eq!(interp.ctx.context_get(&owner), Some(Value::keyword("a")));
        assert_eq!(
            interp.ctx.hidden_get(&hidden),
            Some(Value::keyword("a-hidden"))
        );
        assert_eq!(
            interp.ctx.context_stack_get(&stack),
            vec![
                base_stack,
                Value::keyword("b-stack"),
                Value::keyword("a-stack")
            ]
        );
    }

    #[test]
    fn runtime_spawned_siblings_inherit_then_isolate_dynamic_context() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime(
                r#"(let ((gate (channel/new 1))
                          (ready (channel/new 1)))
                     (context/set :who :parent)
                     (context/set-hidden :secret :parent-hidden)
                     (context/push :layers :parent-stack)
                     (let ((a (async
                                (context/set :who :a)
                                (context/set-hidden :secret :a-hidden)
                                (context/push :layers :a-stack)
                                (channel/send ready :ready)
                                (channel/recv gate)
                                (list (context/get :who)
                                      (context/get-hidden :secret)
                                      (context/stack :layers))))
                           (b (async
                                (channel/recv ready)
                                (let ((before
                                        (list (context/get :who)
                                              (context/get-hidden :secret)
                                              (context/stack :layers))))
                                  (context/set :who :b)
                                  (context/set-hidden :secret :b-hidden)
                                  (context/push :layers :b-stack)
                                  (channel/send gate :go)
                                  (list before
                                        (list (context/get :who)
                                              (context/get-hidden :secret)
                                              (context/stack :layers)))))))
                       (list (await a)
                             (await b)
                             (list (context/get :who)
                                   (context/get-hidden :secret)
                                   (context/stack :layers)))))"#,
            )
            .expect("spawned siblings settle");

        let expected = interp
            .eval_str(
                "'((:a :a-hidden (:parent-stack :a-stack)) \
                   ((:parent :parent-hidden (:parent-stack)) \
                    (:b :b-hidden (:parent-stack :b-stack))) \
                   (:parent :parent-hidden (:parent-stack)))",
            )
            .expect("evaluate sibling-isolation oracle");
        assert_eq!(result, expected);
    }

    #[test]
    fn runtime_spawned_child_settlement_does_not_publish_child_context() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime(
                r#"(begin
                     (context/set :child-owner :parent)
                     (context/set-hidden :child-hidden :parent-hidden)
                     (context/push :child-stack :parent-stack)
                     (let ((child
                             (async
                               (context/set :child-owner :child)
                               (context/set-hidden :child-hidden :child-hidden)
                               (context/push :child-stack :child-stack)
                               (list (context/get :child-owner)
                                     (context/get-hidden :child-hidden)
                                     (context/stack :child-stack)))))
                       (list (await child)
                             (list (context/get :child-owner)
                                   (context/get-hidden :child-hidden)
                                   (context/stack :child-stack)))))"#,
            )
            .expect("child and root settle without child publication");
        let expected = interp
            .eval_str(
                "'((:child :child-hidden (:parent-stack :child-stack)) \
                   (:parent :parent-hidden (:parent-stack)))",
            )
            .expect("evaluate child-publication oracle");
        assert_eq!(result, expected);
        assert_eq!(
            interp.ctx.context_get(&Value::keyword("child-owner")),
            Some(Value::keyword("parent"))
        );
        assert_eq!(
            interp.ctx.hidden_get(&Value::keyword("child-hidden")),
            Some(Value::keyword("parent-hidden"))
        );
        assert_eq!(
            interp.ctx.context_stack_get(&Value::keyword("child-stack")),
            vec![Value::keyword("parent-stack")]
        );
    }

    #[test]
    fn runtime_context_with_frame_is_task_local_and_cleanup_is_exact() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime(
                r#"(begin
                     (context/set :scoped :outer)
                     (list
                       (context/with {:scoped :inner :inner-only :value}
                         (fn ()
                           (list (context/get :scoped)
                                 (context/get :inner-only)
                                 (context/all))))
                       (context/get :scoped)
                       (context/get :inner-only)))"#,
            )
            .expect("context/with runs against the root task snapshot");
        let expected = interp
            .eval_str("'((:inner :value {:inner-only :value :scoped :inner}) :outer nil)")
            .expect("evaluate context/with oracle");
        assert_eq!(result, expected);
        assert_eq!(
            interp.ctx.context_get(&Value::keyword("scoped")),
            Some(Value::keyword("outer"))
        );
        assert_eq!(interp.ctx.context_get(&Value::keyword("inner-only")), None);
    }

    #[test]
    fn runtime_context_with_suspends_and_child_inherits_only_inside_scope() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime(
                r#"(begin
                     (context/set :scoped :outer)
                     (list
                       (context/with {:scoped :inner}
                         (fn ()
                           (async/sleep 2)
                           (let ((child (async (context/get :scoped))))
                             (list (await child) (context/get :scoped)))))
                       (context/get :scoped)
                       (await (async (context/get :scoped)))))"#,
            )
            .expect("context/with cooperates across suspension and child spawn");
        let expected = interp
            .eval_str("'((:inner :inner) :outer :outer)")
            .expect("evaluate scoped-context oracle");
        assert_eq!(result, expected);
        assert_eq!(
            interp.ctx.context_get(&Value::keyword("scoped")),
            Some(Value::keyword("outer"))
        );
    }

    #[test]
    fn runtime_context_with_does_not_leak_into_preexisting_sibling() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime(
                r#"(let ((go (channel/new 1))
                          (ready (channel/new 1)))
                     (context/set :scoped :outer)
                     (let ((sibling
                             (async
                               (channel/send ready :ready)
                               (channel/recv go)
                               (context/get :scoped))))
                       (channel/recv ready)
                       (list
                         (context/with {:scoped :inner}
                           (fn ()
                             (channel/send go :go)
                             (async/sleep 2)
                             (list (context/get :scoped) (await sibling))))
                         (context/get :scoped))))"#,
            )
            .expect("preexisting sibling remains outside context/with scope");
        let expected = interp
            .eval_str("'((:inner :outer) :outer)")
            .expect("evaluate sibling-isolation oracle");
        assert_eq!(result, expected);
    }

    #[test]
    fn runtime_context_with_preserves_captured_cell_mutation_across_suspend() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime(
                r#"(let ((captured 0))
                     (context/with {:scoped :inner}
                       (fn ()
                         (async/sleep 2)
                         (set! captured 41)
                         :done))
                     (+ captured 1))"#,
            )
            .expect("context/with closes the thunk's captured cells before parking");
        assert_eq!(result, Value::int(42));
    }

    #[test]
    fn runtime_context_with_accepts_scope_already_removed_by_context_clear() {
        let interp = Interpreter::new();
        interp
            .ctx
            .context_set(Value::keyword("outer"), Value::keyword("present"));

        let result = interp
            .eval_str_via_runtime(
                r#"(context/with {:scoped :inner}
                     (fn () (async/sleep 2) (context/clear) 42))"#,
            )
            .expect("context/clear makes context/with teardown idempotent");

        assert_eq!(result, Value::int(42));
        assert!(interp.ctx.context_all().is_empty());
    }

    #[test]
    fn runtime_context_with_failure_removes_only_its_owned_frame() {
        let interp = Interpreter::new();
        interp
            .ctx
            .context_set(Value::keyword("scoped"), Value::keyword("outer"));

        let error = interp
            .eval_str_via_runtime(
                r#"(context/with {:scoped :inner :inner-only :value}
                     (fn () (async/sleep 2) (error "expected failure")))"#,
            )
            .expect_err("context/with body fails");

        assert!(error.to_string().contains("expected failure"));
        assert_eq!(
            interp.ctx.context_get(&Value::keyword("scoped")),
            Some(Value::keyword("outer"))
        );
        assert_eq!(interp.ctx.context_get(&Value::keyword("inner-only")), None);
    }

    #[test]
    fn runtime_context_with_cancellation_does_not_publish_scoped_bindings() {
        let interp = Interpreter::new();
        interp
            .ctx
            .context_set(Value::keyword("scoped"), Value::keyword("outer"));
        let root = interp
            .submit_str(
                r#"(context/with {:scoped :inner :inner-only :value}
                     (fn () (channel/recv (channel/new 1))))"#,
                RootOptions::default(),
            )
            .expect("submit cancellable context/with root");

        interp
            .drive_roots(&[root.id()])
            .expect("drive context/with body to channel wait");
        assert!(matches!(root.poll_result(), RootPoll::Pending));
        assert!(root.cancel(CancelReason::Explicit));
        let settlement = drive_selected_until_ready(&interp, &root);
        assert!(matches!(
            settlement.outcome,
            TaskOutcome::Cancelled(CancelReason::Explicit)
        ));
        assert_eq!(
            interp.ctx.context_get(&Value::keyword("scoped")),
            Some(Value::keyword("outer"))
        );
        assert_eq!(interp.ctx.context_get(&Value::keyword("inner-only")), None);
    }

    #[test]
    fn simultaneous_interpreters_isolate_colliding_context_with_scope_ids() {
        let interp_a = Interpreter::new();
        let interp_b = Interpreter::new();
        interp_a
            .eval_str_via_runtime(
                "(context/set :scoped :outer-a) (define context-gate (channel/new 1))",
            )
            .expect("prepare interpreter A");
        interp_b
            .eval_str_via_runtime(
                "(context/set :scoped :outer-b) (define context-gate (channel/new 1))",
            )
            .expect("prepare interpreter B");
        let root_a = interp_a
            .submit_str(
                "(context/with {:scoped :inner-a} (fn () (channel/recv context-gate) (context/get :scoped)))",
                RootOptions::default(),
            )
            .expect("submit interpreter A scope");
        let root_b = interp_b
            .submit_str(
                "(context/with {:scoped :inner-b} (fn () (channel/recv context-gate) (context/get :scoped)))",
                RootOptions::default(),
            )
            .expect("submit interpreter B scope");

        interp_a
            .drive_roots(&[root_a.id()])
            .expect("park interpreter A scope");
        interp_b
            .drive_roots(&[root_b.id()])
            .expect("park interpreter B scope");
        assert!(matches!(root_a.poll_result(), RootPoll::Pending));
        assert!(matches!(root_b.poll_result(), RootPoll::Pending));

        assert!(root_a.cancel(CancelReason::Explicit));
        let cancelled = drive_selected_until_ready(&interp_a, &root_a);
        assert!(matches!(
            cancelled.outcome,
            TaskOutcome::Cancelled(CancelReason::Explicit)
        ));
        assert!(matches!(root_b.poll_result(), RootPoll::Pending));
        interp_b
            .eval_str_via_runtime("(channel/send context-gate :go)")
            .expect("release interpreter B scope");
        let settled_b = drive_selected_until_ready(&interp_b, &root_b);
        assert!(matches!(
            settled_b.outcome,
            TaskOutcome::Returned(ref value) if *value == Value::keyword("inner-b")
        ));
        assert_eq!(
            interp_a.ctx.context_get(&Value::keyword("scoped")),
            Some(Value::keyword("outer-a"))
        );
        assert_eq!(
            interp_b.ctx.context_get(&Value::keyword("scoped")),
            Some(Value::keyword("outer-b"))
        );
    }

    #[test]
    fn runtime_root_publishes_sets_into_preexisting_ambient_frames() {
        let interp = Interpreter::new();
        interp.ctx.context_push_frame_with(BTreeMap::from([(
            Value::keyword("user"),
            Value::keyword("ambient-user"),
        )]));
        interp.ctx.hidden_push_frame();
        interp
            .ctx
            .hidden_set(Value::keyword("hidden"), Value::keyword("ambient-hidden"));

        interp
            .eval_str_via_runtime(
                "(context/set :user :root-user) (context/set-hidden :hidden :root-hidden)",
            )
            .expect("root settles into ambient frames");

        assert_eq!(
            interp.ctx.context_get(&Value::keyword("user")),
            Some(Value::keyword("root-user"))
        );
        assert_eq!(
            interp.ctx.hidden_get(&Value::keyword("hidden")),
            Some(Value::keyword("root-hidden"))
        );
    }

    #[test]
    fn runtime_root_publishes_remove_from_preexisting_ambient_frame() {
        let interp = Interpreter::new();
        interp.ctx.context_push_frame_with(BTreeMap::from([(
            Value::keyword("removed"),
            Value::keyword("ambient"),
        )]));

        let result = interp
            .eval_str_via_runtime("(context/remove :removed)")
            .expect("root removes ambient binding");

        assert_eq!(result, Value::keyword("ambient"));
        assert_eq!(interp.ctx.context_get(&Value::keyword("removed")), None);
    }

    #[test]
    fn runtime_cancelled_root_publishes_only_at_settlement() {
        let interp = Interpreter::new();
        let user = Value::keyword("cancelled-user");
        let hidden = Value::keyword("cancelled-hidden");
        let stack = Value::keyword("cancelled-stack");
        let root = interp
            .submit_str(
                r#"(begin
                     (context/set :cancelled-user :kept)
                     (context/set-hidden :cancelled-hidden :kept-hidden)
                     (context/push :cancelled-stack :kept-stack)
                     (channel/recv (channel/new 1)))"#,
                RootOptions::default(),
            )
            .expect("submit cancellable root");

        interp
            .drive_roots(&[root.id()])
            .expect("drive cancellable root to its channel wait");
        assert!(matches!(root.poll_result(), RootPoll::Pending));
        assert_eq!(interp.ctx.context_get(&user), None);
        assert_eq!(interp.ctx.hidden_get(&hidden), None);
        assert!(interp.ctx.context_stack_get(&stack).is_empty());

        assert!(root.cancel(CancelReason::Explicit));
        let settlement = drive_selected_until_ready(&interp, &root);
        assert!(matches!(
            settlement.outcome,
            TaskOutcome::Cancelled(CancelReason::Explicit)
        ));
        assert_eq!(interp.ctx.context_get(&user), Some(Value::keyword("kept")));
        assert_eq!(
            interp.ctx.hidden_get(&hidden),
            Some(Value::keyword("kept-hidden"))
        );
        assert_eq!(
            interp.ctx.context_stack_get(&stack),
            vec![Value::keyword("kept-stack")]
        );
        assert!(matches!(root.poll_result(), RootPoll::Ready(_)));
        interp
            .drive_roots(&[root.id()])
            .expect("driving an already-settled root remains harmless");
        assert_eq!(
            interp.ctx.context_stack_get(&stack),
            vec![Value::keyword("kept-stack")]
        );
    }

    #[test]
    fn runtime_failed_root_publishes_dynamic_context_exactly_once() {
        let interp = Interpreter::new();
        let user = Value::keyword("failed-user");
        let hidden = Value::keyword("failed-hidden");
        let stack = Value::keyword("failed-stack");
        let root = interp
            .submit_str(
                r#"(begin
                     (context/set :failed-user :kept)
                     (context/set-hidden :failed-hidden :kept-hidden)
                     (context/push :failed-stack :kept-stack)
                     (error "expected failure"))"#,
                RootOptions::default(),
            )
            .expect("submit failing root");

        let settlement = drive_selected_until_ready(&interp, &root);
        assert!(matches!(settlement.outcome, TaskOutcome::Failed(_)));
        assert_eq!(interp.ctx.context_get(&user), Some(Value::keyword("kept")));
        assert_eq!(
            interp.ctx.hidden_get(&hidden),
            Some(Value::keyword("kept-hidden"))
        );
        assert_eq!(
            interp.ctx.context_stack_get(&stack),
            vec![Value::keyword("kept-stack")]
        );

        assert!(matches!(root.poll_result(), RootPoll::Ready(_)));
        interp
            .drive_roots(&[root.id()])
            .expect("driving an already-failed root remains harmless");
        assert_eq!(
            interp.ctx.context_stack_get(&stack),
            vec![Value::keyword("kept-stack")]
        );
    }

    // GATE (clean drop with a detached timer-parked task): an interpreter whose
    // persistent runtime still holds a detached task parked on a timer must drop
    // WITHOUT hanging. `Drop` runs a BOUNDED `shutdown` (finite deadline) that
    // cancels + reaps the task before the global-env teardown collection. If the
    // shutdown could hang this test would hang the process; the wall-clock
    // assertion turns a partial regression into a failure rather than a timeout.
    #[test]
    fn runtime_drop_with_detached_timer_parked_task_does_not_hang() {
        let start = std::time::Instant::now();
        {
            let interp = Interpreter::new();
            // Detach a long-sleeping task and return before it can fire, so the
            // interpreter is dropped with the task still parked on its timer.
            let result = interp
                .eval_str_via_runtime("(async/spawn (fn () (async/sleep 100000) 1)) 7")
                .expect("root returns with a detached timer-parked task still live");
            assert_eq!(result, Value::int(7));
        } // interp dropped here — bounded shutdown must not wait out the 100s timer
        let elapsed = start.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "dropping an interpreter with a detached timer-parked task must be \
             bounded, not block on the sleep deadline (took {elapsed:?})",
        );
    }

    // ── ADVERSARIAL VERIFICATION (spawn/await seam) ──────────────────

    // Awaiting a spawned task that raises settles the awaiting root Failed with
    // the real rejection error (not a panic, hang, or wrong Ok value).
    #[test]
    fn runtime_await_rejected_spawn_settles_failed() {
        let interp = Interpreter::new();
        let result = interp.eval_str_via_runtime("(await (async/spawn (fn () (error \"boom\"))))");
        let msg = format!("{}", result.expect_err("await of a raising task must fail"));
        assert!(msg.contains("boom"), "missing cause: {msg}");
    }

    // Await of an ALREADY-settled promise resumes with the value: spawn, let the
    // child settle during a sleep, then await -> returns the resolved value with
    // no double-settle or hang.
    #[test]
    fn runtime_await_already_settled_promise() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime("(let ((p (async/spawn (fn () 5)))) (async/sleep 5) (await p))")
            .expect("await of a settled promise resolves");
        assert_eq!(result, Value::int(5));
    }

    // Two distinct tasks awaiting the SAME spawned promise both get the value
    // exactly once (no lost/duplicate wake).
    #[test]
    fn runtime_multiple_awaiters_same_promise() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime(
                "(begin \
                   (define p (async/spawn (fn () 99))) \
                   (define a (async/spawn (fn () (await p)))) \
                   (define b (async/spawn (fn () (await p)))) \
                   (+ (await a) (await b)))",
            )
            .expect("both awaiters resolve");
        assert_eq!(result, Value::int(198));
    }

    // A spawned task that itself spawns and awaits a nested detached task.
    #[test]
    fn runtime_nested_spawn_await() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime("(await (async/spawn (fn () (await (async/spawn (fn () 42))))))")
            .expect("nested spawn/await resolves");
        assert_eq!(result, Value::int(42));
    }

    // A spawned task that is never awaited does not corrupt the root result: the
    // root settles with its own value regardless of the detached child.
    #[test]
    fn runtime_fire_and_forget_spawn() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime("(begin (async/spawn (fn () 1)) 99)")
            .expect("root settles regardless of the detached child");
        assert_eq!(result, Value::int(99));
    }

    // DEADLOCK SAFETY (scenario 5): a task parked on its own never-settling
    // promise (and the root parked on it too) must make the drive loop TERMINATE
    // with an error, never hang. Child sleeps first so the self-await genuinely
    // parks on a pending promise.
    #[test]
    fn runtime_self_await_deadlock_terminates_not_hangs() {
        let interp = Interpreter::new();
        let result = interp.eval_str_via_runtime(
            "(begin (define p nil) \
                    (set! p (async/spawn (fn () (async/sleep 1) (await p)))) \
                    (await p))",
        );
        // The exact error text is not load-bearing; the point is it RETURNS.
        assert!(
            result.is_err(),
            "deadlock must surface as Err, got {result:?}"
        );
    }

    // ── DEADLOCK / ALL-BLOCKED DETECTION (legacy-parity gates) ───────────────
    //
    // When the requested root can make no further progress — parked on an
    // intra-runtime wait (channel/promise) that no runnable task can satisfy,
    // with no pending timer or external completion — the drive loop settles the
    // root Failed with the SAME error the legacy evaluator (`eval_str`) produces,
    // rather than hanging or returning an opaque "root did not settle". Each gate
    // pins the runtime error string to the `eval_str` oracle.

    /// Run `program` through the runtime on a dedicated thread with a hard wall
    /// clock, so a detection regression surfaces as a test failure (bounded) and
    /// never wedges the suite. `Interpreter` is `!Send` (it holds `Rc`s), so the
    /// interpreter is constructed *inside* the worker; only the `Result`'s string
    /// projection (which is `Send`) crosses the channel back.
    fn runtime_eval_bounded(program: &'static str) -> Result<String, String> {
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let interp = Interpreter::new();
            let projected = interp
                .eval_str_via_runtime(program)
                .map(|v| format!("{v:?}"))
                .map_err(|e| e.to_string());
            let _ = tx.send(projected);
        });
        match rx.recv_timeout(std::time::Duration::from_secs(30)) {
            Ok(projected) => {
                worker.join().expect("runtime worker thread panicked");
                projected
            }
            Err(_) => panic!("eval_str_via_runtime hung (no termination in 30s) for {program:?}"),
        }
    }

    /// Assert the runtime path fails with the exact `eval_str` legacy oracle
    /// error string, within a bounded wall clock (never hangs).
    fn assert_runtime_deadlock_matches_oracle(program: &'static str) {
        let oracle = Interpreter::new()
            .eval_str(program)
            .err()
            .unwrap_or_else(|| panic!("legacy oracle unexpectedly succeeded for {program:?}"))
            .to_string();
        let got = runtime_eval_bounded(program)
            .err()
            .unwrap_or_else(|| panic!("runtime unexpectedly succeeded for {program:?}"));
        assert_eq!(
            got, oracle,
            "runtime deadlock error != legacy oracle for {program:?}"
        );
    }

    // GATE — top-level `channel/recv` on an empty channel with no sender parks
    // the root (everything runs in a runtime quantum), and the drive loop settles
    // it with the legacy "channel/recv: channel is empty".
    #[test]
    fn runtime_deadlock_toplevel_recv_empty_matches_oracle() {
        assert_runtime_deadlock_matches_oracle("(channel/recv (channel/new 1))");
    }

    // GATE — top-level `channel/send` on a full channel with no receiver parks
    // the root; the drive loop settles it with the legacy "channel/send: channel
    // is full". (`(channel/new 0)` is rejected before it can be sent to — capacity
    // must be >= 1 — so a genuine full channel is a cap-1 channel fed twice.)
    #[test]
    fn runtime_deadlock_toplevel_send_full_matches_oracle() {
        assert_runtime_deadlock_matches_oracle(
            "(begin (define ch (channel/new 1)) (channel/send ch 1) (channel/send ch 2))",
        );
    }

    // GATE — two spawned tasks mutually awaiting each other (sleep-first so both
    // promises exist before either await parks) is a genuine cross-task deadlock:
    // the root settles with the legacy "async scheduler: all tasks blocked
    // (deadlock detected)" and the drive loop TERMINATES (bounded by the guard).
    #[test]
    fn runtime_deadlock_mutual_await_terminates_matches_oracle() {
        assert_runtime_deadlock_matches_oracle(
            "(begin \
               (define pa nil) (define pb nil) \
               (set! pa (async/spawn (fn () (async/sleep 1) (await pb)))) \
               (set! pb (async/spawn (fn () (async/sleep 1) (await pa)))) \
               (await pa))",
        );
    }

    // GATE — a detached task legitimately parked on a REAL timer must NOT be
    // misclassified as a deadlock: while it sleeps the drive loop reports
    // `Idle { next_deadline: Some, .. }` (not the all-idle deadlock state), so a
    // new root that completes synchronously in the same runtime returns its value
    // normally and the sleeping detached task survives to settle later. This
    // guards that deadlock detection keys on the fully-idle state alone, never on
    // "some task is parked".
    #[test]
    fn runtime_detached_timer_park_is_not_deadlock() {
        let interp = Interpreter::new();
        // Detached task parked on a real 5s timer; it does not settle within this
        // eval, but the runtime persists it (survival gate).
        interp
            .eval_str_via_runtime("(async/spawn (fn () (async/sleep 5000) 1))")
            .expect("spawning a sleeping detached task returns its promise");
        // A subsequent synchronous root must settle with its own value, NOT be
        // misreported as deadlocked because a detached task is parked on a timer.
        let result = interp
            .eval_str_via_runtime("(+ 1 2)")
            .expect("a synchronous root settles even while a detached timer is parked");
        assert_eq!(result, Value::int(3));
    }

    // Catchability of an await rejection must NOT be timing-dependent. When the
    // awaited promise is still Pending at the moment `async/await` runs, the
    // root parks and is later resumed via `VmResume::Fail` (state.rs
    // visit_ready). That resume now RAISES the error at the parked call site (as
    // if the awaiting native returned `Err`) rather than settling the whole task
    // Failed — so an enclosing `try`/`catch` catches it, identical to the
    // already-settled fast path (native's Rejected branch in async_ops.rs). Both
    // scheduling orders (child sleeps first, so the await genuinely parks on a
    // pending promise) must behave the same. An UNCAUGHT rejection still settles
    // the root Failed with the real error.
    #[test]
    fn runtime_await_pending_rejection_is_catchable() {
        let interp = Interpreter::new();
        // Pending-then-rejected await, wrapped in try/catch → catchable.
        let result = interp
            .eval_str_via_runtime(
                "(try (await (async/spawn (fn () (async/sleep 2) (error \"x\")))) \
                      (catch e \"caught\"))",
            )
            .expect("try/catch should catch a rejected await");
        assert_eq!(
            result,
            Value::string("caught"),
            "a rejected await must be catchable regardless of timing"
        );

        // Uncaught pending-then-rejected await → still settles Failed with the
        // real error (the fix must not swallow uncaught rejections).
        let uncaught = interp
            .eval_str_via_runtime("(await (async/spawn (fn () (async/sleep 2) (error \"boom\"))))");
        let err = uncaught.expect_err("an uncaught rejected await must settle Failed");
        assert!(
            err.to_string().contains("boom"),
            "uncaught rejection must carry the real error, got: {err}"
        );
    }

    // ── OBSERVATIONAL COMBINATORS (Task 04) ──────────────────────────

    /// Evaluate a Sema literal on a fresh interpreter to build an expected value.
    fn lit(program: &str) -> Value {
        Interpreter::new()
            .eval_str(program)
            .unwrap_or_else(|e| panic!("literal eval failed for {program:?}: {e:?}"))
    }

    #[test]
    fn runtime_async_all_returns_values_in_input_order() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime(
                "(async/all (list (async/spawn (fn () 1)) \
                                  (async/spawn (fn () 2)) \
                                  (async/spawn (fn () 3))))",
            )
            .expect("async/all resolves through the runtime");
        assert_eq!(result, lit("(list 1 2 3)"));
    }

    #[test]
    fn runtime_async_all_empty_input_is_empty_list() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime("(async/all (list))")
            .expect("async/all of empty input");
        assert_eq!(result, Value::list(vec![]));
    }

    // A failing member raises, but a supplied sibling STILL runs to completion:
    // it records its side effect into a channel, observable after the failure.
    #[test]
    fn runtime_async_all_failure_does_not_cancel_sibling() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime(
                "(begin \
                   (define ch (channel/new 1)) \
                   (define sib (async/spawn (fn () (async/sleep 4) (channel/send ch 77) 1))) \
                   (define bad (async/spawn (fn () (error \"boom\")))) \
                   (define outcome (try (async/all (list bad sib)) (catch e :caught))) \
                   (list outcome (await sib) (channel/recv ch)))",
            )
            .expect("failure surfaces but sibling completes");
        assert_eq!(result, lit("(list :caught 1 77)"));
    }

    #[test]
    fn runtime_async_race_returns_fast_and_loser_continues() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime(
                "(begin \
                   (define ch (channel/new 1)) \
                   (define fast (async/spawn (fn () 10))) \
                   (define slow (async/spawn (fn () (async/sleep 5) (channel/send ch 88) 20))) \
                   (define winner (async/race (list fast slow))) \
                   (list winner (await slow) (channel/recv ch)))",
            )
            .expect("race returns the fast value and the loser continues");
        assert_eq!(result, lit("(list 10 20 88)"));
    }

    #[test]
    fn runtime_async_timeout_settled_wins() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime("(async/timeout 10000 (async/spawn (fn () (async/sleep 1) 5)))")
            .expect("a promise that settles before the deadline wins");
        assert_eq!(result, Value::int(5));
    }

    #[test]
    fn runtime_async_timeout_pending_raises_and_producer_continues() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime(
                "(begin \
                   (define ch (channel/new 1)) \
                   (define slow (async/spawn (fn () (async/sleep 20) (channel/send ch 55) 9))) \
                   (define outcome (try (async/timeout 1 slow) (catch e :timeout))) \
                   (list outcome (await slow) (channel/recv ch)))",
            )
            .expect("timeout raises but the producer keeps running");
        assert_eq!(result, lit("(list :timeout 9 55)"));
    }

    // ── Channel gates (Task 04): channel ops routed through the runtime's
    // canonical ChannelRegistry via the ChannelSend/ChannelRecv/ChannelClose
    // yield seam. Unlike the earlier async/all|race|timeout channel uses (which
    // never blocked — the Sema buffer served them synchronously), these gates
    // exercise cross-task rendezvous, blocking send/recv, and close.

    // GATE 1 — unbuffered rendezvous: a spawned producer sends across tasks and
    // the main task receives the value.
    #[test]
    fn runtime_channel_rendezvous_across_tasks() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime(
                "(begin \
                   (define ch (channel/new 1)) \
                   (async/spawn (fn () (channel/send ch 42))) \
                   (channel/recv ch))",
            )
            .expect("value sent from a spawned task arrives at the receiver");
        assert_eq!(result, Value::int(42));
    }

    // GATE 2 — buffered channel: sends up to capacity don't block and receive
    // preserves FIFO order.
    #[test]
    fn runtime_channel_buffered_fifo_order() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime(
                "(begin \
                   (define ch (channel/new 3)) \
                   (channel/send ch 1) (channel/send ch 2) (channel/send ch 3) \
                   (list (channel/recv ch) (channel/recv ch) (channel/recv ch)))",
            )
            .expect("buffered sends receive in FIFO order");
        assert_eq!(result, lit("(list 1 2 3)"));
    }

    // GATE 3 — blocking send on a full channel parks the sender until a receiver
    // takes the value. A capacity-1 channel fed 3 values REQUIRES the sender to
    // park twice; without parking, values would be lost or error. All three
    // arrive in order, proving the sender blocked and resumed.
    #[test]
    fn runtime_channel_blocking_send_parks_until_received() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime(
                "(begin \
                   (define ch (channel/new 1)) \
                   (define p (async/spawn (fn () \
                       (channel/send ch 1) (channel/send ch 2) (channel/send ch 3) :done))) \
                   (define out (list (channel/recv ch) (channel/recv ch) (channel/recv ch))) \
                   (list out (await p)))",
            )
            .expect("a full-channel send parks until the receiver drains it");
        assert_eq!(result, lit("(list (list 1 2 3) :done)"));
    }

    // GATE 4 — blocking receive on an empty channel parks the receiver until a
    // value is sent (the producer sleeps first, so the receiver must park).
    #[test]
    fn runtime_channel_blocking_recv_parks_until_sent() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime(
                "(begin \
                   (define ch (channel/new 1)) \
                   (async/spawn (fn () (async/sleep 2) (channel/send ch 77))) \
                   (channel/recv ch))",
            )
            .expect("an empty-channel receive parks until a value is sent");
        assert_eq!(result, Value::int(77));
    }

    // GATE 5a — receiving from a closed+empty channel returns the closed sentinel
    // (nil), after draining any buffered values first. Parity with `eval_str`.
    #[test]
    fn runtime_channel_recv_after_close_drains_then_sentinel() {
        assert_runtime_matches_oracle(
            "(begin \
               (define ch (channel/new 2)) \
               (channel/send ch 1) \
               (channel/close ch) \
               (list (channel/recv ch) (channel/recv ch)))",
        );
    }

    // GATE 5b — sending to a closed channel raises a catchable condition.
    #[test]
    fn runtime_channel_send_to_closed_errors() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime(
                "(begin \
                   (define ch (channel/new 1)) \
                   (channel/close ch) \
                   (try (channel/send ch 9) (catch e :send-failed)))",
            )
            .expect("send to a closed channel raises, caught by try");
        assert_eq!(result, Value::keyword("send-failed"));
    }

    // GATE 6 — capacity is validated before allocation: a zero/negative capacity
    // returns a Sema condition (catchable), never a panic.
    #[test]
    fn runtime_channel_rejects_invalid_capacity() {
        let interp = Interpreter::new();
        assert_eq!(
            interp
                .eval_str_via_runtime("(try (channel/new 0) (catch e :bad-capacity))")
                .expect("zero capacity is a condition, not a panic"),
            Value::keyword("bad-capacity"),
        );
        assert_eq!(
            interp
                .eval_str_via_runtime("(try (channel/new -1) (catch e :bad-capacity))")
                .expect("negative capacity is a condition, not a panic"),
            Value::keyword("bad-capacity"),
        );
    }

    // GATE 7 — observational channel ops read the canonical ChannelRegistry
    // under the unified runtime (regression: they previously read the empty Sema
    // buffer, so `channel/count` reported 0 and `channel/try-recv` stranded the
    // value in the registry — silent data loss). Each asserts parity with the
    // `eval_str` oracle.
    #[test]
    fn runtime_channel_count_reflects_buffered_sends() {
        assert_runtime_matches_oracle(
            "(begin \
               (define ch (channel/new 5)) \
               (channel/send ch 10) \
               (channel/send ch 20) \
               (channel/count ch))",
        );
    }

    #[test]
    fn runtime_channel_try_recv_returns_buffered_value() {
        // The sent value must come back (no silent data loss), and a second
        // try-recv on the now-empty channel returns the nil sentinel.
        assert_runtime_matches_oracle(
            "(begin \
               (define ch (channel/new 5)) \
               (channel/send ch 10) \
               (channel/send ch 20) \
               (list (channel/try-recv ch) (channel/try-recv ch) (channel/try-recv ch)))",
        );
    }

    #[test]
    fn runtime_channel_empty_and_full_reflect_registry_state() {
        assert_runtime_matches_oracle(
            "(begin \
               (define ch (channel/new 2)) \
               (define before (list (channel/empty? ch) (channel/full? ch))) \
               (channel/send ch 1) \
               (channel/send ch 2) \
               (list before (channel/empty? ch) (channel/full? ch)))",
        );
    }

    #[test]
    fn runtime_channel_try_recv_after_close_drains_then_sentinel() {
        assert_runtime_matches_oracle(
            "(begin \
               (define ch (channel/new 2)) \
               (channel/send ch 7) \
               (channel/close ch) \
               (list (channel/closed? ch) (channel/try-recv ch) (channel/try-recv ch)))",
        );
    }

    // ── CANCELLATION (Task 04) ───────────────────────────────────────

    // GATE 1: `async/cancel` returns `#t` ONLY for the FIRST cancellation request
    // of a pending spawned task; a second request on the same task returns `#f`
    // (already requested / terminal). Idempotent.
    #[test]
    fn runtime_async_cancel_first_request_true_second_false() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime(
                "(let ((p (async/spawn (fn () (async/sleep 100000) 42)))) \
                   (list (async/cancel p) (async/cancel p)))",
            )
            .expect("async/cancel of a sleeping spawned task drives through the runtime");
        assert_eq!(
            result,
            Value::list(vec![Value::bool(true), Value::bool(false)]),
            "first cancel is the newly-requested #t; the second is #f",
        );
    }

    // GATE 1b: `async/cancel` returns `#f` for a synthetic promise (no backing
    // spawned task) — there is nothing to cancel, and it never errors.
    #[test]
    fn runtime_async_cancel_synthetic_promise_is_false() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime("(async/cancel (async/resolved 5))")
            .expect("cancelling a synthetic promise is a no-op boolean, not an error");
        assert_eq!(result, Value::bool(false));
    }

    // GATE 2: awaiting a cancelled promise raises a STRUCTURED, catchable
    // `:cancelled` condition (not a plain error, not a value). `(:type e)` on the
    // caught condition is `:cancelled`.
    #[test]
    fn runtime_await_cancelled_promise_raises_cancelled_condition() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime(
                "(let ((p (async/spawn (fn () (async/sleep 100000) 42)))) \
                   (async/cancel p) \
                   (try (await p) (catch e (:type e))))",
            )
            .expect("awaiting a cancelled promise raises a catchable condition");
        assert_eq!(
            result,
            Value::keyword("cancelled"),
            "the caught condition's :type must be :cancelled",
        );
    }

    // GATE 2b: an UNCAUGHT `(await <cancelled>)` settles the root errored (Failed
    // with the cancellation), never Returned.
    #[test]
    fn runtime_await_cancelled_uncaught_settles_errored() {
        let interp = Interpreter::new();
        let result = interp.eval_str_via_runtime(
            "(let ((p (async/spawn (fn () (async/sleep 100000) 42)))) \
               (async/cancel p) \
               (await p))",
        );
        let err =
            result.expect_err("uncaught await of a cancelled promise must not return a value");
        assert!(
            err.to_string().contains("cancelled"),
            "uncaught cancellation must surface as a cancellation error, got: {err}",
        );
    }

    // GATE 3: a task blocked on a LONG `async/sleep`, when cancelled, actually
    // stops at the next cooperative boundary and settles Cancelled PROMPTLY —
    // NOT after the full (100s) sleep. The promise reports `async/cancelled?`
    // and the whole evaluation completes well under the sleep duration.
    #[test]
    fn runtime_cancel_sleeping_task_stops_promptly() {
        let interp = Interpreter::new();
        let start = std::time::Instant::now();
        let result = interp
            .eval_str_via_runtime(
                "(let ((p (async/spawn (fn () (async/sleep 100000) 42)))) \
                   (async/cancel p) \
                   (try (await p) (catch e :cancelled)) \
                   (async/cancelled? p))",
            )
            .expect("a cancelled sleeping task settles through the runtime");
        let elapsed = start.elapsed();
        assert_eq!(
            result,
            Value::bool(true),
            "the cancelled task's promise must be in the Cancelled state",
        );
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "cancellation must be observed at the next cooperative boundary, \
             not after the full 100s sleep (took {elapsed:?})",
        );
    }

    // REGRESSION (critical channel-cancel hang): a spawned task parked on
    // `channel/recv` is tracked ONLY in `channel_waits`, with a wait key that is
    // never registered in `WaitRuntime::active`. Before the fix, `cancel_waiting`
    // had no `channel_waits` branch, so such a task fell through to the generic
    // fallback which returned `Ok(true)` WITHOUT waking it — leaving it Waiting
    // forever and spinning the cancel loop (an infinite hang on cancel/drop).
    // These tests are BOUNDED: a regression fails via a wall-clock assertion or a
    // CI timeout, never a silent pass.

    // GATE A: cancelling a channel-recv-parked spawned task settles it Cancelled
    // PROMPTLY; `async/cancelled?` is #t and `await` raises a catchable
    // `:cancelled` condition. Completes well under a second (no hang).
    #[test]
    fn runtime_cancel_channel_recv_parked_task_settles_cancelled() {
        let interp = Interpreter::new();
        let start = std::time::Instant::now();
        let result = interp
            .eval_str_via_runtime(
                "(let ((p (async/spawn (fn () (channel/recv (channel/new 1)))))) \
                   (async/cancel p) \
                   (list (try (await p) (catch e (:type e))) (async/cancelled? p)))",
            )
            .expect("a cancelled channel-recv-parked task settles through the runtime");
        let elapsed = start.elapsed();
        assert_eq!(
            result,
            Value::list(vec![Value::keyword("cancelled"), Value::bool(true)]),
            "await raises :cancelled and the promise is Cancelled",
        );
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "cancellation of a channel-parked task must be prompt, not a hang (took {elapsed:?})",
        );
    }

    // GATE B: THE HANG PROOF. A detached task left parked on `channel/recv` when
    // the root returns means the `Runtime` (created inside `run_exprs_via_runtime`)
    // drops with a channel-parked task still live — running
    // `close_for_interpreter_drop`'s `while cancel_waiting() == Ok(true) {}` loop.
    // Before the fix that loop never terminates, so `eval_str_via_runtime` would
    // NEVER RETURN. If this test returns at all, the drop completed cleanly. The
    // wall-clock assertion makes a partial regression fail rather than merely
    // relying on the CI timeout.
    #[test]
    fn runtime_drop_with_channel_parked_task_does_not_hang() {
        let interp = Interpreter::new();
        let start = std::time::Instant::now();
        let result = interp
            .eval_str_via_runtime("(async/spawn (fn () (channel/recv (channel/new 1)))) 42")
            .expect("root returns even though a detached task is parked on a channel");
        let elapsed = start.elapsed();
        assert_eq!(result, Value::int(42));
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "the Runtime must drop cleanly with a channel-parked task, not hang \
             in the cancel loop (took {elapsed:?})",
        );
    }

    // GATE C: a channel-SEND-parked detached task (capacity-0 channel, no
    // receiver) also cancels cleanly on drop — exercises the cancelled-blocked-
    // SENDER path (its unsent value is dropped, not leaked or double-counted).
    #[test]
    fn runtime_drop_with_channel_send_parked_task_does_not_hang() {
        let interp = Interpreter::new();
        let start = std::time::Instant::now();
        let result = interp
            .eval_str_via_runtime(
                "(async/spawn (fn () (let ((c (channel/new 1))) (channel/send c 1) (channel/send c 99)))) 7",
            )
            .expect("root returns even though a detached task is parked sending on a channel");
        let elapsed = start.elapsed();
        assert_eq!(result, Value::int(7));
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "a channel-send-parked task must cancel cleanly on drop (took {elapsed:?})",
        );
    }

    // GATE D: owned fail-fast that cancels a worker parked on `channel/recv`
    // (the semaphore is a channel) completes with the correct result and no hang.
    #[test]
    fn runtime_owned_fail_fast_cancels_channel_parked_worker() {
        let interp = Interpreter::new();
        let start = std::time::Instant::now();
        let result = interp
            .eval_str_via_runtime(
                "(begin \
                   (define ch (channel/new 1)) \
                   (define outcome \
                     (try (async/spawn-all \
                            (list (fn () (error \"boom\")) \
                                  (fn () (channel/recv ch)))) \
                          (catch e :caught))) \
                   outcome)",
            )
            .expect("owned fail-fast cancels a channel-parked sibling and settles");
        let elapsed = start.elapsed();
        assert_eq!(result, Value::keyword("caught"));
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "owned fail-fast over a channel-parked worker must not hang (took {elapsed:?})",
        );
    }

    // ── OWNED CONCURRENCY (Task 04) ───────────────────────────────────
    // Thunk-taking combinators OWN the tasks they create: on a fail-fast
    // settlement they CANCEL and reap every unfinished child before propagating.
    // Each fail-fast gate PROVES ownership by asserting a slow sibling/loser did
    // NOT run its post-sleep side effect (`set!`) — the exact opposite of the
    // observational `async/all`/`race`/`timeout` gates above, where the supplied
    // sibling always completes.

    // async/spawn-all GATE 1 — happy path: values in INPUT order.
    #[test]
    fn runtime_owned_spawn_all_returns_input_order() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime("(async/spawn-all (list (fn () 1) (fn () 2) (fn () 3)))")
            .expect("spawn-all returns input-order values");
        assert_eq!(result, lit("(list 1 2 3)"));
    }

    // async/spawn-all GATE 1b — empty input → empty list.
    #[test]
    fn runtime_owned_spawn_all_empty_is_empty_list() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime("(async/spawn-all (list))")
            .expect("spawn-all of empty input");
        assert_eq!(result, Value::list(vec![]));
    }

    // async/spawn-all GATE 2 — fail-fast OWNERSHIP: one child errors immediately;
    // the slow sibling is CANCELLED before it can run its post-sleep `set!` side
    // effect. `flag` stays 0 even after we wait past the sibling's sleep, proving
    // the sibling was reaped (contrast `runtime_async_all_failure_does_not_cancel_sibling`).
    #[test]
    fn runtime_owned_spawn_all_failure_cancels_sibling() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime(
                "(begin \
                   (define flag 0) \
                   (define outcome \
                     (try (async/spawn-all \
                            (list (fn () (async/sleep 60) (set! flag 77) 1) \
                                  (fn () (error \"boom\")))) \
                          (catch e :caught))) \
                   (async/sleep 200) \
                   (list outcome flag))",
            )
            .expect("failure cancels the owned sibling");
        assert_eq!(
            result,
            lit("(list :caught 0)"),
            "the slow sibling must be cancelled before its side effect (flag stays 0)",
        );
    }

    // async/map GATE 1 — happy path: one owned child per item, input-order results.
    #[test]
    fn runtime_owned_map_returns_input_order() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime("(async/map (fn (x) (* x 10)) (list 1 2 3))")
            .expect("async/map returns input-order results");
        assert_eq!(result, lit("(list 10 20 30)"));
    }

    // async/map GATE 2 — fail-fast OWNERSHIP: a failing item cancels the slow one.
    #[test]
    fn runtime_owned_map_failure_cancels_sibling() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime(
                "(begin \
                   (define flag 0) \
                   (define outcome \
                     (try (async/map \
                            (fn (x) (if (= x 2) (error \"boom\") \
                                        (begin (async/sleep 60) (set! flag x) x))) \
                            (list 1 2)) \
                          (catch e :caught))) \
                   (async/sleep 200) \
                   (list outcome flag))",
            )
            .expect("a failing item cancels the owned sibling");
        assert_eq!(result, lit("(list :caught 0)"));
    }

    // async/pool-map GATE 1 — bounded concurrency: at most `n` calls to `f` are
    // active at once. A shared max-observed counter over 6 items with n=2 must
    // top out at exactly 2, and results stay in INPUT order.
    #[test]
    fn runtime_owned_pool_map_bounds_concurrency_and_orders() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime(
                "(begin \
                   (define active 0) \
                   (define maxseen 0) \
                   (define (work x) \
                     (set! active (+ active 1)) \
                     (set! maxseen (max maxseen active)) \
                     (async/sleep 15) \
                     (set! active (- active 1)) \
                     (* x 10)) \
                   (define result (async/pool-map work (list 1 2 3 4 5 6) 2)) \
                   (list result maxseen))",
            )
            .expect("pool-map bounds concurrency and preserves order");
        assert_eq!(
            result,
            lit("(list (list 10 20 30 40 50 60) 2)"),
            "results in input order; at most 2 workers active at once",
        );
    }

    // async/pool-map GATE 2 — n <= 0 is an argument error (catchable condition).
    #[test]
    fn runtime_owned_pool_map_rejects_nonpositive_n() {
        let interp = Interpreter::new();
        assert_eq!(
            interp
                .eval_str_via_runtime(
                    "(try (async/pool-map (fn (x) x) (list 1 2) 0) (catch e :bad-n))",
                )
                .expect("zero concurrency is a condition, not a panic"),
            Value::keyword("bad-n"),
        );
        assert_eq!(
            interp
                .eval_str_via_runtime(
                    "(try (async/pool-map (fn (x) x) (list 1 2) -3) (catch e :bad-n))",
                )
                .expect("negative concurrency is a condition, not a panic"),
            Value::keyword("bad-n"),
        );
    }

    // async/pool-map GATE 3 — fail-fast OWNERSHIP under a bound: a failing item
    // cancels a not-yet-started (parked-on-semaphore) sibling.
    #[test]
    fn runtime_owned_pool_map_failure_cancels_pending() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime(
                "(begin \
                   (define flag 0) \
                   (define (work x) \
                     (if (= x 1) (begin (async/sleep 5) (error \"boom\")) \
                         (begin (async/sleep 60) (set! flag x) x))) \
                   (define outcome \
                     (try (async/pool-map work (list 1 2) 1) (catch e :caught))) \
                   (async/sleep 200) \
                   (list outcome flag))",
            )
            .expect("a failing worker cancels the pending owned sibling");
        assert_eq!(result, lit("(list :caught 0)"));
    }

    // async/race-owned GATE 1 — first settlement wins AND the loser is CANCELLED
    // (its post-sleep side effect never runs), contrast the observational
    // `runtime_async_race_returns_fast_and_loser_continues` where the loser does.
    #[test]
    fn runtime_owned_race_returns_winner_and_cancels_loser() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime(
                "(begin \
                   (define flag 0) \
                   (define winner \
                     (async/race-owned \
                       (list (fn () 10) \
                             (fn () (async/sleep 60) (set! flag 99) 20)))) \
                   (async/sleep 200) \
                   (list winner flag))",
            )
            .expect("race-owned returns the fast winner and cancels the loser");
        assert_eq!(
            result,
            lit("(list 10 0)"),
            "winner is 10; the slow loser is cancelled before its side effect",
        );
    }

    // async/race-owned GATE 2 — empty input is an argument error.
    #[test]
    fn runtime_owned_race_rejects_empty() {
        let interp = Interpreter::new();
        assert_eq!(
            interp
                .eval_str_via_runtime("(try (async/race-owned (list)) (catch e :empty))",)
                .expect("empty race is a condition, not a panic"),
            Value::keyword("empty"),
        );
    }

    // async/race-owned GATE 3 — the first settlement being an error re-raises it.
    #[test]
    fn runtime_owned_race_winner_error_propagates() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime(
                "(try (async/race-owned (list (fn () (error \"boom\")) \
                                              (fn () (async/sleep 100) 2))) \
                      (catch e :caught))",
            )
            .expect("a failing winner re-raises");
        assert_eq!(result, Value::keyword("caught"));
    }

    // async/with-timeout GATE 1 — the deadline wins: the slow child is CANCELLED
    // (side effect never runs) and a structured `:timeout` condition is raised.
    #[test]
    fn runtime_owned_with_timeout_cancels_slow_child() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime(
                "(begin \
                   (define flag 0) \
                   (define outcome \
                     (try (async/with-timeout 20 \
                            (fn () (async/sleep 200) (set! flag 5) :done)) \
                          (catch e (:type e)))) \
                   (async/sleep 300) \
                   (list outcome flag))",
            )
            .expect("with-timeout cancels the slow child on deadline");
        assert_eq!(
            result,
            lit("(list :timeout 0)"),
            "deadline raises :timeout and the child is cancelled before its side effect",
        );
    }

    // async/with-timeout GATE 2 — a fast child settles first: its value is preserved.
    #[test]
    fn runtime_owned_with_timeout_fast_child_returns_value() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime("(async/with-timeout 10000 (fn () (async/sleep 1) 42))")
            .expect("a child that settles before the deadline preserves its value");
        assert_eq!(result, Value::int(42));
    }

    // async/with-timeout GATE 3 — a child that errors before the deadline has its
    // failure preserved (re-raised), not masked by the timeout.
    #[test]
    fn runtime_owned_with_timeout_child_error_preserved() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime(
                "(try (async/with-timeout 10000 (fn () (error \"boom\"))) \
                      (catch e :caught))",
            )
            .expect("a fast child error is preserved");
        assert_eq!(result, Value::keyword("caught"));
    }

    // Fan-out combinators (`parallel`/`pipeline`/`parallel-settled`/
    // `pipeline-settled`) expand through `__fanout-tagged`, which spawns children
    // at bytecode level via `__spawn-apply` rather than `(map async/spawn …)`.
    // Under `eval_str_via_runtime` the outer VM runs in a runtime quantum, where a
    // `(map async/spawn …)` shape would raise "async yield outside of scheduler
    // context". Each gate asserts parity with the `eval_str` oracle.
    #[test]
    fn runtime_parallel_returns_thunk_results_in_order() {
        assert_runtime_matches_oracle("(parallel (list (fn () 1) (fn () 2) (fn () 3)))");
    }

    #[test]
    fn runtime_parallel_drops_failures_to_nil() {
        assert_runtime_matches_oracle(
            "(parallel (list (fn () 1) (fn () (throw \"boom\")) (fn () 3)))",
        );
    }

    #[test]
    fn runtime_pipeline_flows_items_through_stages() {
        assert_runtime_matches_oracle(
            "(pipeline (list 1 2 3) \
                (fn (x) (+ x 10)) \
                (fn (x) (* x 2)))",
        );
    }

    #[test]
    fn runtime_parallel_settled_preserves_ok_and_err_slots() {
        // Compare the settled shape structurally: {:ok v} slots survive verbatim,
        // {:err …} slots carry an opaque error value, so map them to :err first.
        assert_runtime_matches_oracle(
            "(map (fn (r) (if (contains? r :err) :err (:ok r))) \
                (parallel-settled (list (fn () 1) (fn () (throw \"boom\")) (fn () 3))))",
        );
    }

    #[test]
    fn runtime_pipeline_settled_preserves_ok_and_err_slots() {
        assert_runtime_matches_oracle(
            "(map (fn (r) (if (contains? r :err) :err (:ok r))) \
                (pipeline-settled (list 0 1 2) \
                  (fn (i) (if (= i 1) (throw \"boom\") i)) \
                  (fn (x) (* x 10))))",
        );
    }

    // Task 04 acceptance gate: a `map` callback that performs an ASYNC op
    // (spawn + await) must have its yield SERVICED BY THE RUNTIME — the child
    // async op genuinely parks and resumes — rather than running synchronously
    // and surfacing "async yield outside of scheduler context". `map`, when it
    // runs inside a runtime quantum, drives each callback via the
    // `NativeOutcome::Call` continuation ABI (a `MapContinuation` state machine)
    // so every callback is a fresh cooperative call that may suspend.
    #[test]
    fn runtime_map_callback_awaits_spawned_child() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime(
                "(map (fn (x) (async/await (async/spawn (fn () (* x x))))) (list 1 2 3))",
            )
            .expect("map with an async callback resolves through the runtime");
        assert_eq!(
            result,
            common_list(&[Value::int(1), Value::int(4), Value::int(9)]),
        );
    }

    // The embedding-style shape: each `async` desugars to `(async/spawn (fn ()
    // …))`, so `map` produces a list of promises which `async/all` then awaits.
    // The callback's `async/spawn` yield must be serviced cooperatively.
    #[test]
    fn runtime_async_all_over_mapped_spawns() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime("(async/all (map (fn (x) (async (* x x))) (list 1 2 3)))")
            .expect("async/all over a mapped list of spawns resolves");
        assert_eq!(
            result,
            common_list(&[Value::int(1), Value::int(4), Value::int(9)]),
        );
    }

    // Task 04 acceptance gates: `filter`/`foldl`/`reduce`/`for-each`/`sort-by`
    // drive their callback COOPERATIVELY under the runtime (a continuation state
    // machine emitting `NativeOutcome::Call`), so an async op inside the callback
    // genuinely parks and resumes instead of surfacing "async yield outside of
    // scheduler context". Each gate proves the async-callback case works AND that
    // the plain sync case stays parity with the `eval_str` oracle.

    #[test]
    fn runtime_filter_callback_awaits_spawned_child() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime(
                "(filter (fn (x) (async/await (async/spawn (fn () (> x 1))))) (list 1 2 3))",
            )
            .expect("filter with an async predicate resolves through the runtime");
        assert_eq!(result, common_list(&[Value::int(2), Value::int(3)]));
    }

    #[test]
    fn runtime_filter_sync_matches_oracle() {
        assert_runtime_matches_oracle("(filter (fn (x) (> x 1)) (list 1 2 3))");
    }

    #[test]
    fn runtime_foldl_callback_awaits_spawned_child() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime(
                "(foldl (fn (acc x) (async/await (async/spawn (fn () (+ acc x))))) 0 \
                 (list 1 2 3))",
            )
            .expect("foldl with an async combiner resolves through the runtime");
        assert_eq!(result, Value::int(6));
    }

    #[test]
    fn runtime_foldl_sync_matches_oracle() {
        assert_runtime_matches_oracle("(foldl (fn (acc x) (+ acc x)) 0 (list 1 2 3))");
    }

    #[test]
    fn runtime_foldl_empty_returns_init() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime(
                "(foldl (fn (acc x) (async/await (async/spawn (fn () (+ acc x))))) 99 (list))",
            )
            .expect("foldl over empty returns init through the runtime");
        assert_eq!(result, Value::int(99));
    }

    #[test]
    fn runtime_reduce_callback_awaits_spawned_child() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime(
                "(reduce (fn (acc x) (async/await (async/spawn (fn () (+ acc x))))) \
                 (list 1 2 3 4))",
            )
            .expect("reduce with an async combiner resolves through the runtime");
        assert_eq!(result, Value::int(10));
    }

    #[test]
    fn runtime_reduce_sync_matches_oracle() {
        assert_runtime_matches_oracle("(reduce (fn (acc x) (+ acc x)) (list 1 2 3 4))");
    }

    #[test]
    fn runtime_reduce_single_element_no_callback() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime(
                "(reduce (fn (acc x) (async/await (async/spawn (fn () (+ acc x))))) (list 42))",
            )
            .expect("reduce over a single element returns it through the runtime");
        assert_eq!(result, Value::int(42));
    }

    #[test]
    fn runtime_sort_by_callback_awaits_spawned_child() {
        let interp = Interpreter::new();
        // Sort descending by keying on the negation, computed asynchronously.
        let result = interp
            .eval_str_via_runtime(
                "(sort-by (fn (x) (async/await (async/spawn (fn () (- x))))) (list 3 1 2))",
            )
            .expect("sort-by with an async key fn resolves through the runtime");
        assert_eq!(
            result,
            common_list(&[Value::int(3), Value::int(2), Value::int(1)]),
        );
    }

    #[test]
    fn runtime_sort_by_sync_matches_oracle() {
        assert_runtime_matches_oracle("(sort-by (fn (x) (- x)) (list 3 1 2))");
    }

    // `for-each` runs a callback for its side effects; the async side effect must
    // be serviced cooperatively. Assert via a channel the callback sends into.
    #[test]
    fn runtime_for_each_callback_awaits_spawned_child() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime(
                "(define ch (channel/new 8)) \
                 (for-each (fn (x) (async/await (async/spawn (fn () (channel/send ch (* x 10)))))) \
                   (list 1 2 3)) \
                 (channel/close ch) \
                 (list (channel/recv ch) (channel/recv ch) (channel/recv ch))",
            )
            .expect("for-each with an async side effect resolves through the runtime");
        assert_eq!(
            result,
            common_list(&[Value::int(10), Value::int(20), Value::int(30)]),
        );
    }

    #[test]
    fn runtime_for_each_sync_returns_nil() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime("(for-each (fn (x) (+ x 1)) (list 1 2 3))")
            .expect("for-each returns nil through the runtime");
        assert_eq!(result, Value::nil());
    }

    // ---- Open-upvalue escape across the cooperative HOF callback ABI ----
    //
    // A runtime-quantum HOF (`for-each`/`map`) dispatches its Sema callback via
    // `NativeOutcome::Call` → `invoke_callable`, which runs the callback on a
    // FRESH callback VM. If the callback (or a closure reachable through it or
    // its arguments) captured OPEN upvalues — locals of an enclosing frame still
    // live on the HOF-invoking (parent) VM's stack — those cells point into a
    // stack that is not the callback VM's. Deref then either panics
    // ("captured variable's stack slot is not on this VM") or, worse, silently
    // reads/writes a foreign slot. The fix snapshots (closes to a SHARED,
    // still-live `Tracked` cell, mirroring `async/spawn`) every escaping open
    // upvalue against the parent VM before the callback runs, so `set!`
    // write-backs remain visible to the defining frame.

    // Shape 1: HOF callback closes over a mutable local and `set!`s it — the
    // write-back must be visible after the HOF returns.
    #[test]
    fn runtime_hof_callback_open_upvalue_shallow_write_back() {
        let program = "(let ((n 0)) (for-each (fn (x) (set! n (+ n 1))) (list 1 2 3)) n)";
        assert_runtime_matches_oracle(program);
        let interp = Interpreter::new();
        let got = interp.eval_str_via_runtime(program).expect("eval");
        assert_eq!(
            got,
            Value::int(3),
            "set! write-back must reach the captured local"
        );
    }

    // Shape 2: a callback capturing an open upvalue several frames up, plus a
    // handler that arrives as DATA (through a global map) and itself writes
    // through an open upvalue into a still-live frame. No panic; correct value.
    #[test]
    fn runtime_hof_callback_open_upvalue_deep_nesting_no_panic() {
        let program = r#"
            (begin
              (define handlers {})
              (defun reg-set! (m) (set! handlers m))
              (defun reg-emit (ev)
                (for-each (fn (entry) ((cadr entry) ev)) (map/entries handlers)))
              (define captured (list))
              (defun capture (ev) (set! captured (append captured (list ev))))
              (defun t ()
                (define local-events (list))
                (define (local-handler ev)
                  (set! local-events (append local-events (list ev))))
                (reg-set! {:a capture :b local-handler})
                (reg-emit {:msg 1})
                (set! captured captured)
                (list (length local-events) (length captured)))
              (t))
        "#;
        assert_runtime_matches_oracle(program);
        let interp = Interpreter::new();
        let got = interp.eval_str_via_runtime(program).expect("eval");
        assert_eq!(
            got,
            common_list(&[Value::int(1), Value::int(1)]),
            "both the data-carried handler and the direct capture must write back"
        );
    }

    // Shape 3: a caller-supplied closure dispatched through a HOF *wrapper* (a
    // second closure passing the callback to `for-each`). The `set!` through the
    // callback's open upvalue must flow back to the caller's local.
    #[test]
    fn runtime_hof_wrapper_open_upvalue_set_write_back() {
        let program = r#"
            (begin
              (defun hof-each (f xs) (for-each f xs))
              (let ((n 0))
                (hof-each (fn (x) (set! n (+ n 1))) (list 1 2 3))
                n))
        "#;
        assert_runtime_matches_oracle(program);
        let interp = Interpreter::new();
        let got = interp.eval_str_via_runtime(program).expect("eval");
        assert_eq!(
            got,
            Value::int(3),
            "write-back through a HOF wrapper must reach the caller's local"
        );
    }

    // Shape 4: a closure reached TRANSITIVELY by the dispatched callback (not the
    // callback itself) writes through its open upvalue. Its write must reach its
    // own slot (222) and clobber no bystander local (a/b/c stay :a/:b/:c).
    #[test]
    fn runtime_hof_transitive_closure_open_upvalue_no_slot_clobber() {
        let program = r#"
            (begin
              (defun hof-each (f xs) (for-each f xs))
              (define observed nil)
              (define (outer)
                (let ((secret 111))
                  (let ((writer (fn () (set! secret 222))))
                    (hof-each (fn (x)
                                (let ((a :a) (b :b) (c :c))
                                  (writer)
                                  (set! observed (list a b c))))
                              (list 0))
                    secret)))
              (list (outer) observed))
        "#;
        assert_runtime_matches_oracle(program);
    }

    // ── ROBUSTNESS GATE 3a: deep-recursion parity (native stack) ─────────────
    //
    // A deeply-recursive program routed through the unified runtime
    // (`eval_str_via_runtime` → `run_quantum`) must NOT consume more native
    // (Rust) stack per Sema recursion level than the legacy `eval_str`
    // (`VM::execute`) entry — otherwise a program that legacy handles gracefully
    // would SIGABRT (native stack overflow) on the runtime path. The VM's
    // `MAX_FRAMES` guard raises a graceful "stack overflow: maximum call depth
    // exceeded" before the native stack is exhausted; both entry points drive the
    // same iterative `run_inner` loop, so the runtime path must hit the SAME
    // graceful error, never a SIGABRT. These gates run on the default (small)
    // test-thread stack, so a per-level native-stack regression on the runtime
    // path surfaces as an abort here, not a silent pass on a fat main stack.

    /// Assert the runtime path produces the SAME `Result` projection as the
    /// legacy `eval_str` oracle (value on success, error string on failure) for a
    /// recursion-heavy program — proving native-stack + recursion-limit parity.
    fn assert_runtime_recursion_parity(program: &str) {
        let legacy = Interpreter::new()
            .eval_str(program)
            .map(|v| format!("{v:?}"))
            .map_err(|e| e.to_string());
        let runtime = Interpreter::new()
            .eval_str_via_runtime(program)
            .map(|v| format!("{v:?}"))
            .map_err(|e| e.to_string());
        assert_eq!(
            runtime, legacy,
            "runtime recursion result must match the legacy oracle for {program:?}"
        );
    }

    // Unbounded non-tail self recursion: the legacy path hits the graceful
    // `MAX_FRAMES` guard ("stack overflow: maximum call depth exceeded"). The
    // runtime path MUST hit the SAME graceful error, never SIGABRT.
    #[test]
    fn runtime_deep_recursion_matches_legacy_overflow_error() {
        let program = "(define (f x) (+ 1 (f x))) (f 0)";
        assert_runtime_recursion_parity(program);
        let err = Interpreter::new()
            .eval_str_via_runtime(program)
            .expect_err("unbounded non-tail recursion must fail gracefully");
        assert!(
            err.to_string().contains("maximum call depth"),
            "runtime must hit the graceful frame-limit error, got: {err}"
        );
    }

    // Deep-but-finite non-tail recursion, comfortably under the frame limit,
    // computes the same value through both entry points (no premature overflow on
    // the runtime path from extra native frames per level).
    #[test]
    fn runtime_deep_finite_recursion_matches_legacy() {
        assert_runtime_recursion_parity("(define (f n) (if (= n 0) 0 (+ 1 (f (- n 1))))) (f 1500)");
    }

    // Recursion that re-enters the evaluator through a native callback (`map`
    // dispatches `call_value` each level) — the path most likely to nest native
    // frames — still matches legacy exactly.
    #[test]
    fn runtime_native_reentry_recursion_matches_legacy() {
        assert_runtime_recursion_parity(
            "(define (build n) (if (= n 0) :done (map (fn (x) (build (- n 1))) (list 1)))) (build 300)",
        );
    }

    // ── ROBUSTNESS GATE 3b: bounded executor drop-join ───────────────────────
    //
    // Dropping an `Interpreter` (hence its `Runtime` + real `ThreadPoolExecutor`)
    // while an external-wait executor job is in flight must complete cleanly and
    // BOUNDED: no `"Resource deadlock avoided"` self-join panic, no hang, no
    // abort. A detached task blocking-`sleep`s via the executor; the root returns
    // immediately, so the executor job is genuinely in flight at drop time.
    #[test]
    fn runtime_drop_with_inflight_executor_job_is_bounded() {
        let start = std::time::Instant::now();
        let interp = Interpreter::new();
        interp
            .eval_str_via_runtime("(async/spawn (fn () (sleep 100000) 1)) 7")
            .expect("root returns while a detached blocking sleep is in flight");
        drop(interp);
        let elapsed = start.elapsed();
        // The interpreter Drop uses a 2s shutdown deadline; drop must return near
        // that bound, never wait out the 100s sleep and never hang.
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "interpreter drop with an in-flight executor job must be bounded, took {elapsed:?}"
        );
    }

    fn common_list(items: &[Value]) -> Value {
        Value::list(items.to_vec())
    }

    // ── SPAWNED-TASK OBSERVATION PARITY (full-flip family A) ──────────────────
    //
    // These gate the scheduling contract that lets a parent observe a
    // JUST-spawned child synchronously through the unified runtime.

    // A freshly spawned task is Pending until the spawner suspends: the runtime
    // resumes the spawner AHEAD of the child (`spawn_detached`), so a same-quantum
    // `async/pending?` sees Pending — not a child the runtime eagerly ran first.
    #[test]
    fn runtime_freshly_spawned_task_is_pending() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime("(async/pending? (async (+ 1 2)))")
            .expect("eval");
        assert_eq!(result, Value::bool(true));
    }

    // Cancelling a channel-parked child BEFORE it runs settles it Cancelled (the
    // deadlock detector must not misfire on a task with a pending cancellation),
    // observable synchronously via `async/cancelled?`.
    #[test]
    fn runtime_cancel_pending_channel_task_classifies_cancelled() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime(
                "(let ((ch (channel/new 1))) \
                   (let ((p (async (channel/recv ch)))) \
                     (async/cancel p) \
                     (async/cancelled? p)))",
            )
            .expect("eval");
        assert_eq!(result, Value::bool(true));
    }

    // Cancelled is neither resolved/rejected/pending — the predicates partition.
    #[test]
    fn runtime_cancelled_promise_classifies_correctly() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime(
                "(let ((p (async (async/sleep 100)))) \
                   (async/cancel p) \
                   (list (async/cancelled? p) (async/rejected? p) \
                         (async/resolved? p) (async/pending? p)))",
            )
            .expect("eval");
        assert_eq!(
            result,
            common_list(&[
                Value::bool(true),
                Value::bool(false),
                Value::bool(false),
                Value::bool(false),
            ])
        );
    }

    // A yielding native (`channel/recv`) passed DIRECTLY as a HOF callback now
    // suspends cooperatively through the structural NativeOutcome::Call
    // continuation ABI (the tool-loop/HOF migration): `(map channel/recv …)`
    // receives each value instead of raising the old lambda-wrap hint.
    #[test]
    fn runtime_yielding_native_as_hof_callback_suspends_cooperatively() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime(
                "(let ((ch (channel/new 1))) \
                   (let ((producer (async (channel/send ch 1) (channel/close ch))) \
                         (consumer (async (map channel/recv (list ch))))) \
                     (await consumer)))",
            )
            .expect("directly-passed yielding native suspends and receives");
        assert_eq!(result, Value::list(vec![Value::int(1)]));
    }

    // `async/run` inside an async task suspends cooperatively and preserves the
    // task context across the origin barrier.
    #[test]
    fn runtime_async_run_yields_and_preserves_context() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime(
                "(let ((ch (channel/new 1))) \
                   (await (async (async/run) (channel/send ch 42) (channel/recv ch))))",
            )
            .expect("eval");
        assert_eq!(result, Value::int(42));
    }

    // ── VIRTUAL-CLOCK / COOPERATIVE-YIELD ORDERING (full-flip family B) ───────

    // A 0 ms `async/timeout` must still let synchronously-ready work finish: the
    // runtime fires the (already-due) deadline timer only once ready work AND
    // pending settlements quiesce (`fire_timer` guard), so the ready child wins.
    #[test]
    fn runtime_timeout_zero_lets_ready_work_complete() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime("(async/timeout 0 (async 42))")
            .expect("eval");
        assert_eq!(result, Value::int(42));
    }

    // `retry` backoff in a runtime quantum yields cooperatively (via `async/sleep`)
    // so a sibling sleeping LESS than the backoff wakes first — the shorter timer
    // fires first because timers only advance when no task is runnable.
    #[test]
    fn runtime_retry_backoff_yields_to_shorter_sleeping_sibling() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime(
                "(let ((out (channel/new 8)) (counter 0)) \
                   (async/all \
                     (list (async/spawn (fn () \
                             (retry (fn () (set! counter (+ counter 1)) \
                                            (if (< counter 2) (error \"not yet\") counter)) \
                                    {:max-attempts 5 :base-delay-ms 40}) \
                             (channel/send out :slow))) \
                           (async/spawn (fn () (async/sleep 10) (channel/send out :fast))))) \
                   (list (channel/recv out) (channel/recv out)))",
            )
            .expect("eval");
        assert_eq!(
            result,
            common_list(&[Value::keyword("fast"), Value::keyword("slow")])
        );
    }

    // A fire-and-forget top-level `(async …)` side effect runs before the eval
    // returns (ready detached work is drained at exit), even though the spawner
    // resumes ahead of the child.
    #[test]
    fn runtime_top_level_async_side_effect_drains_at_exit() {
        let interp = Interpreter::new();
        let result = interp
            .eval_str_via_runtime(
                "(define ch (channel/new 1)) \
                 (begin (async (channel/send ch :ran)) :end)",
            )
            .expect("eval");
        assert_eq!(result, Value::keyword("end"));
        // The detached sender ran at exit, so the value is buffered and readable.
        let drained = interp
            .eval_str_via_runtime("(channel/recv ch)")
            .expect("eval");
        assert_eq!(drained, Value::keyword("ran"));
    }
}
