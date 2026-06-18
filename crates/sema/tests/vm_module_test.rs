//! Tests for VM-backed `(load ...)` and `(import ...)`: when the VM is the active
//! backend, a module's body is compiled and run on the bytecode VM (not the
//! tree-walker), so async/channels work in modules and the code runs at VM speed.
//!
//! `(import ...)` also runs its module body on the VM (M4). Module isolation —
//! an exported fn calling a private module helper — holds because M1 gives each
//! closure a home-globals pointer to its defining (module) env, and each frame
//! restores its own function table, so exported closures resolve their own
//! globals/functions even when copied into and called from the importer.

use sema_core::Value;
use sema_eval::Interpreter;
use std::path::PathBuf;

fn temp_dir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("sema-vmmod-{tag}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn write(dir: &std::path::Path, name: &str, src: &str) -> String {
    let p = dir.join(name);
    std::fs::write(&p, src).expect("write module file");
    p.to_string_lossy().to_string()
}

/// Evaluate on the VM backend (sets vm_backend=true → load runs on the VM).
fn vm(input: &str) -> Result<Value, String> {
    Interpreter::new()
        .eval_str_compiled(input)
        .map_err(|e| e.to_string())
}

/// Evaluate on the tree-walker backend.
fn tw(input: &str) -> Result<Value, String> {
    Interpreter::new()
        .eval_str(input)
        .map_err(|e| e.to_string())
}

fn assert_equiv(input: &str) -> Value {
    let v = vm(input).unwrap_or_else(|e| panic!("VM failed for `{input}`: {e}"));
    let t = tw(input).unwrap_or_else(|e| panic!("TW failed for `{input}`: {e}"));
    assert_eq!(v, t, "VM/TW divergence for `{input}`");
    v
}

