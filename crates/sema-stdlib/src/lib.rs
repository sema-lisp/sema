#![allow(clippy::mutable_key_type, clippy::cloned_ref_to_slice_refs)]
#[cfg(not(target_arch = "wasm32"))]
mod archive;
mod arithmetic;
mod async_ops;
mod bitwise;
mod bytevector;
mod comparison;
mod context;
mod crypto;
mod csv_ops;
mod datetime;
mod diff;
#[cfg(not(target_arch = "wasm32"))]
mod event;
#[cfg(not(target_arch = "wasm32"))]
mod fs_watch;
#[cfg(not(target_arch = "wasm32"))]
mod git;
#[cfg(not(target_arch = "wasm32"))]
mod http;
#[cfg(not(target_arch = "wasm32"))]
mod io;
/// Test/observation hooks for the canonical quarantined-bounded file operations
/// (Task 05 R08A): the in-flight overlap gauge and the pre-dispatch cap/delay
/// knobs used by the resource-contract tests.
#[cfg(not(target_arch = "wasm32"))]
pub use io::{
    fs_peak_inflight, reset_fs_inflight, set_fs_byte_cap, set_fs_list_cap, set_fs_test_delay_ms,
    FS_BYTE_CAP_DEFAULT, FS_LIST_CAP_DEFAULT,
};
pub(crate) mod json;
#[cfg(not(target_arch = "wasm32"))]
mod kv;
mod list;
mod map;
#[cfg(not(target_arch = "wasm32"))]
mod markup;
mod math;
mod meta;
mod mutable;
mod otel;
#[cfg(not(target_arch = "wasm32"))]
mod pdf;
mod pio;
mod predicates;
#[cfg(not(target_arch = "wasm32"))]
mod proc;
#[cfg(not(target_arch = "wasm32"))]
mod pty;
mod reflect;
mod regex_ops;
/// Shared reference glue for offloading an I/O op onto the unified-runtime
/// executor as a structural `NativeOutcome::Suspend` (http, git, sqlite, …).
#[cfg(not(target_arch = "wasm32"))]
mod runtime_offload;
mod secret;
#[cfg(not(target_arch = "wasm32"))]
mod serial;
#[cfg(not(target_arch = "wasm32"))]
mod server;
#[cfg(not(target_arch = "wasm32"))]
mod sqlite;
mod stream;
mod string;
#[cfg(not(target_arch = "wasm32"))]
mod system;
#[cfg(all(not(target_arch = "wasm32"), unix))]
#[doc(hidden)]
pub use system::mark_sigwinch_pending_for_test;
#[cfg(not(target_arch = "wasm32"))]
mod terminal;
mod text;
mod toml_ops;
mod typed_array;
#[cfg(not(target_arch = "wasm32"))]
mod workflow;
#[cfg(not(target_arch = "wasm32"))]
pub mod workflow_check;
#[cfg(not(target_arch = "wasm32"))]
pub mod workflow_mcp;
#[cfg(not(target_arch = "wasm32"))]
mod ws;

#[cfg(not(target_arch = "wasm32"))]
use sema_core::Caps;
use sema_core::{Env, Sandbox, Value};

/// Strip ANSI escape sequences from `s`: full CSI (`ESC[ … final-byte`), OSC
/// (`ESC] … BEL|ST`), and other two-char escapes. Shared by `term/strip`,
/// `string/width`, and `string/wrap` so display-width math ignores styling.
pub(crate) fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\x1b' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            // CSI: ESC [ (params/intermediates) final-byte in 0x40..=0x7E.
            Some('[') => {
                for inner in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&inner) {
                        break;
                    }
                }
            }
            // OSC: ESC ] … terminated by BEL (0x07) or ST (ESC \).
            Some(']') => {
                while let Some(inner) = chars.next() {
                    if inner == '\x07' {
                        break;
                    }
                    if inner == '\x1b' {
                        if chars.peek() == Some(&'\\') {
                            chars.next();
                        }
                        break;
                    }
                }
            }
            // Other two-char escapes (ESC 7, ESC 8, …): drop the byte after ESC.
            _ => {}
        }
    }
    out
}

