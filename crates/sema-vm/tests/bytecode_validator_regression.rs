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

use sema_vm::{
    deserialize_from_bytes, serialize_to_bytes, Chunk, CompileResult, Emitter, ExceptionEntry,
    Function, Op, UpvalueDesc,
};

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

fn expect_result_rejected(result: CompileResult, what: &str) -> String {
    let bytes = serialize_to_bytes(&result, 0).expect("serialize should succeed");
    match deserialize_from_bytes(&bytes) {
        Ok(_) => panic!("{what}: expected rejection, but deserialization succeeded"),
        Err(e) => e.to_string(),
    }
}

fn deserialize_error(bytes: &[u8], what: &str) -> String {
    match deserialize_from_bytes(bytes) {
        Ok(_) => panic!("{what}: expected rejection, but deserialization succeeded"),
        Err(e) => e.to_string(),
    }
}

fn template(upvalue_descs: Vec<UpvalueDesc>) -> Function {
    let mut e = Emitter::new();
    e.emit_op(Op::Nil);
    e.emit_op(Op::Return);
    let upvalue_names = (0..upvalue_descs.len())
        .map(|index| sema_core::intern(&format!("upvalue-{index}")))
        .collect();
    Function {
        name: None,
        chunk: e.into_chunk(),
        upvalue_descs,
        upvalue_names,
        arity: 0,
        has_rest: false,
        param_names: Vec::new().into(),
        local_names: vec![],
        local_scopes: vec![],
        source_file: None,
        cache_offset: 0,
        suspend_cache: std::cell::Cell::new(None),
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
    e.emit_const(sema_core::Value::int(1)).unwrap(); // depth 1
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
    e.emit_const(sema_core::Value::int(1)).unwrap(); // depth 1
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
    e.emit_const(sema_core::Value::int(1)).unwrap();
    e.emit_const(sema_core::Value::int(2)).unwrap();
    e.emit_op(Op::Add);
    e.emit_op(Op::Return);
    let chunk = e.into_chunk();

    let result = roundtrip_chunk(chunk).expect("balanced program must be accepted");
    assert_eq!(result.chunk.consts.len(), 2);
}

/// A chunk containing `SelfTailCall` (issue #62) must serialize, pass the
/// stack-balance verifier (it pops `argc` and exits the frame — no callee
/// slot), and deserialize byte-for-byte unchanged.
#[test]
fn semac_self_tail_call_roundtrips_and_validates() {
    let mut e = Emitter::new();
    e.emit_const(sema_core::Value::int(0)).unwrap(); // one arg on the stack
    e.emit_op(Op::SelfTailCall);
    e.emit_u16(1); // argc = 1: pops 1, exits the frame
    let mut chunk = e.into_chunk();
    chunk.n_locals = 1;
    chunk.max_stack = 2;
    let original = chunk.code.clone();

    let result = roundtrip_chunk(chunk).expect("SelfTailCall chunk must validate and round-trip");
    assert_eq!(result.chunk.code, original);
}

/// A crafted exception entry whose `stack_depth` is inflated above what the
/// protected range can actually supply must be rejected. The handler is
/// reachable only via the exception edge (`Throw` has no fallthrough), so the
/// strict-equality join never cross-checks the seed; the dedicated protected-
/// range depth check is what closes this hole. Without it, the runtime's
/// shrink-only `truncate(base + stack_depth)` would leave the handler with
/// fewer operands than verified and `pop_unchecked` would underflow → UB.
#[test]
fn semac_exception_handler_inflated_stack_depth_rejected() {
    let mut e = Emitter::new();
    e.emit_op(Op::Nil); // pc 0: operand depth 0 -> 1
    e.emit_op(Op::Throw); // pc 1: pops 1, exits frame (no fallthrough)
    e.emit_op(Op::Pop); // pc 2: handler entry
    e.emit_op(Op::Pop); // pc 3
    e.emit_op(Op::Nil); // pc 4
    e.emit_op(Op::Return); // pc 5
    let mut chunk = e.into_chunk();
    chunk.exception_table.push(ExceptionEntry {
        try_start: 0,
        try_end: 2,
        handler_pc: 2,
        stack_depth: 2, // inflated: the protected range only reaches operand depth 0/1
        catch_slot: 0,
    });

    let err = expect_rejected(chunk, "inflated exception handler stack_depth");
    assert!(
        err.contains("protected range") || err.contains("operand depth"),
        "expected handler-depth rejection, got: {err}"
    );
}

/// A crafted `.semac` with a `CallNative` whose `native_id` is out of range
/// must be rejected at load time (audit finding VM-1). The native table is
/// process-local and is NOT serialized, so a deserialized chunk always has an
/// empty native table — any `CallNative` therefore references a missing entry.
/// Without the verifier arm, only a `debug_assert!` in `vm.rs` guarded this, so
/// a release build would index past the resolved native table (OOB / panic).
///
/// The opcode is otherwise stack-balanced (argc=0 pops 0, pushes 1), so only the
/// `native_id < n_natives` bounds check can reject it — not the stack verifier.
#[test]
fn semac_call_native_out_of_range_id_rejected() {
    let mut e = Emitter::new();
    e.emit_op(Op::CallNative);
    e.emit_u16(99); // native_id far past the (empty, unserialized) native table
    e.emit_u16(0); // argc — stack-balanced so this isolates the id check
    e.emit_op(Op::Return);
    let chunk = e.into_chunk();

    let err = expect_rejected(chunk, "out-of-range CallNative native_id");
    assert!(
        err.contains("CallNative") && err.contains("out of range"),
        "expected CallNative out-of-range rejection, got: {err}"
    );
}

/// Even `native_id` 0 must be rejected, because the deserialized native table is
/// empty (`0` entries). This pins the boundary: the check is `< n_natives`, not a
/// looser sentinel comparison.
#[test]
fn semac_call_native_zero_id_rejected_when_table_empty() {
    let mut e = Emitter::new();
    e.emit_op(Op::CallNative);
    e.emit_u16(0); // native_id 0 — still out of range against a 0-entry table
    e.emit_u16(0); // argc
    e.emit_op(Op::Return);
    let chunk = e.into_chunk();

    let err = expect_rejected(chunk, "CallNative native_id 0 against empty table");
    assert!(
        err.contains("CallNative") && err.contains("out of range"),
        "expected CallNative out-of-range rejection, got: {err}"
    );
}

/// A jump past the end of the chunk must be rejected at load time, not left
/// for the VM's runtime pc bounds check to trip mid-program. Pass 2 of
/// `validate_chunk_bytecode` rejects the target outright; the stack-balance
/// walk independently rejects any *reachable* control transfer to
/// `code.len()` or beyond ("falls off the end"). Together these prove the
/// pc-bounds invariant for loaded bytecode (ADR #56).
#[test]
fn semac_oob_jump_rejected_at_load() {
    let mut e = Emitter::new();
    e.emit_op(Op::Jump);
    e.emit_i32(1000); // target pc 1005; the chunk is 6 bytes
    e.emit_op(Op::Return);
    let chunk = e.into_chunk();

    let err = expect_rejected(chunk, "out-of-chunk jump");
    assert!(
        err.contains("out-of-bounds"),
        "expected out-of-bounds jump rejection, got: {err}"
    );
}

/// An empty chunk must be rejected at load time: it has no terminator, so
/// activating it violates the pc-bounds invariant at pc 0 (today the VM's
/// runtime check catches that; rejecting up front keeps the invariant fully
/// verifier-backed). The compiler never emits an empty chunk (empty programs
/// compile to `Nil; Return`), so nothing legitimate is lost.
#[test]
fn semac_empty_chunk_rejected() {
    let chunk = Emitter::new().into_chunk();

    let err = expect_rejected(chunk, "empty chunk");
    assert!(
        err.contains("empty bytecode chunk"),
        "expected empty-chunk rejection, got: {err}"
    );
}

/// A pure LINEAR stack-growth sequence (no back-edge) must be rejected by the
/// `MAX_STACK_DEPTH` overflow bound specifically. The existing dup-overflow test
/// uses a self-loop, which trips the join-depth-disagreement check instead, so it
/// does not actually pin the overflow bound. This sequence has no join point, so
/// only the overflow check can reject it. (Found via mutation testing: disabling
/// the overflow bound left the loop-based test still green.)
#[test]
fn semac_linear_stack_overflow_rejected() {
    let mut e = Emitter::new();
    e.emit_const(sema_core::Value::int(1)).unwrap(); // depth 0 -> 1
    for _ in 0..70_000 {
        e.emit_op(Op::Dup); // each +1; well past MAX_STACK_DEPTH (65535)
    }
    e.emit_op(Op::Return);
    let chunk = e.into_chunk();

    let err = expect_rejected(chunk, "linear stack overflow");
    assert!(
        err.contains("maximum"),
        "expected stack-overflow (maximum depth) rejection, got: {err}"
    );
}

#[test]
fn semac_fixed_local_opcode_out_of_range_rejected() {
    let mut e = Emitter::new();
    e.emit_op(Op::LoadLocal3);
    e.emit_op(Op::Return);
    let mut chunk = e.into_chunk();
    chunk.n_locals = 3;

    let err = expect_rejected(chunk, "fixed local slot out of range");
    assert!(
        err.contains("fixed local slot 3") && err.contains("out of range"),
        "expected fixed-local rejection, got: {err}"
    );
}

#[test]
fn semac_global_cache_slot_out_of_range_rejected() {
    let mut e = Emitter::new();
    e.emit_op(Op::LoadGlobal);
    e.emit_u32(sema_core::intern("cache-test").into_inner().get());
    e.emit_u16(1);
    e.emit_op(Op::Return);
    let mut chunk = e.into_chunk();
    chunk.n_global_cache_slots = 1;

    let err = expect_rejected(chunk, "global cache slot out of range");
    assert!(
        err.contains("global cache slot 1") && err.contains("out of range"),
        "expected cache-slot rejection, got: {err}"
    );
}

#[test]
fn semac_make_closure_capture_shape_rejected() {
    let mut e = Emitter::new();
    e.emit_op(Op::MakeClosure);
    e.emit_u16(0);
    e.emit_u16(0);
    e.emit_op(Op::Return);
    let chunk = e.into_chunk();
    let result = CompileResult::new(chunk, vec![template(vec![UpvalueDesc::ParentLocal(0)])]);

    let err = expect_result_rejected(result, "MakeClosure upvalue count mismatch");
    assert!(
        err.contains("MakeClosure") && err.contains("encodes 0 upvalues"),
        "expected MakeClosure count rejection, got: {err}"
    );
}

#[test]
fn semac_make_closure_capture_bounds_rejected() {
    let mut e = Emitter::new();
    e.emit_op(Op::MakeClosure);
    e.emit_u16(0);
    e.emit_u16(1);
    e.emit_u16(1);
    e.emit_u16(0);
    e.emit_op(Op::Return);
    let chunk = e.into_chunk();
    let result = CompileResult::new(chunk, vec![template(vec![UpvalueDesc::ParentLocal(0)])]);

    let err = expect_result_rejected(result, "MakeClosure local capture out of range");
    assert!(
        err.contains("MakeClosure") && err.contains("captures local 0"),
        "expected MakeClosure capture rejection, got: {err}"
    );
}

#[test]
fn semac_handler_mid_instruction_rejected() {
    let mut e = Emitter::new();
    e.emit_op(Op::Nil);
    e.emit_op(Op::Throw);
    e.emit_op(Op::StoreLocal);
    e.emit_u16(0);
    e.emit_op(Op::Nil);
    e.emit_op(Op::Return);
    let mut chunk = e.into_chunk();
    chunk.n_locals = 1;
    chunk.exception_table.push(ExceptionEntry {
        try_start: 0,
        try_end: 2,
        handler_pc: 3,
        stack_depth: 1,
        catch_slot: 0,
    });

    let err = expect_rejected(chunk, "handler in instruction operand");
    assert!(
        err.contains("handler_pc 3") && err.contains("instruction boundary"),
        "expected handler-boundary rejection, got: {err}"
    );
}

#[test]
fn semac_reserved_flags_and_trailing_bytes_rejected() {
    let mut e = Emitter::new();
    e.emit_op(Op::Nil);
    e.emit_op(Op::Return);
    let bytes = serialize_to_bytes(&CompileResult::new(e.into_chunk(), vec![]), 0)
        .expect("serialize should succeed");

    let mut flagged = bytes.clone();
    flagged[6..8].copy_from_slice(&1u16.to_le_bytes());
    let flags_err = deserialize_error(&flagged, "reserved flags");
    assert!(
        flags_err.contains("header flags"),
        "unexpected error: {flags_err}"
    );

    let mut trailing = bytes;
    trailing.push(0);
    let trailer_err = deserialize_error(&trailing, "whole-file trailing bytes");
    assert!(
        trailer_err.contains("trailing bytes"),
        "unexpected error: {trailer_err}"
    );
}

#[test]
fn semac_string_table_trailing_bytes_rejected() {
    let mut e = Emitter::new();
    e.emit_op(Op::Nil);
    e.emit_op(Op::Return);
    let mut bytes = serialize_to_bytes(&CompileResult::new(e.into_chunk(), vec![]), 0)
        .expect("serialize should succeed");

    const HEADER_LEN: usize = 24;
    const SECTION_HEADER_LEN: usize = 6;
    let length_offset = HEADER_LEN + 2;
    let string_table_len = u32::from_le_bytes(
        bytes[length_offset..length_offset + 4]
            .try_into()
            .expect("string table section length"),
    ) as usize;
    let string_table_end = HEADER_LEN + SECTION_HEADER_LEN + string_table_len;
    bytes.insert(string_table_end, 0);
    bytes[length_offset..length_offset + 4]
        .copy_from_slice(&u32::try_from(string_table_len + 1).unwrap().to_le_bytes());

    let err = deserialize_error(&bytes, "string-table trailing bytes");
    assert!(
        err.contains("string table section") && err.contains("trailing"),
        "unexpected error: {err}"
    );
}