#[test]
fn vm_load_defines_visible_after() {
    let dir = temp_dir("load-vis");
    let m = write(
        &dir,
        "m.sema",
        "(define loaded-value 42)\n(define (dbl x) (* x 2))",
    );
    let r = vm(&format!(
        r#"(begin (load "{m}") (list loaded-value (dbl 21)))"#
    ))
    .unwrap();
    assert_eq!(r, Value::list(vec![Value::int(42), Value::int(42)]));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn vm_load_returns_last_expr() {
    let dir = temp_dir("load-ret");
    let m = write(&dir, "m.sema", "(define a 1)\n(+ a 99)");
    assert_eq!(vm(&format!(r#"(load "{m}")"#)).unwrap(), Value::int(100));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn vm_nested_transitive_load() {
    let dir = temp_dir("load-nested");
    let c = write(&dir, "c.sema", "(define c-val 3)");
    let b = write(
        &dir,
        "b.sema",
        &format!("(load \"{c}\")\n(define b-val (+ c-val 10))"),
    );
    let r = vm(&format!(r#"(begin (load "{b}") (list b-val c-val))"#)).unwrap();
    assert_eq!(r, Value::list(vec![Value::int(13), Value::int(3)]));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn vm_macro_defined_and_used_within_loaded_file() {
    // Per-form expand→compile→run: a defmacro is registered before later forms
    // in the same file are compiled, so intra-file macro use works on the VM.
    let dir = temp_dir("load-macro");
    let m = write(
        &dir,
        "macros.sema",
        "(defmacro twice (x) (list (quote begin) x x))\n(define counter 0)\n(twice (set! counter (+ counter 1)))\n(define result counter)",
    );
    assert_eq!(
        vm(&format!(r#"(begin (load "{m}") result)"#)).unwrap(),
        Value::int(2)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn vm_async_in_loaded_module_works() {
    // The motivating capability: async (a VM-only feature) inside a loaded file
    // works because the body runs on the VM.
    let dir = temp_dir("load-async");
    let m = write(
        &dir,
        "amod.sema",
        "(define (compute) (await (async (+ 40 2))))",
    );
    let src = format!(r#"(begin (load "{m}") (compute))"#);
    assert_eq!(vm(&src).unwrap(), Value::int(42));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn vm_load_redefine_global_invalidates_cache() {
    // Regression: a global redefined inside a loaded file must be observed by the
    // caller afterward (the inner VM runs on a cloned env; load bumps the shared
    // env's version so the outer VM's inline global cache is invalidated).
    let dir = temp_dir("load-cache");
    let m = write(&dir, "redef.sema", "(define shared 999)");
    let r = vm(&format!(
        r#"(begin (define shared 1) (define (peek) shared) (list (peek) (begin (load "{m}") (peek))))"#
    ))
    .unwrap();
    assert_eq!(
        r,
        Value::list(vec![Value::int(1), Value::int(999)]),
        "second peek must see the redefined value, not a stale cached one"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn vm_load_error_propagates_and_recovers() {
    let dir = temp_dir("load-err");
    let bad = write(&dir, "bad.sema", "(+ 1 undefined-symbol-xyz)");
    let good = write(&dir, "good.sema", "(define ok 1)");
    let err = vm(&format!(r#"(load "{bad}")"#)).unwrap_err();
    assert!(
        err.to_lowercase().contains("undefined-symbol-xyz")
            || err.to_lowercase().contains("unbound"),
        "loaded-file error should surface: {err}"
    );
    // A subsequent load on a fresh interpreter still works (stacks balanced).
    assert_eq!(
        vm(&format!(r#"(begin (load "{good}") ok)"#)).unwrap(),
        Value::int(1)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn vm_load_matches_tree_walker() {
    let dir = temp_dir("load-equiv");
    let m = write(&dir, "m.sema", "(define (sq x) (* x x))\n(define base 5)");
    assert_equiv(&format!(r#"(begin (load "{m}") (+ (sq base) base))"#));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn vm_backend_import_keeps_tree_walker_isolation() {
    // M4: import now runs the module body on the VM, and module isolation still
    // holds — the ubiquitous "exported fn calls a private helper" pattern works
    // because M1 gives the exported closure a home-globals pointer to the
    // module env, so `private-helper` resolves there even when `public-api` is
    // copied into and called from the importer.
    let dir = temp_dir("imp-iso");
    let m = write(
        &dir,
        "lib.sema",
        "(define (private-helper x) (* x 10))\n(define (public-api x) (private-helper x))",
    );
    // selective import of only the public fn
    let r = vm(&format!(
        r#"(begin (import "{m}" public-api) (public-api 5))"#
    ))
    .unwrap();
    assert_eq!(r, Value::int(50));
    // private helper must not leak into the importer
    let leaked = vm(&format!(
        r#"(begin (import "{m}" public-api) (private-helper 1))"#
    ));
    assert!(
        leaked.is_err(),
        "private-helper must not leak, got {leaked:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// (Removed `vm_backend_flag_resets_for_single_expr_eval`: it asserted the
// tree-walker backend-flag reset — now obsolete, as every eval entry point runs
// on the VM, so async in a loaded file always works regardless of entry point.)

// === M4: VM-native import (module body runs on the VM) ===

#[test]
fn vm_import_runs_on_vm_async_in_module() {
    // Decisive proof import runs on the VM: an imported fn uses async/await
    // (a VM-only feature). On the tree-walker this errors; here it must succeed.
    let dir = temp_dir("imp-async");
    let m = write(
        &dir,
        "amod.sema",
        "(define (compute) (await (async (+ 40 2))))",
    );
    let r = vm(&format!(r#"(begin (import "{m}" compute) (compute))"#)).unwrap();
    assert_eq!(r, Value::int(42));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn vm_import_private_helper_name_collision_with_importer() {
    // Adversarial: the importer defines its OWN `helper` with different behavior
    // than the module's private `helper`. The exported `api` must call the
    // MODULE's helper (home-globals), not the importer's — no cache aliasing or
    // global-resolution bleed across the two isolated global envs.
    let dir = temp_dir("imp-collide");
    let m = write(
        &dir,
        "lib.sema",
        "(define (helper x) (* x 100))\n(define (api x) (helper x))",
    );
    let r = vm(&format!(
        r#"(begin (define (helper x) (+ x 1)) (import "{m}" api) (list (api 5) (helper 5)))"#
    ))
    .unwrap();
    // api -> module helper -> 500 ; importer helper -> 6
    assert_eq!(r, Value::list(vec![Value::int(500), Value::int(6)]));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn vm_import_interleaved_with_importer_functions() {
    // Adversarial: interleave calls to an imported fn and importer-local fns so
    // the VM's function-table swapping for cross-module closures must restore
    // correctly (a wrong table would mis-resolve MakeClosure/Call func ids).
    let dir = temp_dir("imp-interleave");
    let m = write(
        &dir,
        "lib.sema",
        "(define (mod-secret) 7)\n(define (mod-fn x) (* x (mod-secret)))",
    );
    let r = vm(&format!(
        r#"(begin
             (import "{m}" mod-fn)
             (define (loc-fn x) (+ x 1000))
             (list (loc-fn 1) (mod-fn 2) (loc-fn 3) (mod-fn 4) (map (fn (n) (mod-fn n)) (list 1 2 3))))"#
    ))
    .unwrap();
    assert_eq!(
        r,
        Value::list(vec![
            Value::int(1001),
            Value::int(14),
            Value::int(1003),
            Value::int(28),
            Value::list(vec![Value::int(7), Value::int(14), Value::int(21)]),
        ])
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn vm_import_matches_tree_walker_isolation() {
    // Equivalence: VM-backed import and tree-walker import agree on the result
    // of the exported-calls-private pattern.
    let dir = temp_dir("imp-equiv");
    let m = write(
        &dir,
        "lib.sema",
        "(define (priv x) (* x 3))\n(define (pub x) (+ (priv x) 1))",
    );
    assert_equiv(&format!(r#"(begin (import "{m}" pub) (pub 10))"#));
    let _ = std::fs::remove_dir_all(&dir);
}
