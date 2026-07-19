use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::{Rc, Weak};

use crate::runtime::{RootId, RuntimeId};

type OutputHook = Option<Box<dyn Fn(&str) + Send>>;

// Thread-local output hooks for capturing program stdout/stderr.
// Used by the DAP server to redirect program output into DAP `Output` events
// instead of letting it corrupt the JSON-RPC protocol stream on stdout.
thread_local! {
    static STDOUT_HOOK: RefCell<OutputHook> = RefCell::new(None);
    static STDERR_HOOK: RefCell<OutputHook> = RefCell::new(None);
}

/// Set the thread-local stdout capture hook.
/// Pass `None` to clear.
pub fn set_stdout_hook(hook: OutputHook) {
    STDOUT_HOOK.with(|cell| *cell.borrow_mut() = hook);
}

/// Set the thread-local stderr capture hook.
/// Pass `None` to clear.
pub fn set_stderr_hook(hook: OutputHook) {
    STDERR_HOOK.with(|cell| *cell.borrow_mut() = hook);
}

/// One line of program output captured for a root that opted into
/// [`RootOptions::capture_output`](../../sema_eval/struct.RootOptions.html)
/// instead of inheriting process stdout/stderr. Produced by
/// [`write_stdout`]/[`write_stderr`] when the currently-running task's root
/// is registered via [`mark_root_capturing`], drained by
/// `Runtime::take_captured_output`.
#[derive(Clone, Debug)]
pub struct CapturedOutput {
    pub root: RootId,
    pub is_stderr: bool,
    pub text: String,
}

thread_local! {
    // Each runtime owns its output buffer. Routes are weak so this hook cannot
    // keep a dropped runtime alive when its explicit teardown is bypassed.
    static OUTPUT_CAPTURE_ROUTES: RefCell<HashMap<RuntimeId, Weak<RefCell<Vec<CapturedOutput>>>>> =
        RefCell::new(HashMap::new());
    // Roots currently opted into capture. A root is added here at submission
    // (`capture_output: true`) and removed when it is reaped, so this never
    // grows unbounded across a long-running host (REPL, notebook server).
    static CAPTURING_ROOTS: RefCell<HashSet<RootId>> = RefCell::new(HashSet::new());
    // Mirrors `CAPTURING_ROOTS.len()` as a plain counter so the print hot
    // path can skip the hash-set lookup entirely with one `Cell` read when
    // no root on this thread is capturing (the overwhelmingly common case —
    // `capture_output` defaults to `false`).
    static CAPTURING_COUNT: Cell<usize> = const { Cell::new(0) };
    // The root of the task currently executing a quantum on this thread, set
    // by the runtime around every VM step (mirrors `CURRENT_TASK_ID`).
    static CURRENT_ROOT: Cell<Option<RootId>> = const { Cell::new(None) };
}

/// Register the buffer owned by `runtime_id`. Other live runtime routes and
/// capturing-root markers on this thread remain intact.
pub fn register_output_capture_sink(
    runtime_id: RuntimeId,
    sink: &Rc<RefCell<Vec<CapturedOutput>>>,
) {
    OUTPUT_CAPTURE_ROUTES.with(|routes| {
        routes.borrow_mut().insert(runtime_id, Rc::downgrade(sink));
    });
}

/// Remove one runtime's output route and any abandoned capturing-root markers
/// it minted. Teardown is scoped by the full runtime identity so another live
/// runtime on the same thread is unaffected.
pub fn unregister_output_capture_sink(runtime_id: RuntimeId) {
    OUTPUT_CAPTURE_ROUTES.with(|routes| {
        routes.borrow_mut().remove(&runtime_id);
    });
    let remaining = CAPTURING_ROOTS.with(|roots| {
        let mut roots = roots.borrow_mut();
        roots.retain(|root| root.runtime() != runtime_id);
        roots.len()
    });
    CAPTURING_COUNT.with(|count| count.set(remaining));
}

/// Test/introspection accessor for `CAPTURING_COUNT` — lets a white-box
/// test (in another crate, so it can't reach the thread-local directly)
/// assert the fast-path counter is clean after runtime teardown. Not
/// `cfg(test)`: integration tests in downstream crates build this crate without
/// the library's own `test` cfg, so a `cfg(test)`-gated item here would be
/// invisible to them.
#[doc(hidden)]
pub fn capturing_root_count() -> usize {
    CAPTURING_COUNT.with(Cell::get)
}

/// Mark `root` as capturing its output instead of inheriting process
/// stdout/stderr. Idempotent.
pub fn mark_root_capturing(root: RootId) {
    CAPTURING_ROOTS.with(|set| {
        if set.borrow_mut().insert(root) {
            CAPTURING_COUNT.with(|c| c.set(c.get() + 1));
        }
    });
}

/// Stop capturing `root`'s output — called when a root is reaped, so the
/// capturing set never accumulates dead entries. Idempotent.
pub fn unmark_root_capturing(root: RootId) {
    CAPTURING_ROOTS.with(|set| {
        if set.borrow_mut().remove(&root) {
            CAPTURING_COUNT.with(|c| c.set(c.get().saturating_sub(1)));
        }
    });
}

