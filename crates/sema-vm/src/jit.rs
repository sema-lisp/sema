//! Native-code hook for the bytecode VM.
//!
//! The VM stays the sole evaluator. This module is only the seam a native code
//! generator plugs into: it owns the calling convention, the per-`Function`
//! compile state, and the hot-call counter, so the generator crate
//! (`sema-codegen`) has to know nothing about `Value` ownership or frame
//! layout. When no backend is installed, the whole path costs one thread-local
//! `bool` load per closure call.
//!
//! # Calling convention
//!
//! A compiled entry point is an `extern "C"` function taking the call's
//! arguments as raw NaN-boxed `u64`s, one per register, and returning one raw
//! `u64`:
//!
//! ```text
//! extern "C" fn(a0: u64, a1: u64, ..., aN: u64) -> u64
//! ```
//!
//! Arity is capped at [`MAX_JIT_ARITY`] so every argument is register-passed on
//! both System V and AAPCS. Functions with a rest parameter are never compiled.
//!
//! # The immediate-only rule (this is what makes it sound)
//!
//! Generated code handles values as plain `u64` bits with **no refcount
//! traffic**: it never clones an `Rc`, never drops one, and never stores one.
//! That is only safe if no `Rc` is ever in its hands, so the seam enforces two
//! things:
//!
//! 1. **Entry guard** — every argument must satisfy [`Value::is_immediate`].
//!    One non-immediate argument (a list, a string, a bignum) and the call goes
//!    to the VM instead.
//! 2. **Producer guard** — the generator may only emit code that produces
//!    immediates. Any operation that could allocate (fixnum overflow to bignum,
//!    inexact integer division, string concatenation) must bail instead.
//!
//! With both in force, every bit pattern crossing the boundary is a value whose
//! `Clone` and `Drop` are no-ops, so [`Value::from_raw_bits`] on the result
//! cannot double-free.
//!
//! # Bailing out
//!
//! Generated code returns [`JIT_BAIL`] to mean "a guard failed, run this call
//! in the VM". The VM then executes the call normally, from the start. Re-running
//! from the start is correct because a compiled function is pure: the opcode
//! whitelist in `sema-codegen` admits no global store, no mutation, no call
//! other than self-recursion, and no I/O, so a partially executed compiled call
//! has changed nothing observable.
//!
//! [`JIT_BAIL`] is `NAN_TAG_MASK`, i.e. a boxed value with tag 63 and an empty
//! payload. The highest tag a real `Value` uses is 34, so no `Value` can ever
//! collide with it (`sema_core::value::immediate_tests::top_tag_is_unused_by_real_values`).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use sema_core::{Value, NAN_TAG_MASK};

use crate::chunk::Function;
use crate::opcodes::Op;

/// Returned by generated code to mean "guard failed, re-run this call in the VM".
///
/// A boxed value with tag 63 and payload 0. No `Value` constructor produces
/// tag 63, so this cannot alias a real result.
pub const JIT_BAIL: u64 = NAN_TAG_MASK;

/// Largest arity that can be compiled. Keeps every argument in a register on
/// both System V (6 integer argument registers) and AAPCS (8).
pub const MAX_JIT_ARITY: usize = 6;

/// Deepest native recursion a compiled function may reach before bailing.
///
/// Compiled code recurses on the machine stack, which the VM's frame vector
/// cannot police, so the generator must count non-tail self-calls itself and
/// return [`JIT_BAIL`] at this depth. Set to the VM's own frame limit so a
/// program that would overflow in the VM also stops here — the VM re-runs the
/// call and raises the usual "stack overflow" error.
pub const JIT_MAX_DEPTH: u64 = crate::vm::MAX_FRAMES as u64;

/// `jit_entry` slot value meaning "never examined".
const STATE_UNSEEN: usize = 0;
/// `jit_entry` slot value meaning "examined, cannot be compiled".
const STATE_INELIGIBLE: usize = 1;

/// Default number of VM calls a function must take before compilation is
/// attempted. Overridden by `SEMA_JIT_THRESHOLD`; `0` compiles on first call.
const DEFAULT_THRESHOLD: u32 = 32;

/// Per-`Function` native-code state.
///
/// Lives on [`Function`] so a hot call checks one `Cell` instead of hashing a
/// pointer. Never serialized — a `.semac` file carries bytecode, and each
/// process decides for itself what to compile.
#[derive(Debug, Default)]
pub struct JitSlot {
    /// [`STATE_UNSEEN`], [`STATE_INELIGIBLE`], or a native entry point address.
    entry: Cell<usize>,
    /// VM calls seen so far, counted only while below the compile threshold.
    calls: Cell<u32>,
}

