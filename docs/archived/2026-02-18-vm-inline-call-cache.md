# VM Optimization: Inline Call-Site Caching

**Status:** Design needed  
**Priority:** Medium-High  
**Expected impact:** 10-20% on call-heavy benchmarks (tak)

---

## Problem

Every `Call` and `TailCall` instruction goes through `call_value` which:

1. Reads `self.stack[func_idx]` 
2. Checks `raw_tag() == 15` (is it a NativeFn?)
3. Borrows via `as_native_fn_ref()`
4. Checks `native.payload.is_some()`
5. Calls `payload.downcast_ref::<VmClosurePayload>()`
6. Clones `vmc.closure` (Rc bump) and `vmc.functions` (Rc bump)
7. Finally enters `call_vm_closure_from_rc`

In tak, 100% of calls are to the same function (`tak` itself, resolved via LoadGlobal). Steps 2-6 are pure overhead — we already know the answer from the first call.

## Solution: Monomorphic Inline Cache (MIC)

At each `Call`/`TailCall` site in the bytecode, cache the last callee identity and its resolved closure. On subsequent calls, compare the callee's raw bits — if identical, skip all dispatch and jump directly to the frame setup.

### Approach A: Per-VM call cache (simpler)

```rust
pub struct VM {
    // ... existing fields ...
    /// Last call-site cache: (callee_raw_bits, closure, functions)
    call_cache: Option<(u64, Rc<Closure>, Rc<Vec<Rc<Function>>>)>,
}
```

In `call_value`:
```rust
let callee_bits = self.stack[func_idx].raw_bits();
if let Some((cached_bits, ref closure, ref functions)) = self.call_cache {
    if callee_bits == cached_bits {
        self.functions = functions.clone();
        return self.call_vm_closure_from_rc(closure, argc);
    }
}
// ... slow path, then cache the result ...
```

**Limitation:** Single-entry cache. For tak (which only calls `tak`), this is perfect. For programs that alternate between different callees at the same site, it thrashes.

### Approach B: Bytecode-patched inline cache (advanced)

Rewrite the Call instruction's operand bytes to encode a "cached function ID" after first resolution. This is how V8 and LuaJIT work, but requires mutable bytecode and more complexity.

### Approach C: Per-call-site cache array

Store an array of `(pc, callee_bits, closure, functions)` tuples. Index by the call-site pc. Direct-mapped with 16-32 entries.

## Recommended: Approach A first

Single-entry cache is trivial to implement and perfect for recursive benchmarks. Can upgrade to per-site cache later if profiling shows thrashing.

## Files to modify

- `crates/sema-vm/src/vm.rs` — add cache field, check in call_value/tail_call_value

## Risks

- Low risk: cache is purely an optimization, correctness unaffected (cache miss = normal path)
- The Rc clone of `closure` and `functions` on cache population still costs 2 refcount bumps, but this happens once per unique callee, not per call
- For polymorphic call sites, the cache miss penalty (check + miss + slow path) is slightly worse than no cache

## Verification

```bash
cargo test -p sema-vm --lib
cargo test -p sema --test vm_integration_test
cargo build --release && hyperfine --runs 10 --warmup 3 \
  "./target/release/sema --vm --no-llm examples/benchmarks/tak.sema"
```
