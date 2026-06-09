# Magic Number Cleanup Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace all raw magic numbers across the VM, compiler, and value system with named constants for readability and maintainability.

**Architecture:** Export additional NaN-boxing tag constants from `sema-core`. Add instruction-size constants to the opcodes module. Replace all raw numeric literals with their named equivalents.

**Tech Stack:** Rust, sema-core (NaN-boxing constants), sema-vm (opcodes, compiler, VM dispatch)

---

## Summary of Issues Found

| Category | Location | Example | Fix |
|---|---|---|---|
| Raw tag numbers | `vm.rs:773,839` | `raw_tag() == Some(15)` | Export `TAG_NATIVE_FN` from sema-core |
| PC offset magic | `vm.rs:264,311,323` | `pc - 5`, `pc - 3` | Use instruction-size constants |
| Bit-width magic | `vm.rs:151-152` | `const PAYLOAD_BITS: u32 = 45` | Import from sema-core |
| `0x3F` mask | `value.rs:321` | `(bits >> 45) & 0x3F` | Use named constant |
| `Op::from_u8` | `opcodes.rs:82-132` | Manual 0..45 → Op map | Derive or use `as u8` round-trip |

---

### Task 1: Export `TAG_NATIVE_FN` from sema-core

**Files:**
- Modify: `crates/sema-core/src/value.rs:279` (make public)
- Modify: `crates/sema-core/src/lib.rs:14-18` (add to re-exports)

**Step 1: Make TAG_NATIVE_FN public in value.rs**

Change line 279 from:
```rust
const TAG_NATIVE_FN: u64 = 15;
```
to:
```rust
pub const TAG_NATIVE_FN: u64 = 15;
```

**Step 2: Re-export from lib.rs**

Add `TAG_NATIVE_FN` to the `pub use value::{...}` block in `crates/sema-core/src/lib.rs`.

**Step 3: Replace raw `15` in vm.rs**

In `crates/sema-vm/src/vm.rs`, replace both occurrences of:
```rust
self.stack[func_idx].raw_tag() == Some(15)
```
with:
```rust
self.stack[func_idx].raw_tag() == Some(TAG_NATIVE_FN)
```

Add `TAG_NATIVE_FN` to the import from `sema_core`.

**Step 4: Run tests**

Run: `cargo test -p sema-vm && cargo test -p sema --test integration_test`
Expected: All pass.

**Step 5: Commit**

```bash
git add crates/sema-core/src/value.rs crates/sema-core/src/lib.rs crates/sema-vm/src/vm.rs
git commit -m "refactor: replace raw tag number 15 with TAG_NATIVE_FN constant"
```

---

### Task 2: Add instruction-size constants to opcodes module

**Files:**
- Modify: `crates/sema-vm/src/opcodes.rs` (add constants to `mod op`)

**Step 1: Add instruction size constants**

Add to the `pub mod op` block in `opcodes.rs`:

```rust
// Instruction sizes (opcode byte + operand bytes)
/// Size of an instruction with a u16 operand (e.g., CALL, LOAD_LOCAL): 1 + 2 = 3
pub const SIZE_OP_U16: usize = 3;
/// Size of an instruction with a u32 operand (e.g., LOAD_GLOBAL): 1 + 4 = 5
pub const SIZE_OP_U32: usize = 5;
/// Size of a bare opcode with no operands: 1
pub const SIZE_OP: usize = 1;
```

**Step 2: Run tests**

Run: `cargo test -p sema-vm`
Expected: All pass (additive change).

**Step 3: Commit**

```bash
git add crates/sema-vm/src/opcodes.rs
git commit -m "refactor: add instruction-size constants to opcodes module"
```

---

### Task 3: Replace PC offset magic numbers in vm.rs

**Files:**
- Modify: `crates/sema-vm/src/vm.rs` (lines 264, 311, 323, 345, 396, etc.)

**Step 1: Replace `pc - 5` with `pc - op::SIZE_OP_U32`**

Line 264 (`LOAD_GLOBAL` error path): The instruction is opcode(1) + u32(4) = 5 bytes.
```rust
// Before:
match self.handle_exception(err, pc - 5)? {
// After:
match self.handle_exception(err, pc - op::SIZE_OP_U32)? {
```

**Step 2: Replace `pc - 3` with `pc - op::SIZE_OP_U16`**

Lines 311, 323 (`CALL`, `TAIL_CALL`): Each is opcode(1) + u16(2) = 3 bytes.
```rust
// Before:
let saved_pc = pc - 3;
// After:
let saved_pc = pc - op::SIZE_OP_U16;
```

**Step 3: Replace `pc - 1` with `pc - op::SIZE_OP`**

Lines 345, 396, 410, 424, 438, 452, 468 (bare opcodes like `THROW`, `NEGATE`, arithmetic ops):
```rust
// Before:
match self.handle_exception(err, pc - 1)? {
// After:
match self.handle_exception(err, pc - op::SIZE_OP)? {
```

Also line 345 (`MAKE_CLOSURE`):
```rust
// Before:
self.frames[fi].pc = pc - 1;
// After:
self.frames[fi].pc = pc - op::SIZE_OP;
```

