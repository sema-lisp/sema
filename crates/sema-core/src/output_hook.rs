use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::{Rc, Weak};

use crate::runtime::{RootId, RuntimeId};

type OutputHook = Option<Box<dyn Fn(&str) + Send>>;

// HOST-ADAPTER-ONLY fallback output hooks (C05 / Commit C1).
//
// These thread-local hooks let a HOST adapter keep program stdout/stderr off the
// process's real streams: the DAP server redirects program output into `Output`
// events so prints don't corrupt the JSON-RPC stream, the MCP `eval_with_capture`
// buffers it out of the protocol stream, the wasm host installs inert no-op
// sinks (a real `print!` is an unsupported syscall on wasm32), and the
// debug-session tests capture stderr. They are the fallback taken only when the
// currently running root did NOT opt into root-tagged capture
// (`OUTPUT_CAPTURE_ROUTES`, below) — the runtime-tagged path is unaffected.
//
// Contract (enforced): a hook is a non-suspending `Fn(&str)` — it MUST NOT block
// or structurally suspend, because it runs an arbitrary host closure on the VM
// thread inside a quantum where there is no runtime wait to yield to. A hook is
// free to print, but a hook that itself calls `write_stdout`/`write_stderr`
// re-enters as a PASS-THROUGH (a direct `print!`/`eprint!`), never an unbounded
// recursion: `IN_HOST_OUTPUT_HOOK` latches the thread for the duration of one
// hook invocation. Installation is HOST-ADAPTER-ONLY — the `set_host_*` naming
// and the `HOST_OUTPUT_HOOK` source-policy allowlist pin every install site.
thread_local! {
    static HOST_STDOUT_HOOK: RefCell<OutputHook> = RefCell::new(None);
    static HOST_STDERR_HOOK: RefCell<OutputHook> = RefCell::new(None);
    // Latched for the duration of one host output-hook invocation on this
    // thread so a hook that itself prints passes straight through instead of
    // re-entering the hook (unbounded recursion). Shared across stdout/stderr so
    // a cross-stream print inside a hook (a stdout hook that writes stderr) also
    // passes through.
    static IN_HOST_OUTPUT_HOOK: Cell<bool> = const { Cell::new(false) };
}

/// Install the thread-local HOST-ADAPTER-ONLY stdout capture hook; `None` clears.
///
/// HOST-ADAPTER-ONLY: only a host embedding (DAP/MCP/wasm/debug-session) that
/// owns the process may install this. The hook must be a non-suspending
/// `Fn(&str)` (see the module contract above); a hook that prints re-enters as a
/// pass-through via the re-entrancy latch, never recursion. Runtime code routes
/// output through root-tagged capture, never this fallback.
pub fn set_host_stdout_hook(hook: OutputHook) {
    HOST_STDOUT_HOOK.with(|cell| *cell.borrow_mut() = hook);
}

/// Install the thread-local HOST-ADAPTER-ONLY stderr capture hook; `None` clears.
///
/// See [`set_host_stdout_hook`] for the HOST-ADAPTER-ONLY / non-suspending
/// contract and the re-entrancy latch.
pub fn set_host_stderr_hook(hook: OutputHook) {
    HOST_STDERR_HOOK.with(|cell| *cell.borrow_mut() = hook);
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

/// Return the root published for the currently executing runtime quantum.
/// Every runtime-driven VM quantum, including a root's main task, publishes
/// this identity; host and ordinary compiled evaluation return `None`.
pub fn current_root() -> Option<RootId> {
    CURRENT_ROOT.with(Cell::get)
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

/// RAII latch that closes the host output-hook re-entrancy hole. [`enter`] hands
/// back `Some` only for the outermost hook invocation on this thread; while the
/// guard is held, a hook that calls `write_stdout`/`write_stderr` again sees
/// `None` and passes straight through. `Drop` clears the latch even if the hook
/// panics, so a panicking hook can't wedge the thread into permanent pass-through.
///
/// [`enter`]: HostHookGuard::enter
struct HostHookGuard;

impl HostHookGuard {
    fn enter() -> Option<Self> {
        IN_HOST_OUTPUT_HOOK.with(|latched| {
            if latched.get() {
                None
            } else {
                latched.set(true);
                Some(HostHookGuard)
            }
        })
    }
}

impl Drop for HostHookGuard {
    fn drop(&mut self) {
        IN_HOST_OUTPUT_HOOK.with(|latched| latched.set(false));
    }
}

/// Write a string to stdout: captured for the current quantum's root if it
/// opted into `capture_output`, otherwise routed through the HOST-ADAPTER-ONLY
/// fallback hook (if a host installed one) or via `print!`. A hook that itself
/// prints re-enters through the latch as a direct `print!`, never recursion.
pub fn write_stdout(s: &str) {
    if try_capture(false, s) {
        return;
    }
    match HostHookGuard::enter() {
        Some(_guard) => HOST_STDOUT_HOOK.with(|cell| {
            if let Some(hook) = cell.borrow().as_ref() {
                hook(s);
            } else {
                print!("{}", s);
            }
        }),
        // Re-entrant call from inside a running hook: pass through directly.
        None => print!("{}", s),
    }
}

/// Write a string to stderr: mirrors [`write_stdout`] — root capture first, then
/// the HOST-ADAPTER-ONLY fallback hook or `eprint!`, guarded by the same
/// re-entrancy latch so a hook that prints passes through instead of recursing.
pub fn write_stderr(s: &str) {
    if try_capture(true, s) {
        return;
    }
    match HostHookGuard::enter() {
        Some(_guard) => HOST_STDERR_HOOK.with(|cell| {
            if let Some(hook) = cell.borrow().as_ref() {
                hook(s);
            } else {
                eprint!("{}", s);
            }
        }),
        // Re-entrant call from inside a running hook: pass through directly.
        None => eprint!("{}", s),
    }
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
        set_host_stdout_hook(Some(Box::new(|_| {})));
        set_current_root(Some(root_b));
        write_stdout("dead route falls through");
        set_current_root(None);

        assert_eq!(capturing_root_count(), 0);
        assert_eq!(output_capture_route_count(), 0);
        set_host_stdout_hook(None);
    }

    #[test]
    fn reentrant_host_hook_passes_through_without_recursion() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        // A host hook that itself prints. Without the re-entrancy latch its
        // nested `write_stdout` would call the hook again — unbounded recursion
        // (a stack overflow that aborts the process). With the latch, the nested
        // write passes straight through, so the hook runs exactly once and the
        // test returns.
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_hook = calls.clone();
        set_host_stdout_hook(Some(Box::new(move |_s: &str| {
            calls_hook.fetch_add(1, Ordering::SeqCst);
            write_stdout("nested-from-hook");
        })));

        write_stdout("outer");
        set_host_stdout_hook(None);

        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn host_hook_delivers_output_during_a_quantum() {
        use std::sync::{Arc, Mutex};

        // A DAP/MCP-style capture hook: a current root is published (a runtime
        // quantum is active) but it did not opt into root-tagged capture, so
        // output must still reach the host fallback hook.
        let buf = Arc::new(Mutex::new(String::new()));
        let sink = buf.clone();
        set_host_stdout_hook(Some(Box::new(move |s: &str| {
            sink.lock().unwrap().push_str(s);
        })));

        let (_runtime, root) = runtime_and_root();
        set_current_root(Some(root));
        write_stdout("hello ");
        write_stdout("world");
        set_current_root(None);
        set_host_stdout_hook(None);

        assert_eq!(buf.lock().unwrap().as_str(), "hello world");
    }
}
