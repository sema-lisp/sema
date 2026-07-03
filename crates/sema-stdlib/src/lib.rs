#![allow(clippy::mutable_key_type, clippy::cloned_ref_to_slice_refs)]
#[cfg(not(target_arch = "wasm32"))]
mod archive;
mod arithmetic;
mod async_ops;
#[cfg(not(target_arch = "wasm32"))]
mod async_rt;
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
pub(crate) mod json;
#[cfg(not(target_arch = "wasm32"))]
mod kv;
mod list;
mod map;
#[cfg(not(target_arch = "wasm32"))]
mod markup;
mod math;
mod meta;
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