**Step 4: Run tests**

Run: `cargo test -p sema-vm && cargo test -p sema --test integration_test`
Expected: All pass.

**Step 5: Commit**

```bash
git add crates/sema-vm/src/vm.rs
git commit -m "refactor: replace PC offset magic numbers with instruction-size constants"
```

---

### Task 4: Replace PAYLOAD_BITS / SIGN_SHIFT with sema-core imports

**Files:**
- Modify: `crates/sema-core/src/value.rs` (export payload bit width)
- Modify: `crates/sema-core/src/lib.rs` (re-export)
- Modify: `crates/sema-vm/src/vm.rs:151-152` (use imported constant)

**Step 1: Export payload bit width from sema-core**

In `crates/sema-core/src/value.rs`, add a public constant:
```rust
/// Number of payload bits in NaN-boxed values (45).
pub const NAN_PAYLOAD_BITS: u32 = 45;
```

Add to the re-export in `lib.rs`.

**Step 2: Replace local constants in vm.rs**

Replace lines 151-152:
```rust
// Before:
const PAYLOAD_BITS: u32 = 45;
const SIGN_SHIFT: u32 = 64 - PAYLOAD_BITS;
// After:
const SIGN_SHIFT: u32 = 64 - NAN_PAYLOAD_BITS;
```

Add `NAN_PAYLOAD_BITS` to the sema_core import.

**Step 3: Run tests**

Run: `cargo test -p sema-vm && cargo test -p sema --test integration_test`
Expected: All pass.

**Step 4: Commit**

```bash
git add crates/sema-core/src/value.rs crates/sema-core/src/lib.rs crates/sema-vm/src/vm.rs
git commit -m "refactor: import NAN_PAYLOAD_BITS from sema-core instead of hardcoding 45"
```

---

### Task 5: Use named constant for `0x3F` tag mask in value.rs

**Files:**
- Modify: `crates/sema-core/src/value.rs:321`

**Step 1: Add a named constant for the 6-bit tag field width**

Near the existing NaN-boxing constants (around line 258), add:
```rust
/// 6-bit mask for extracting the tag from a boxed value (bits 50-45).
const TAG_MASK_6BIT: u64 = 0x3F;
```

**Step 2: Use it in `get_tag`**

```rust
// Before:
fn get_tag(bits: u64) -> u64 {
    (bits >> 45) & 0x3F
}
// After:
fn get_tag(bits: u64) -> u64 {
    (bits >> 45) & TAG_MASK_6BIT
}
```

Also in the `NAN_TAG_MASK` definition (line 296):
```rust
// Before:
pub const NAN_TAG_MASK: u64 = BOX_MASK | (0x3F << 45);
// After:
pub const NAN_TAG_MASK: u64 = BOX_MASK | (TAG_MASK_6BIT << 45);
```

**Step 3: Run tests**

Run: `cargo test -p sema-core && cargo test -p sema --test integration_test`
Expected: All pass.

**Step 4: Commit**

```bash
git add crates/sema-core/src/value.rs
git commit -m "refactor: replace 0x3F magic number with TAG_MASK_6BIT constant"
```

---

### Task 6 (Optional): Derive `Op::from_u8` via macro instead of manual match

**Files:**
- Modify: `crates/sema-vm/src/opcodes.rs:82-132`

This task is lower priority — the manual mapping is correct and complete but fragile (adding a new opcode requires updating both the enum and the match). A simple alternative is a `#[repr(u8)]`-based transmute with a bounds check:

```rust
impl Op {
    pub fn from_u8(byte: u8) -> Option<Op> {
        if byte <= Op::LoadLocal3 as u8 {
            Some(unsafe { std::mem::transmute::<u8, Op>(byte) })
        } else {
            None
        }
    }
}
```

This only works because the enum has `#[repr(u8)]` and variants are dense (0..=45). If the enum ever has gaps, this breaks — so this is a tradeoff. Document the invariant.

**Step 1: Replace the manual match with transmute + bounds check**

**Step 2: Run tests**

Run: `cargo test -p sema-vm && cargo test -p sema --test integration_test`
Expected: All pass.

**Step 3: Commit**

```bash
git add crates/sema-vm/src/opcodes.rs
git commit -m "refactor: simplify Op::from_u8 with transmute on repr(u8) enum"
```

---

## Not Addressed (Acceptable)

These patterns were reviewed but are **not** magic numbers worth replacing:

- **`Vec::with_capacity(256)` / `with_capacity(64)`** — performance tuning hints, not semantic constants.
- **`LoadLocal0..3` using literal `0, 1, 2, 3`** — these are the *definition* of those opcodes, not magic numbers.
- **PC arithmetic in compiler.rs `patch_closure_func_ids`** — uses `Op` enum matching, already clean. The `+ 1 + 2 + 2` offsets could use `SIZE_*` constants but the comments explain the layout inline; optional cleanup.
- **`0x7` alignment mask in `ptr_to_payload`** — standard 8-byte alignment check, idiomatic.
