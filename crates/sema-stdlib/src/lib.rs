#![allow(clippy::mutable_key_type, clippy::cloned_ref_to_slice_refs)]
mod arithmetic;
mod async_ops;
mod bitwise;
mod bytevector;
mod comparison;
mod context;
mod crypto;
mod csv_ops;
mod datetime;
#[cfg(not(target_arch = "wasm32"))]
mod http;
#[cfg(not(target_arch = "wasm32"))]
mod io;
pub(crate) mod json;
#[cfg(not(target_arch = "wasm32"))]
mod kv;
mod list;
mod map;
mod math;
mod meta;
mod otel;
#[cfg(not(target_arch = "wasm32"))]
mod pdf;
mod pio;
mod predicates;
mod regex_ops;
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
use sema_core::Caps;
use sema_core::{Env, Sandbox, Value};

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