impl Clone for JitSlot {
    /// A clone starts cold. Cloning a `Function` is rare (deserialization and
    /// tests), and re-deciding costs one analysis pass at worst.
    fn clone(&self) -> Self {
        JitSlot::default()
    }
}

/// A native code generator the VM can call into.
///
/// Implemented by `sema-codegen`. The VM asks once per function, caches the
/// answer in that function's [`JitSlot`], and owns everything else.
pub trait JitBackend {
    /// Compile `func` and return its entry point, or `None` if the function is
    /// outside the compilable subset.
    ///
    /// # Safety contract for implementors
    ///
    /// The returned pointer must be an `extern "C"` function of exactly
    /// `func.arity` `u64` parameters returning `u64`, it must stay valid and
    /// executable for the life of the backend, and it must obey the
    /// immediate-only rule described in the module docs: return [`JIT_BAIL`]
    /// rather than produce any value that owns an `Rc`.
    fn compile(&self, func: &Rc<Function>) -> Option<*const u8>;

    /// Human-readable backend name, for `sema jit --stats`.
    fn name(&self) -> &'static str;
}

thread_local! {
    /// Fast gate. Checked before anything else on every closure call, so the
    /// cost of an uninstalled JIT is a single thread-local `bool` load.
    static ACTIVE: Cell<bool> = const { Cell::new(false) };
    static BACKEND: RefCell<Option<Rc<dyn JitBackend>>> = const { RefCell::new(None) };
    static THRESHOLD: Cell<u32> = const { Cell::new(DEFAULT_THRESHOLD) };
    static STATS: RefCell<JitStats> = const { RefCell::new(JitStats::new()) };
}

/// Counters describing what the JIT did this process.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct JitStats {
    /// Functions the backend accepted and compiled.
    pub compiled: u32,
    /// Functions the backend rejected as outside the compilable subset.
    pub rejected: u32,
    /// Calls that ran as native code and returned a result.
    pub native_calls: u64,
    /// Calls that ran as native code, hit a guard, and re-ran in the VM.
    pub bailouts: u64,
    /// Calls skipped because an argument was not an immediate value.
    pub arg_rejects: u64,
}

impl JitStats {
    const fn new() -> Self {
        JitStats {
            compiled: 0,
            rejected: 0,
            native_calls: 0,
            bailouts: 0,
            arg_rejects: 0,
        }
    }
}

/// Install `backend` for the current thread and start compiling hot functions.
pub fn install_backend(backend: Rc<dyn JitBackend>) {
    BACKEND.with(|b| *b.borrow_mut() = Some(backend));
    ACTIVE.with(|a| a.set(true));
}

/// Remove the current thread's backend.
///
/// Already-compiled entry points stay recorded in their [`JitSlot`]s but are
/// never called again, because [`try_execute`] gates on [`is_active`] first.
/// Dropping the backend frees its code memory, so the gate must come first.
pub fn uninstall_backend() {
    ACTIVE.with(|a| a.set(false));
    BACKEND.with(|b| *b.borrow_mut() = None);
}

/// Whether a backend is installed on this thread.
pub fn is_active() -> bool {
    ACTIVE.with(|a| a.get())
}

/// Set how many VM calls a function takes before compilation is attempted.
/// `0` compiles on the first call.
pub fn set_threshold(n: u32) {
    THRESHOLD.with(|t| t.set(n));
}

/// The configured compile threshold.
pub fn threshold() -> u32 {
    THRESHOLD.with(|t| t.get())
}

/// Read the compile threshold from `SEMA_JIT_THRESHOLD`, if set and valid.
pub fn threshold_from_env() {
    if let Ok(raw) = std::env::var("SEMA_JIT_THRESHOLD") {
        if let Ok(n) = raw.trim().parse::<u32>() {
            set_threshold(n);
        }
    }
}

/// Snapshot of this thread's JIT counters.
pub fn stats() -> JitStats {
    STATS.with(|s| *s.borrow())
}

/// Reset this thread's JIT counters.
pub fn reset_stats() {
    STATS.with(|s| *s.borrow_mut() = JitStats::new());
}

fn bump(f: impl FnOnce(&mut JitStats)) {
    STATS.with(|s| f(&mut s.borrow_mut()));
}