pub fn register_stdlib(env: &Env, sandbox: &Sandbox) {
    #[cfg(target_arch = "wasm32")]
    let _ = sandbox;

    // Install THE process-wide I/O pool behind the sema-core executor seam
    // (ADR #69) so every offloading builtin below reaches one pool. Idempotent.
    #[cfg(not(target_arch = "wasm32"))]
    sema_io::install();

    arithmetic::register(env);
    comparison::register(env);
    context::register(env);
    list::register(env);
    string::register(env);
    predicates::register(env);
    map::register(env);
    #[cfg(not(target_arch = "wasm32"))]
    io::register(env, sandbox);
    math::register(env);
    #[cfg(not(target_arch = "wasm32"))]
    system::register(env, sandbox);
    json::register(env);
    toml_ops::register(env);
    meta::register(env);
    otel::register(env);
    regex_ops::register(env);
    #[cfg(not(target_arch = "wasm32"))]
    http::register(env, sandbox);
    #[cfg(not(target_arch = "wasm32"))]
    server::register(env, sandbox);
    #[cfg(not(target_arch = "wasm32"))]
    ws::register(env, sandbox);
    bitwise::register(env);
    crypto::register(env);
    mutable::register(env);
    datetime::register(env);
    csv_ops::register(env);
    bytevector::register(env);
    #[cfg(not(target_arch = "wasm32"))]
    terminal::register(env);
    text::register(env);
    stream::register(env);
    pio::register(env);
    typed_array::register(env);
    // Agent/TUI host primitives (issue #53, wave 2)
    secret::register(env);
    diff::register(env, sandbox);
    reflect::register(env, sandbox);
    #[cfg(not(target_arch = "wasm32"))]
    {
        proc::register(env, sandbox);
        pty::register(env, sandbox);
        event::register(env);
        git::register(env, sandbox);
        archive::register(env, sandbox);
        markup::register(env);
        fs_watch::register(env, sandbox);
    }
    #[cfg(not(target_arch = "wasm32"))]
    workflow::register(env);
    async_ops::register(env);
    #[cfg(not(target_arch = "wasm32"))]
    stream::register_io(env, sandbox);
    #[cfg(not(target_arch = "wasm32"))]
    kv::register(env, sandbox);
    #[cfg(not(target_arch = "wasm32"))]
    pdf::register(env, sandbox);
    #[cfg(not(target_arch = "wasm32"))]
    sqlite::register(env, sandbox);
    #[cfg(not(target_arch = "wasm32"))]
    serial::register(env, sandbox);
}

