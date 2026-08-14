# VM Optimization: Raw Stack Operations for Immediate Values

**Status:** ✅ Implemented  
**Priority:** High — targets the hottest code in the VM dispatch loop  
**Expected impact:** 10-25% on integer-heavy benchmarks (tak, fib), 5-10% on mixed (deriv)  
**Actual impact:** ~1.7% on tak, neutral on deriv — less than expected because the call dispatch path dominates tak's runtime, not the arithmetic ops themselves

---

## Problem

The specialized int opcodes (`AddInt`, `SubInt`, `MulInt`, `LtInt`, `EqInt`) are the highest-frequency instructions in arithmetic benchmarks. Each one currently does:

```rust
let b = self.stack.pop().unwrap();  // bounds check + Clone + Drop
let a = self.stack.pop().unwrap();  // bounds check + Clone + Drop
if let (Some(x), Some(y)) = (a.as_int(), b.as_int()) {
    self.stack.push(Value::int(x.wrapping_add(y)));  // bounds check + potential realloc
}
// a and b are dropped here — Drop::drop does tag check + match (no-op for ints, but compiled anyway)
```

**Per-instruction overhead (int fast path):**
1. 2× `Vec::pop()` — bounds check, length decrement, returns `Option<Value>`
2. 2× `Clone::clone()` — `is_boxed` check + tag match (7 immediate tags) + bitwise copy
3. 2× `as_int()` — `is_boxed` + `get_tag` + sign extension
4. 1× `Value::int()` — range check + `make_boxed`
5. 1× `Vec::push()` — bounds check, length increment
6. 2× `Drop::drop()` — `is_boxed` check + tag match (no-op for ints, but code is still emitted)

Since `Value` is `#[repr(transparent)]` over `u64`, and small ints are immediates (no heap pointer), we can operate directly on the stack's backing array using `unsafe` pointer ops — eliminating Clone/Drop entirely.

## Solution

Replace pop/push with direct stack slot manipulation using `unsafe`:

```rust
37 /* AddInt */ => {
    let len = self.stack.len();
    let a_bits = unsafe { (*self.stack.as_ptr().add(len - 2)).raw_bits() };
    let b_bits = unsafe { (*self.stack.as_ptr().add(len - 1)).raw_bits() };
    // Check both are small ints: boxed + TAG_INT_SMALL (tag 3)
    if (a_bits & TAG_INT_SMALL_MASK) == TAG_INT_SMALL_PATTERN
        && (b_bits & TAG_INT_SMALL_MASK) == TAG_INT_SMALL_PATTERN
    {
        // Extract payloads, compute, write result directly
        let a_payload = a_bits & PAYLOAD_MASK;
        let b_payload = b_bits & PAYLOAD_MASK;
        // For addition: add payloads, mask to 45 bits, re-tag
        let sum = (a_payload.wrapping_add(b_payload)) & PAYLOAD_MASK;
        let result_bits = BOX_MASK | (TAG_INT_SMALL << 45) | sum;
        // Overwrite stack[len-2] with result, shrink by 1
        // No Clone needed (ints are immediate), no Drop needed
        unsafe {
            std::ptr::write(self.stack.as_mut_ptr().add(len - 2), Value::from_raw(result_bits));
            self.stack.set_len(len - 1);
        }
    } else {
        // Slow path: pop, clone, etc.
    }
}
```

## Key technical details

- Need to expose `Value::from_raw(bits: u64) -> Value` constructor (trivial: `Value(bits)`)
- Need to expose NaN-boxing constants as public: `BOX_MASK`, `PAYLOAD_MASK`, `TAG_INT_SMALL`, `INT_SIGN_BIT`
- For the int fast-path check, we can create a combined mask: `bits & 0xFFFF_E000_0000_0000 == 0xFFF9_8000_0000_0000` (BOX_MASK | TAG_INT_SMALL << 45)
- Since ints are immediates, `std::ptr::write` won't leak (no Drop needed for old value)
- `set_len(len-1)` doesn't run Drop on the removed element — that's correct since we consumed both operands by writing the result over `stack[len-2]` and the value at `stack[len-1]` is an int (no drop needed)

## Safety argument

- `stack[len-2]` and `stack[len-1]` are valid because we just checked `len >= 2` (the compiler guarantees the stack has enough operands for each opcode)
- We only skip Drop for values proven to be small ints (tag check)
- `set_len(len-1)` is safe because: the value at `stack[len-1]` is an int (immediate, no destructor), and `len-1 <= capacity`
- `ptr::write` overwrites `stack[len-2]` — the old value was also an int (verified), so no leak

## Files to modify

- `crates/sema-core/src/value.rs` — add `from_raw()`, expose constants
- `crates/sema-vm/src/vm.rs` — rewrite AddInt/SubInt/MulInt/LtInt/EqInt fast paths

## Verification

```bash
cargo test -p sema-vm --lib
cargo test -p sema --test vm_integration_test
cargo test -p sema --test integration_test
cargo build --release && hyperfine --runs 10 --warmup 3 \
  "./target/release/sema --vm --no-llm examples/benchmarks/tak.sema" \
  "./target/release/sema --vm --no-llm examples/benchmarks/deriv.sema"
```