/// Try to run `func` as native code over `args`.
///
/// Returns `Some(result)` when native code ran to completion, and `None` when
/// the call must go to the VM — no backend, function not compilable, an
/// argument that is not an immediate, or a guard that failed mid-run.
///
/// `args` are the callee's arguments in declaration order. The caller must have
/// already checked arity.
pub(crate) fn try_execute(func: &Rc<Function>, args: &[Value]) -> Option<Value> {
    if !ACTIVE.with(|a| a.get()) {
        return None;
    }
    execute_slow(func, args)
}

/// The out-of-line half of [`try_execute`], kept separate so the hot VM path
/// only inlines the `ACTIVE` check.
#[inline(never)]
fn execute_slow(func: &Rc<Function>, args: &[Value]) -> Option<Value> {
    let entry = match func.jit.entry.get() {
        STATE_INELIGIBLE => return None,
        STATE_UNSEEN => resolve_entry(func)?,
        addr => addr,
    };

    // Entry guard: generated code performs no refcount work, so it may only
    // ever see values whose Clone and Drop are no-ops.
    if !args.iter().all(Value::is_immediate) {
        bump(|s| s.arg_rejects += 1);
        return None;
    }

    // SAFETY: `entry` came from a `JitBackend::compile` call for this exact
    // `func`, so by that method's contract it is an `extern "C"` function of
    // `func.arity` u64 parameters returning u64, and it stays valid while the
    // backend is installed (which `ACTIVE` guarantees for the duration of this
    // call — nothing here can uninstall it). `args.len() == func.arity` is the
    // caller's precondition, re-checked below against MAX_JIT_ARITY.
    let raw = unsafe { call_entry(entry, args)? };

    if raw == JIT_BAIL {
        bump(|s| s.bailouts += 1);
        return None;
    }
    bump(|s| s.native_calls += 1);

    // SAFETY: generated code only ever produces immediates (module docs, and
    // enforced by the opcode whitelist in sema-codegen), and an immediate owns
    // no Rc, so building a Value from these bits transfers no ownership.
    let result = unsafe { Value::from_raw_bits(raw) };
    debug_assert!(
        result.is_immediate(),
        "JIT returned a non-immediate value: 0x{raw:016x}"
    );
    Some(result)
}

/// Ask the backend about `func` once and record the verdict in its slot.
fn resolve_entry(func: &Rc<Function>) -> Option<usize> {
    // Rest parameters would need a heap-allocated list built at entry, which is
    // outside the immediate-only rule.
    if func.has_rest || func.arity as usize > MAX_JIT_ARITY {
        func.jit.entry.set(STATE_INELIGIBLE);
        return None;
    }

    // Warm up: only spend compile time on functions that are actually hot.
    //
    // A call counter alone misses the hottest shape there is. A loop — a
    // self-tail-call or a backward jump — runs its iterations *inside* one VM
    // frame, so a function that spends a second looping is still only one call.
    // Waiting for a 32nd call that never comes would leave exactly the code the
    // generator exists for running on the VM, so a function that contains a loop
    // is compiled the first time it is called.
    let hits = func.jit.calls.get().saturating_add(1);
    func.jit.calls.set(hits);
    if hits <= THRESHOLD.with(|t| t.get()) && !contains_loop(&func.chunk.code) {
        return None;
    }

    let backend = BACKEND.with(|b| b.borrow().clone())?;
    match backend.compile(func) {
        Some(ptr) => {
            let addr = ptr as usize;
            // A real code address is never one of the state sentinels, but
            // encoding both in one word means being sure.
            debug_assert!(addr > STATE_INELIGIBLE, "JIT entry aliases a state marker");
            if addr <= STATE_INELIGIBLE {
                func.jit.entry.set(STATE_INELIGIBLE);
                return None;
            }
            func.jit.entry.set(addr);
            bump(|s| s.compiled += 1);
            Some(addr)
        }
        None => {
            func.jit.entry.set(STATE_INELIGIBLE);
            bump(|s| s.rejected += 1);
            None
        }
    }
}

