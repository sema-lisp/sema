# VM Optimization: Threaded Dispatch

**Status:** Research needed — Rust limitations apply  
**Priority:** Low-Medium  
**Expected impact:** 5-15%, highly platform-dependent

---

## Problem

The VM dispatch loop uses a `match op { 0 => ..., 1 => ..., ... }` statement. LLVM typically compiles this as either:
- A **jump table** (indirect jump through table of addresses) — good for dense ranges
- A **binary search tree** of comparisons — for sparse ranges

The problem is that the CPU's branch predictor sees a single indirect branch site (the match), making prediction difficult when opcodes alternate unpredictably.

## Solution: Direct/Indirect Threading

### Option 1: Tail-call threading (Rust-compatible)

Replace the match with a function pointer dispatch table:

```rust
type OpHandler = fn(&mut VM, &EvalContext, *const u8, usize, usize, usize) 
    -> Result<DispatchResult, SemaError>;

static DISPATCH_TABLE: [OpHandler; 46] = [
    handle_const,   // 0
    handle_nil,     // 1
    handle_true,    // 2
    // ...
];

fn run(&mut self, ctx: &EvalContext) -> Result<Value, SemaError> {
    // outer loop for frame changes
    'dispatch: loop {
        let fi = self.frames.len() - 1;
        let code = ...;
        let base = ...;
        let mut pc = ...;
        
        loop {
            let op = unsafe { *code.add(pc) } as usize;
            pc += 1;
            match DISPATCH_TABLE[op](self, ctx, code, pc, base, fi)? {
                DispatchResult::Continue(new_pc) => pc = new_pc,
                DispatchResult::FrameChange => continue 'dispatch,
                DispatchResult::Return(val) => return Ok(val),
            }
        }
    }
}
```

**Issue:** This breaks the current architecture where cached locals (`pc`, `base`, `fi`) are stack-local variables. Each handler would need to receive and return them, adding overhead that may negate the benefit.

### Option 2: Computed goto via inline assembly (platform-specific)

Use `asm!()` for `goto *table[op]`. This is what LuaJIT and CPython do in C via GCC's `goto *label_table[op]`. Extremely platform-specific and would require `#[cfg(target_arch)]` handling.

### Option 3: Use nightly `#[feature(label_break_value)]` or similar

Not available in stable Rust. Not viable.

## Analysis: Is this worth it?

The current match on `u8` with ~46 arms compiles to a jump table (LLVM is good at this). The main benefit of threading is that each handler ends with a direct jump to the next handler (the CPU sees N different branch sites instead of 1), improving branch prediction.

However:
- Rust's function-pointer approach adds call/return overhead that may negate the prediction benefit
- The current match-based dispatch is already well-optimized by LLVM
- The biggest wins come from reducing work per instruction, not dispatch mechanism

**Recommendation:** Low priority. Focus on reducing per-instruction work (raw stack ops, superinstructions, call caching) first. Consider only if profiling shows dispatch overhead > 20% of total time.

## Files to modify

- `crates/sema-vm/src/vm.rs` — major restructure of `run()`

## Risks

- High complexity, hard to maintain
- May actually be slower due to Rust's safety/calling convention overhead
- Platform-dependent benefits
- Would prevent LLVM from optimizing across opcode handlers (current match allows it)

## Verification

Same test suite + benchmark, with platform-specific profiling (perf stat for branch mispredictions).
