# VM Optimization: Superinstructions

**Status:** Deprioritized — intrinsic recognition (completed) delivered 20-71% gains; superinstructions are incremental on top  
**Priority:** Low  
**Expected impact:** 5-15% on dispatch-bound benchmarks (now that intrinsics handle the bigger win)

---

## Problem

Each bytecode instruction incurs dispatch overhead: fetch opcode byte, branch on match arm, advance pc. For hot loops with predictable instruction sequences, this overhead is significant. Modern interpreters (Lua, CPython 3.11+) fuse common pairs/triples into single "superinstructions."

## Analysis: tak instruction sequences

The `tak` function compiles to roughly:

```
LOAD_LOCAL_1       ; y
LOAD_LOCAL_0       ; x
LT_INT             ; (< y x)
NOT
JUMP_IF_FALSE      ; if branch
LOAD_LOCAL_2       ; z (then branch — return z)
RETURN
; else branch: 3 recursive calls
LOAD_GLOBAL tak    ; (tak ...)
LOAD_GLOBAL -
LOAD_LOCAL_0       ; x
CONST 1
SUB_INT            ; (- x 1)
LOAD_LOCAL_1       ; y
LOAD_LOCAL_2       ; z
CALL 3             ; (tak (- x 1) y z)
; ... similar for other two calls
TAIL_CALL 3
```

**Candidate superinstructions:**

| Fused sequence | Frequency in tak | Savings |
|---|---|---|
| `LoadLocal_N + LoadLocal_M + LtInt` | 1× per call (31.8M) | 2 dispatches |
| `LoadLocal_N + Const + SubInt` | 3× per call (95.4M) | 2 dispatches |
| `Not + JumpIfFalse` → `JumpIfTrue` | 1× per call (31.8M) | 1 dispatch (already exists as opcode!) |
| `LoadLocal_N + Return` | 1× per call | 1 dispatch |

The `Not + JumpIfFalse` case is interesting — we already have `JumpIfTrue` (opcode 15). The compiler should emit `JumpIfTrue` instead of `Not + JumpIfFalse` for `(if (not cond) ...)`.

## Approach

### Phase 1: Peephole optimization in compiler ✅ DONE
Before adding new opcodes, check if the compiler can use existing opcodes better:
- `(not (< ...))` should emit `LtInt + JumpIfTrue` not `LtInt + Not + JumpIfFalse`
- ✅ Implemented: `(if (not X) ...)` compiles to `JumpIfTrue` (commit f734515)
- ✅ Intrinsic recognition also handles this: `<` now compiles to `LtInt` directly, and `not` to `Not`

### Phase 2: Add superinstructions
Add fused opcodes for the most common sequences:
```
LoadLocal_SubInt1   ; LoadLocal(slot) + Const(1) + SubInt → single opcode with u16 slot
LoadLocal2_LtInt    ; LoadLocal(a) + LoadLocal(b) + LtInt → single opcode with 2× u16 slots
```

### Phase 3: Instruction frequency profiling
Add a debug-only instruction counter to the VM to measure actual instruction mix on real programs. Use this data to identify the highest-value superinstructions.

## Files to modify

- `crates/sema-vm/src/opcodes.rs` — new opcodes
- `crates/sema-vm/src/compiler.rs` — peephole optimization pass and/or direct emission
- `crates/sema-vm/src/vm.rs` — dispatch handlers
- `crates/sema-vm/src/disasm.rs` — disassembly

## Risks

- Each new opcode increases the match arms in dispatch, potentially worsening branch prediction for non-fused paths
- Diminishing returns: each superinstruction only saves ~1ns per dispatch eliminated
- Compiler complexity increases with peephole patterns

## Verification

Same as other VM optimizations — full test suite + benchmark comparison.
