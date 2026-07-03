use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::time::Instant;

use crate::{CallFrame, Env, Sandbox, SemaError, Span, SpanMap, StackTrace, Value};

const MAX_SPAN_TABLE_ENTRIES: usize = 200_000;

/// Function-pointer type for the full evaluator callback: (ctx, expr, env) -> Result<Value, SemaError>
pub type EvalCallbackFn = fn(&EvalContext, &Value, &Env) -> Result<Value, SemaError>;

/// Function-pointer type for calling a function value with evaluated arguments: (ctx, func, args) -> Result<Value, SemaError>
pub type CallCallbackFn = fn(&EvalContext, &Value, &[Value]) -> Result<Value, SemaError>;

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
    /// Optional wall-clock deadline for evaluation. When set, both the
    /// tree-walker and the bytecode VM periodically check whether the current
    /// time has passed this instant and, if so, abort with an error. Used by
    /// the notebook engine to bound how long a single cell evaluation can run.
    pub eval_deadline: Cell<Option<Instant>>,
    pub sandbox: Sandbox,
    pub user_context: RefCell<Vec<BTreeMap<Value, Value>>>,
    pub hidden_context: RefCell<Vec<BTreeMap<Value, Value>>>,
    pub context_stacks: RefCell<BTreeMap<Value, Vec<Value>>>,
    pub eval_fn: Cell<Option<EvalCallbackFn>>,
    pub call_fn: Cell<Option<CallCallbackFn>>,
    pub interactive: Cell<bool>,
}

/// RAII guard for a module-load scope: pops the load stack when dropped, so the
/// stack stays balanced on every exit path (early return, `?`, panic). Created
/// by [`EvalContext::enter_module_load`].
pub struct ModuleLoadGuard<'a> {
    ctx: &'a EvalContext,
    path: PathBuf,
}

impl Drop for ModuleLoadGuard<'_> {
    fn drop(&mut self) {
        self.ctx.end_module_load(&self.path);
    }
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
            context_stacks: RefCell::new(BTreeMap::new()),
            eval_fn: Cell::new(None),
            call_fn: Cell::new(None),
            interactive: Cell::new(false),
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
            context_stacks: RefCell::new(BTreeMap::new()),
            eval_fn: Cell::new(None),
            call_fn: Cell::new(None),
            interactive: Cell::new(false),
        }
    }

    pub fn push_file_path(&self, path: PathBuf) {
        self.current_file.borrow_mut().push(path);
    }

    pub fn pop_file_path(&self) {
        self.current_file.borrow_mut().pop();
    }

    pub fn current_file_dir(&self) -> Option<PathBuf> {
        self.current_file
            .borrow()
            .last()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
    }

    pub fn current_file_path(&self) -> Option<PathBuf> {
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
        let mut stack = self.module_exports.borrow_mut();
        if let Some(top) = stack.last_mut() {
            *top = Some(names);
        }
    }

    pub fn clear_module_exports(&self) {
        self.module_exports.borrow_mut().push(None);
    }

    pub fn take_module_exports(&self) -> Option<Vec<String>> {
        self.module_exports.borrow_mut().pop().flatten()
    }

    /// Enter a module-load scope, guarding against import/load cycles. The
    /// returned [`ModuleLoadGuard`] pops the load stack when dropped, keeping it
    /// balanced on any exit path. Errors if `path` is already being loaded.
    pub fn enter_module_load(&self, path: PathBuf) -> Result<ModuleLoadGuard<'_>, SemaError> {
        self.begin_module_load(&path)?;
        Ok(ModuleLoadGuard { ctx: self, path })
    }

    fn begin_module_load(&self, path: &PathBuf) -> Result<(), SemaError> {
        let mut stack = self.module_load_stack.borrow_mut();
        if let Some(pos) = stack.iter().position(|p| p == path) {
            let mut cycle: Vec<String> = stack[pos..]
                .iter()
                .map(|p| p.display().to_string())
                .collect();
            cycle.push(path.display().to_string());
            return Err(SemaError::eval(format!(
                "cyclic import detected: {}",
                cycle.join(" -> ")
            )));
        }
        stack.push(path.clone());
        Ok(())
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
        let frames = self.user_context.borrow();
        for frame in frames.iter().rev() {
            if let Some(v) = frame.get(key) {
                return Some(v.clone());
            }
        }
        None
    }

    pub fn context_set(&self, key: Value, value: Value) {
        let mut frames = self.user_context.borrow_mut();
        if let Some(top) = frames.last_mut() {
            top.insert(key, value);
        }
    }

    pub fn context_has(&self, key: &Value) -> bool {
        let frames = self.user_context.borrow();
        frames.iter().any(|frame| frame.contains_key(key))
    }

    pub fn context_remove(&self, key: &Value) -> Option<Value> {
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
        self.user_context.borrow_mut().push(BTreeMap::new());
    }

    pub fn context_push_frame_with(&self, bindings: BTreeMap<Value, Value>) {
        self.user_context.borrow_mut().push(bindings);
    }

    pub fn context_pop_frame(&self) {
        let mut frames = self.user_context.borrow_mut();
        if frames.len() > 1 {
            frames.pop();
        }
    }

    pub fn context_clear(&self) {
        let mut frames = self.user_context.borrow_mut();
        frames.clear();
        frames.push(BTreeMap::new());
    }

    // --- Hidden context methods ---

    pub fn hidden_get(&self, key: &Value) -> Option<Value> {
        let frames = self.hidden_context.borrow();
        for frame in frames.iter().rev() {
            if let Some(v) = frame.get(key) {
                return Some(v.clone());
            }
        }
        None
    }

    pub fn hidden_set(&self, key: Value, value: Value) {
        let mut frames = self.hidden_context.borrow_mut();
        if let Some(top) = frames.last_mut() {
            top.insert(key, value);
        }
    }

    pub fn hidden_has(&self, key: &Value) -> bool {
        let frames = self.hidden_context.borrow();
        frames.iter().any(|frame| frame.contains_key(key))
    }

    pub fn hidden_push_frame(&self) {
        self.hidden_context.borrow_mut().push(BTreeMap::new());
    }

    pub fn hidden_pop_frame(&self) {
        let mut frames = self.hidden_context.borrow_mut();
        if frames.len() > 1 {
            frames.pop();
        }
    }

    // --- Stack methods ---

    pub fn context_stack_push(&self, key: Value, value: Value) {
        self.context_stacks
            .borrow_mut()
            .entry(key)
            .or_default()
            .push(value);
    }

    pub fn context_stack_get(&self, key: &Value) -> Vec<Value> {
        self.context_stacks
            .borrow()
            .get(key)
            .cloned()
            .unwrap_or_default()
    }

    pub fn context_stack_pop(&self, key: &Value) -> Option<Value> {
        let mut stacks = self.context_stacks.borrow_mut();
        let stack = stacks.get_mut(key)?;
        let val = stack.pop();
        if stack.is_empty() {
            stacks.remove(key);
        }
        val
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
