use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;

use crate::runtime::RootId;

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
    // The runtime's shared per-quantum output sink, installed once at
    // `Runtime::new`. `None` until a runtime exists on this thread.
    static OUTPUT_CAPTURE_SINK: RefCell<Option<Rc<RefCell<Vec<CapturedOutput>>>>> =
        const { RefCell::new(None) };
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

/// Install (or replace) the shared buffer that captured output is appended
/// to. Called once per `Runtime::new`.
pub fn install_output_capture_sink(sink: Rc<RefCell<Vec<CapturedOutput>>>) {
    OUTPUT_CAPTURE_SINK.with(|cell| *cell.borrow_mut() = Some(sink));
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
    OUTPUT_CAPTURE_SINK.with(|cell| {
        let Some(sink) = cell.borrow().clone() else {
            return false;
        };
        sink.borrow_mut().push(CapturedOutput {
            root,
            is_stderr,
            text: s.to_string(),
        });
        true
    })
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
