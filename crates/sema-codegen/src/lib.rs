//! Native code generation for Sema, on Cranelift.
//!
//! Sema's bytecode VM stays the sole evaluator. This crate is an accelerator
//! bolted to the side of it: when a function is called often enough and its
//! body falls inside a small, provably pure subset of the bytecode, the
//! function is compiled to machine code and the VM calls that instead of
//! pushing a frame. Everything else — every list, string, map, closure, global,
//! native call, LLM primitive, and async operation — runs in the VM exactly as
//! before.
//!
//! # Why Cranelift fits
//!
//! - It is pure Rust, so it joins the workspace as a normal Cargo dependency
//!   with no C++ toolchain and no LLVM.
//! - Sema's `Value` is a NaN-boxed `u64`, so compiled functions pass and return
//!   plain `I64`s in registers, and type tests are single bitwise instructions.
//! - Sema refcounts with `Rc` instead of tracing, so there are no GC safepoints
//!   or stack maps to emit.
//!
//! # What gets compiled
//!
//! The compilable subset is: constants, local slots, `if` / `and` / `or`
//! branching, `+ - * / modulo`, negation, `not`, numeric comparison, and
//! self-recursion (tail and non-tail). Values are limited to *immediates* —
//! fixnums, floats, booleans, nil, chars, symbols, keywords — which own no
//! `Rc`, so generated code never touches a refcount.
//!
//! Anything else stops compilation, and any case that would need to allocate or
//! raise at runtime — a sum that overflows the fixnum range into a bignum, an
//! inexact integer division that yields a rational, division by zero, a
//! non-numeric operand — makes the compiled call bail so the VM runs it
//! instead. Bailing is always correct because the subset admits no side
//! effects, so an abandoned call has changed nothing.
//!
//! # Use
//!
//! ```no_run
//! sema_codegen::install().expect("no code generator for this host");
//! // Any VM on this thread now compiles hot functions.
//! ```
//!
//! The compile threshold defaults to 32 calls and is read from
//! `SEMA_JIT_THRESHOLD`.
//!
//! # Not covered
//!
//! WebAssembly hosts cannot map executable pages, so this crate is native-only;
//! `sema-wasm` keeps using the VM. Ahead-of-time object emission
//! (`cranelift-object`, for `sema build`) reuses [`decode`] and [`translate`]
//! but is not implemented yet.

pub mod backend;
pub mod decode;
pub mod translate;

pub use backend::{BackendError, CraneliftBackend};
pub use decode::{analyze, Program, Reject};

/// Build a Cranelift backend and install it on the current thread.
///
/// After this, every VM running on this thread compiles hot functions that fall
/// inside the supported subset. Calling it twice replaces the previous backend.
pub fn install() -> Result<(), BackendError> {
    CraneliftBackend::install()
}

/// Remove this thread's backend and stop running compiled code.
pub fn uninstall() {
    sema_vm::jit::uninstall_backend();
}

/// Whether a backend is installed on this thread.
pub fn is_installed() -> bool {
    sema_vm::jit::is_active()
}

/// Counters describing what the JIT did on this thread.
pub fn stats() -> sema_vm::jit::JitStats {
    sema_vm::jit::stats()
}
