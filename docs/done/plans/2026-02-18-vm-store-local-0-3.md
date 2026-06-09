# VM Optimization: StoreLocal0..3 Specialized Opcodes

**Status:** Ready to implement  
**Priority:** Low — smaller impact than LoadLocal0..3  
**Expected impact:** 1-3%

---

## Problem

`StoreLocal` (opcode 7) uses a u16 operand for the slot index. Like `LoadLocal`, the most common slots are 0-3. Each StoreLocal instruction costs 3 bytes (opcode + u16) instead of 1 byte.

## Solution

Add `StoreLocal0..3` opcodes (zero-operand, single byte):

```
StoreLocal0,  // = 46
StoreLocal1,  // = 47
StoreLocal2,  // = 48
StoreLocal3,  // = 49
```

Each pops TOS and writes to `stack[base + N]`, checking open_upvalues if present.

## Impact analysis

StoreLocal is less frequent than LoadLocal in typical Lisp code:
- `LoadLocal` happens every time a variable is referenced (very frequent)
- `StoreLocal` happens only for `set!`, `do` step expressions, and internal let bindings

In tak, there are ~0 StoreLocal instructions (no mutation). In deriv, StoreLocal is rare. The main beneficiaries would be programs with `do` loops or heavy mutation.

## Files to modify

- `crates/sema-vm/src/opcodes.rs` — add opcodes 46-49
- `crates/sema-vm/src/compiler.rs` — emit in `compile_var_store`
- `crates/sema-vm/src/vm.rs` — dispatch handlers
- `crates/sema-vm/src/disasm.rs` — disassembly names

## Verification

```bash
cargo test -p sema-vm --lib
cargo test -p sema --test vm_integration_test
```