#[cfg(not(target_arch = "wasm32"))]
fn register_fn_gated(
    env: &Env,
    sandbox: &Sandbox,
    cap: Caps,
    name: &str,
    f: impl Fn(&[Value]) -> Result<Value, sema_core::SemaError> + 'static,
) {
    if sandbox.is_unrestricted() {
        register_fn(env, name, f);
    } else {
        let sandbox = sandbox.clone();
        let fn_name = name.to_string();
        register_fn(env, name, move |args| {
            sandbox.check(cap, &fn_name)?;
            f(args)
        });
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn register_fn_path_gated(
    env: &Env,
    sandbox: &Sandbox,
    cap: Caps,
    name: &str,
    path_args: &[usize],
    f: impl Fn(&[Value]) -> Result<Value, sema_core::SemaError> + 'static,
) {
    if sandbox.is_unrestricted() {
        register_fn(env, name, f);
    } else {
        let sandbox = sandbox.clone();
        let fn_name = name.to_string();
        let path_indices: Vec<usize> = path_args.to_vec();
        register_fn(env, name, move |args| {
            sandbox.check(cap, &fn_name)?;
            for &idx in &path_indices {
                if let Some(val) = args.get(idx) {
                    if let Some(p) = val.as_str() {
                        sandbox.check_path(p, &fn_name)?;
                    }
                }
            }
            f(args)
        });
    }
}

/// Like [`register_fn_path_gated`], but the op body speaks the runtime native
/// ABI (`NativeResult`) so its `in_runtime_quantum` branch can return a
/// `NativeOutcome::Suspend` (an external-wait offload) directly. The sandbox
/// capability + path checks are applied identically, then the checked body is
/// exposed under both ABIs: the runtime callback returns the body's structural
/// `NativeOutcome`, and the synchronous value callback unwraps a plain `Return`
/// when no runtime quantum is active.
#[cfg(not(target_arch = "wasm32"))]
fn register_runtime_fn_path_gated(
    env: &Env,
    sandbox: &Sandbox,
    cap: Caps,
    name: &str,
    path_args: &[usize],
    f: impl Fn(&[Value]) -> sema_core::runtime::NativeResult + 'static,
) {
    use sema_core::runtime::NativeOutcome;
    type RuntimeFnBody = dyn Fn(&[Value]) -> sema_core::runtime::NativeResult;
    let checked: std::rc::Rc<RuntimeFnBody> = if sandbox.is_unrestricted() {
        std::rc::Rc::new(f)
    } else {
        let sandbox = sandbox.clone();
        let fn_name = name.to_string();
        let path_indices: Vec<usize> = path_args.to_vec();
        std::rc::Rc::new(move |args: &[Value]| {
            sandbox.check(cap, &fn_name)?;
            for &idx in &path_indices {
                if let Some(val) = args.get(idx) {
                    if let Some(p) = val.as_str() {
                        sandbox.check_path(p, &fn_name)?;
                    }
                }
            }
            f(args)
        })
    };
    let for_func = checked.clone();
    let for_runtime = checked;
    let func_name = name.to_string();
    env.set(
        sema_core::intern(name),
        Value::native_fn(sema_core::NativeFn::simple_with_runtime(
            name,
            move |args| match for_func(args)? {
                NativeOutcome::Return(value) => Ok(value),
                _ => Err(sema_core::SemaError::eval(format!(
                    "{func_name}: native suspended outside the cooperative runtime"
                ))),
            },
            move |_ctx, args| for_runtime(args),
        )),
    );
}

fn register_fn(
    env: &Env,
    name: &str,
    f: impl Fn(&[Value]) -> Result<Value, sema_core::SemaError> + 'static,
) {
    env.set(
        sema_core::intern(name),
        Value::native_fn(sema_core::NativeFn::simple(name, f)),
    );
}

fn register_fn_with_escaping_args(
    env: &Env,
    name: &str,
    escaping_args: &'static [usize],
    f: impl Fn(&[Value]) -> Result<Value, sema_core::SemaError> + 'static,
) {
    env.set(
        sema_core::intern(name),
        Value::native_fn(sema_core::NativeFn::simple(name, f).with_escaping_args(escaping_args)),
    );
}

/// Like [`register_fn`], but the op body speaks the runtime native ABI
/// (`NativeResult`) so its `in_runtime_quantum` branch can return a
/// `NativeOutcome::Suspend` (an external-wait offload) directly. The single body
/// is exposed under both ABIs: the runtime callback returns its
/// `NativeOutcome`, while the synchronous value callback unwraps a plain
/// `Return` when no runtime quantum is active. Mirrors
/// [`register_runtime_fn_path_gated`] without the sandbox checks.
#[cfg(not(target_arch = "wasm32"))]
fn register_runtime_fn(
    env: &Env,
    name: &str,
    f: impl Fn(&[Value]) -> sema_core::runtime::NativeResult + 'static,
) {
    use sema_core::runtime::NativeOutcome;
    let body = std::rc::Rc::new(f);
    let for_func = body.clone();
    let for_runtime = body;
    let func_name = name.to_string();
    env.set(
        sema_core::intern(name),
        Value::native_fn(sema_core::NativeFn::simple_with_runtime(
            name,
            move |args| match for_func(args)? {
                NativeOutcome::Return(value) => Ok(value),
                _ => Err(sema_core::SemaError::eval(format!(
                    "{func_name}: native suspended outside the cooperative runtime"
                ))),
            },
            move |_ctx, args| for_runtime(args),
        )),
    );
}

/// Like [`register_runtime_fn`], but cap-gated (no path check) — for ops that
/// need a single capability check ahead of a runtime-ABI body (`zip/*`,
/// `tar/*`, `patch/apply-file`). The sandbox check is applied identically under
/// both ABIs before the body runs.
#[cfg(not(target_arch = "wasm32"))]
fn register_runtime_fn_gated(
    env: &Env,
    sandbox: &Sandbox,
    cap: Caps,
    name: &str,
    f: impl Fn(&[Value]) -> sema_core::runtime::NativeResult + 'static,
) {
    if sandbox.is_unrestricted() {
        register_runtime_fn(env, name, f);
    } else {
        let sandbox = sandbox.clone();
        let fn_name = name.to_string();
        register_runtime_fn(env, name, move |args| {
            sandbox.check(cap, &fn_name)?;
            f(args)
        });
    }
}