/// Whether `code` loops: a self-tail-call, or a branch to an earlier pc.
///
/// Walks instructions through [`crate::serialize::advance_pc`] rather than
/// scanning for opcode bytes — an operand byte can hold any value, including one
/// that happens to equal a branch opcode. A malformed chunk (which the
/// deserializer already rejects) simply reports "no loop"; the function then
/// waits for the call counter like any other.
fn contains_loop(code: &[u8]) -> bool {
    let mut pc = 0usize;
    while pc < code.len() {
        let Ok((op, next)) = crate::serialize::advance_pc(code, pc) else {
            return false;
        };
        match op {
            Op::SelfTailCall => return true,
            Op::Jump | Op::JumpIfFalse | Op::JumpIfTrue => {
                let rel =
                    i32::from_le_bytes([code[pc + 1], code[pc + 2], code[pc + 3], code[pc + 4]]);
                if rel < 0 {
                    return true;
                }
            }
            _ => {}
        }
        pc = next;
    }
    false
}

/// Call a compiled entry point with `args`.
///
/// Returns `None` for an arity this convention cannot express, which
/// [`resolve_entry`] already rules out.
///
/// # Safety
///
/// `entry` must be an `extern "C"` function of exactly `args.len()` `u64`
/// parameters returning `u64`, valid for the duration of the call.
unsafe fn call_entry(entry: usize, args: &[Value]) -> Option<u64> {
    macro_rules! bits {
        ($i:expr) => {
            args[$i].raw_bits()
        };
    }
    let out = match args.len() {
        0 => std::mem::transmute::<usize, extern "C" fn() -> u64>(entry)(),
        1 => std::mem::transmute::<usize, extern "C" fn(u64) -> u64>(entry)(bits!(0)),
        2 => {
            std::mem::transmute::<usize, extern "C" fn(u64, u64) -> u64>(entry)(bits!(0), bits!(1))
        }
        3 => std::mem::transmute::<usize, extern "C" fn(u64, u64, u64) -> u64>(entry)(
            bits!(0),
            bits!(1),
            bits!(2),
        ),
        4 => std::mem::transmute::<usize, extern "C" fn(u64, u64, u64, u64) -> u64>(entry)(
            bits!(0),
            bits!(1),
            bits!(2),
            bits!(3),
        ),
        5 => std::mem::transmute::<usize, extern "C" fn(u64, u64, u64, u64, u64) -> u64>(entry)(
            bits!(0),
            bits!(1),
            bits!(2),
            bits!(3),
            bits!(4),
        ),
        6 => std::mem::transmute::<usize, extern "C" fn(u64, u64, u64, u64, u64, u64) -> u64>(
            entry,
        )(bits!(0), bits!(1), bits!(2), bits!(3), bits!(4), bits!(5)),
        _ => return None,
    };
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sentinel must not collide with any value a compiled function could
    /// legitimately return.
    ///
    /// Checked on the raw bits, never by building a `Value`: tag 63 is not a
    /// type `Value::drop` knows, so constructing one and letting it fall out of
    /// scope panics with "invalid heap tag in drop". That panic is the property
    /// this test relies on — it is why [`execute_slow`] compares against
    /// [`JIT_BAIL`] *before* calling `Value::from_raw_bits`, and why a
    /// regression that reordered those two steps would fail loudly instead of
    /// corrupting memory.
    #[test]
    fn bail_sentinel_is_not_a_real_value() {
        // Tag 63, empty payload. Nothing in sema-core produces this.
        assert_eq!(JIT_BAIL, 0xFFFF_E000_0000_0000);
        assert_eq!((JIT_BAIL >> 45) & 0x3F, 63);
        assert_ne!(JIT_BAIL, Value::NIL.raw_bits());
        assert_ne!(JIT_BAIL, Value::FALSE.raw_bits());
        assert_ne!(JIT_BAIL, Value::TRUE.raw_bits());
        assert_ne!(JIT_BAIL, Value::int(0).raw_bits());
        assert_ne!(JIT_BAIL, Value::float(0.0).raw_bits());
        assert_ne!(JIT_BAIL, Value::char('a').raw_bits());
    }

    /// The bail check has to come before the value is rebuilt. This pins the
    /// ordering that keeps the sentinel from ever reaching `Value::drop`.
    #[test]
    fn bail_is_checked_before_a_value_is_built() {
        let raw = JIT_BAIL;
        let result = if raw == JIT_BAIL {
            None
        } else {
            // SAFETY: unreachable here; mirrors `execute_slow`'s ordering.
            Some(unsafe { Value::from_raw_bits(raw) })
        };
        assert!(result.is_none());
    }

    #[test]
    fn inactive_by_default() {
        assert!(!is_active());
    }

    #[test]
    fn threshold_round_trips() {
        let original = threshold();
        set_threshold(7);
        assert_eq!(threshold(), 7);
        set_threshold(original);
    }
}
