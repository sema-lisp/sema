//! Regression tests for audit finding C11: the `.semac` deserializer must
//! verify stack balance, because `vm.rs` relies on stack-balanced bytecode via
//! `pop_unchecked`. A crafted `.semac` with leading `Pop` (or any other
//! underflowing sequence) would cause UB in release builds.
//!
//! These exercise the abstract stack-depth verifier (ADR #56) wired into
//! `validate_bytecode`, which runs inside `deserialize_from_bytes`. See:
//!   - `docs/limitations.md` #32
//!   - `docs/adr.md` ADR #56
//!   - the SAFETY comment above `pop_unchecked` in `crates/sema-vm/src/vm.rs`

use sema_vm::{deserialize_from_bytes, serialize_to_bytes, Chunk, CompileResult, Emitter, Op};

/// Serialize a hand-built main chunk and attempt to deserialize it, returning
/// the deserialization result.
fn roundtrip_chunk(chunk: Chunk) -> Result<CompileResult, sema_core::SemaError> {
    let result = CompileResult::new(chunk, vec![]);
    let bytes = serialize_to_bytes(&result, 0).expect("serialize should succeed");
    deserialize_from_bytes(&bytes)
}

/// Assert that deserializing a chunk fails (the verifier rejected it) and return
/// the error message. `CompileResult` is not `Debug`, so we cannot use
/// `expect_err` directly.
fn expect_rejected(chunk: Chunk, what: &str) -> String {
    match roundtrip_chunk(chunk) {
        Ok(_) => panic!("{what}: expected rejection, but deserialization succeeded"),
        Err(e) => e.to_string(),
    }
}

/// A `.semac` chunk whose first opcode is `Pop` must be rejected at load time,
/// not executed. The VM would otherwise invoke `pop_unchecked` on an empty
/// stack and trigger UB.
#[test]
fn semac_leading_pop_rejected_at_load() {
    let mut e = Emitter::new();
    e.emit_op(Op::Pop); // underflow: nothing on the stack
    e.emit_op(Op::Return);
    let chunk = e.into_chunk();

    let err = expect_rejected(chunk, "leading Pop");
    assert!(
        err.contains("underflow"),
        "expected stack-underflow rejection, got: {err}"
    );
}

/// An unbalanced sequence in the middle of a chunk: `Const 0; Pop; Pop`
/// underflows on the second `Pop`. The verifier rejects this via abstract
/// interpretation of stack depth per instruction.
#[test]
fn semac_mid_chunk_underflow_rejected() {
    let mut e = Emitter::new();
    e.emit_const(sema_core::Value::int(1)); // depth 1
    e.emit_op(Op::Pop); // depth 0
    e.emit_op(Op::Pop); // underflow
    e.emit_op(Op::Return);
    let chunk = e.into_chunk();

    let err = expect_rejected(chunk, "mid-chunk underflow");
    assert!(
        err.contains("underflow"),
        "expected stack-underflow rejection, got: {err}"
    );
}

/// A binary arithmetic op with too few operands must be rejected: `Const 1; Add`
/// pops two values but only one is available.
#[test]
fn semac_binary_op_underflow_rejected() {
    let mut e = Emitter::new();
    e.emit_const(sema_core::Value::int(1)); // depth 1
    e.emit_op(Op::Add); // pops 2, underflow
    e.emit_op(Op::Return);
    let chunk = e.into_chunk();

    let err = expect_rejected(chunk, "arithmetic underflow");
    assert!(
        err.contains("underflow"),
        "expected stack-underflow rejection, got: {err}"
    );
}

/// Control flow that joins at inconsistent stack depths must be rejected: the
/// fallthrough leaves depth 1 but the jump target is reached at depth 0.
#[test]
fn semac_inconsistent_join_rejected() {
    // JumpIfFalse skips a single push, so the two paths reach the join at
    // different depths.
    //   0: Nil            depth 0 -> 1
    //   1: JumpIfFalse +3 pop cond -> depth 0 (taken: pc 8, not-taken: pc 6)
    //   6: Nil            depth 0 -> 1  (push so fallthrough is depth 1 at pc 7)
    //   7: <join target reached at depth 1 from fallthrough, depth 0 from jump>
    let mut e = Emitter::new();
    e.emit_op(Op::Nil); // pc 0: depth 1
    e.emit_op(Op::JumpIfFalse);
    e.emit_i32(1); // pc 1: skip the next 1-byte op; not-taken -> pc 6, taken -> pc 7
    e.emit_op(Op::Nil); // pc 6: push (fallthrough path)
    e.emit_op(Op::Return); // pc 7: join — depth 1 (fallthrough) vs 0 (jump)
    let chunk = e.into_chunk();

    let msg = expect_rejected(chunk, "inconsistent join");
    assert!(
        msg.contains("disagreement") || msg.contains("underflow"),
        "expected stack-depth disagreement rejection, got: {msg}"
    );
}

/// A well-formed, stack-balanced program must still round-trip and pass the
/// verifier (positive control for the negative tests above).
#[test]
fn semac_balanced_program_accepted() {
    let mut e = Emitter::new();
    e.emit_const(sema_core::Value::int(1));
    e.emit_const(sema_core::Value::int(2));
    e.emit_op(Op::Add);
    e.emit_op(Op::Return);
    let chunk = e.into_chunk();

    let result = roundtrip_chunk(chunk).expect("balanced program must be accepted");
    assert_eq!(result.chunk.consts.len(), 2);
}