/// Publish `root` as the currently-executing quantum's root, returning the
/// displaced value so the caller can restore it on quantum exit (mirrors
/// [`crate::set_current_task_id`]).
pub fn set_current_root(root: Option<RootId>) -> Option<RootId> {
    CURRENT_ROOT.with(|cell| cell.replace(root))
}

/// Append to the capture sink if the current quantum's root is capturing.
/// Returns `true` if the text was captured (caller must not also print it).
/// The `CAPTURING_COUNT == 0` check is a single cheap `Cell` read that keeps
/// this a no-op branch for the default (non-capturing) path — no hash-set
/// lookup, no allocation, unless at least one root on this thread actually
/// opted in.
fn try_capture(is_stderr: bool, s: &str) -> bool {
    if CAPTURING_COUNT.with(Cell::get) == 0 {
        return false;
    }
    let Some(root) = CURRENT_ROOT.with(Cell::get) else {
        return false;
    };
    if !CAPTURING_ROOTS.with(|set| set.borrow().contains(&root)) {
        return false;
    }
    let route = OUTPUT_CAPTURE_ROUTES.with(|routes| routes.borrow().get(&root.runtime()).cloned());
    let Some(route) = route else {
        return false;
    };
    let Some(sink) = route.upgrade() else {
        unregister_output_capture_sink(root.runtime());
        return false;
    };
    sink.borrow_mut().push(CapturedOutput {
        root,
        is_stderr,
        text: s.to_string(),
    });
    true
}

#[cfg(test)]
fn output_capture_route_count() -> usize {
    OUTPUT_CAPTURE_ROUTES.with(|routes| routes.borrow().len())
}

/// Write a string to stdout: captured for the current quantum's root if it
/// opted into `capture_output`, otherwise through the DAP hook (if set) or
/// via `print!`, exactly as before capture existed.
pub fn write_stdout(s: &str) {
    if try_capture(false, s) {
        return;
    }
    STDOUT_HOOK.with(|cell| {
        if let Some(hook) = cell.borrow().as_ref() {
            hook(s);
        } else {
            print!("{}", s);
        }
    });
}

/// Write a string to stderr: captured for the current quantum's root if it
/// opted into `capture_output`, otherwise through the DAP hook (if set) or
/// via `eprint!`, exactly as before capture existed.
pub fn write_stderr(s: &str) {
    if try_capture(true, s) {
        return;
    }
    STDERR_HOOK.with(|cell| {
        if let Some(hook) = cell.borrow().as_ref() {
            hook(s);
        } else {
            eprint!("{}", s);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::{RootId, RuntimeId, RuntimeScopedIdCounter};

    fn runtime_and_root() -> (RuntimeId, RootId) {
        let runtime = RuntimeId::allocate().expect("runtime identity available");
        let root = RuntimeScopedIdCounter::<RootId>::new(runtime)
            .allocate()
            .expect("root identity available");
        (runtime, root)
    }

    #[test]
    fn capture_routes_equal_local_roots_to_their_runtime_sinks() {
        let (runtime_a, root_a) = runtime_and_root();
        let (runtime_b, root_b) = runtime_and_root();
        assert_eq!(root_a.local(), root_b.local());

        let sink_a = Rc::new(RefCell::new(Vec::new()));
        let sink_b = Rc::new(RefCell::new(Vec::new()));
        register_output_capture_sink(runtime_a, &sink_a);
        register_output_capture_sink(runtime_b, &sink_b);
        mark_root_capturing(root_a);
        mark_root_capturing(root_b);

        set_current_root(Some(root_a));
        write_stdout("A-only");
        set_current_root(Some(root_b));
        write_stdout("B-only");
        set_current_root(None);

        let events_a = sink_a.borrow();
        assert!(matches!(
            events_a.as_slice(),
            [CapturedOutput { root, is_stderr: false, text }]
                if *root == root_a && text == "A-only"
        ));
        let events_b = sink_b.borrow();
        assert!(matches!(
            events_b.as_slice(),
            [CapturedOutput { root, is_stderr: false, text }]
                if *root == root_b && text == "B-only"
        ));

        unregister_output_capture_sink(runtime_a);
        unregister_output_capture_sink(runtime_b);
    }

    #[test]
    fn unregister_and_dead_weak_pruning_are_scoped_to_one_runtime() {
        let (runtime_a, root_a) = runtime_and_root();
        let (runtime_b, root_b) = runtime_and_root();
        let sink_a = Rc::new(RefCell::new(Vec::new()));
        let sink_b = Rc::new(RefCell::new(Vec::new()));
        register_output_capture_sink(runtime_a, &sink_a);
        register_output_capture_sink(runtime_b, &sink_b);
        mark_root_capturing(root_a);
        mark_root_capturing(root_b);
        assert_eq!(capturing_root_count(), 2);
        assert_eq!(output_capture_route_count(), 2);

        unregister_output_capture_sink(runtime_a);
        assert_eq!(capturing_root_count(), 1);
        assert_eq!(output_capture_route_count(), 1);

        drop(sink_b);
        set_stdout_hook(Some(Box::new(|_| {})));
        set_current_root(Some(root_b));
        write_stdout("dead route falls through");
        set_current_root(None);

        assert_eq!(capturing_root_count(), 0);
        assert_eq!(output_capture_route_count(), 0);
        set_stdout_hook(None);
    }
}
