//! Debug-session awareness for dynamically loaded code.
//!
//! Code pulled in via `(load ...)` / `(import ...)` is evaluated on a
//! fresh VM instance, which does not participate in the current VM's debug loop. As a result,
//! breakpoints set in dynamically loaded/imported files never hit, silently.
//!
//! To avoid that being a silent surprise, the DAP server marks a debug session
//! as active for the duration of a debugged run. When `load`/`import` runs while
//! a session is active, the evaluator emits a single, clear warning (routed
//! through `sema_core::write_stderr`, which the DAP server captures into an
//! `Output` event) noting the limitation. The warning fires at most once per
//! session so a program that loads many files does not spam the debug console.

use std::cell::Cell;

thread_local! {
    /// Whether a VM debug session is currently active on this thread.
    static DEBUG_SESSION_ACTIVE: Cell<bool> = const { Cell::new(false) };
    /// Whether the "loaded/imported code bypasses the debugger" warning has
    /// already been emitted for the current session.
    static WARNED_LOAD_BYPASS: Cell<bool> = const { Cell::new(false) };
}

/// Mark a debug session as active or inactive on the current thread.
///
/// Setting this to `true` also resets the one-time warning latch so the warning
/// can fire once per session. The DAP server calls this around `execute_debug`.
pub fn set_debug_session_active(active: bool) {
    DEBUG_SESSION_ACTIVE.with(|c| c.set(active));
    if active {
        WARNED_LOAD_BYPASS.with(|c| c.set(false));
    }
}

/// Whether a debug session is active on the current thread.
pub fn is_debug_session_active() -> bool {
    DEBUG_SESSION_ACTIVE.with(|c| c.get())
}

/// Emit the "dynamically loaded/imported code bypasses the debugger" warning,
/// but only when a debug session is active and only once per session.
///
/// `form` is the special-form name (`"load"` or `"import"`) and `path` is the
/// path being loaded, used to make the message actionable.
pub fn warn_load_bypass_once(form: &str, path: &str) {
    if !is_debug_session_active() {
        return;
    }
    let already = WARNED_LOAD_BYPASS.with(|c| c.replace(true));
    if already {
        return;
    }
    sema_core::write_stderr(&format!(
        "Debugger: code reached via ({form} \"{path}\") is not stepped by the \
         debugger (it runs outside the attached debug session), so breakpoints \
         set in dynamically loaded or imported files are not hit. Stepping the \
         main program is unaffected. (This warning is shown once per debug \
         session.)\n"
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// Capture stderr-hook output for the duration of `f`.
    fn capture_stderr(f: impl FnOnce()) -> String {
        let buf = Arc::new(Mutex::new(String::new()));
        let buf_hook = buf.clone();
        sema_core::set_host_stderr_hook(Some(Box::new(move |s: &str| {
            buf_hook.lock().unwrap().push_str(s);
        })));
        f();
        sema_core::set_host_stderr_hook(None);
        let out = buf.lock().unwrap().clone();
        out
    }

    #[test]
    fn no_warning_when_session_inactive() {
        set_debug_session_active(false);
        let out = capture_stderr(|| {
            warn_load_bypass_once("load", "helpers.sema");
        });
        assert!(out.is_empty(), "should not warn outside a debug session");
    }

    #[test]
    fn warns_once_per_session() {
        set_debug_session_active(true);
        let out = capture_stderr(|| {
            warn_load_bypass_once("load", "helpers.sema");
            warn_load_bypass_once("import", "other.sema");
        });
        assert!(out.contains("not stepped by the debugger"));
        assert!(out.contains("helpers.sema"));
        // Second call in the same session is suppressed.
        assert!(!out.contains("other.sema"));
        // Re-activating the session resets the latch so a new run warns again.
        set_debug_session_active(true);
        let out2 = capture_stderr(|| {
            warn_load_bypass_once("import", "again.sema");
        });
        assert!(out2.contains("again.sema"));
        set_debug_session_active(false);
    }
}
