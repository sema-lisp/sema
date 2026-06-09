# VM Performance Roadmap: Closing the Gap with Janet

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Bring Sema's bytecode VM within 2-3x of Janet's performance on compute-heavy benchmarks (currently 7.8x slower on TAK).

**Architecture:** Sema's VM is a stack-based bytecode interpreter with variable-length byte-encoded instructions. Janet uses a register-based VM with fixed-width 32-bit instructions, computed gotos, flat stack frames, lazy upvalue capture, and a tracing GC. This plan addresses the highest-impact performance gaps in priority order through 6 phases.

**Tech Stack:** Rust, NaN-boxed `Value(u64)`, `sema-vm` crate

**Current benchmark (tak 500 iterations, hyperfine 5 runs, Apple Silicon):**

| Implementation   | Time   | vs Janet |
| ---------------- | ------ | -------- |
| Janet 1.x        | 1.19s  | 1.0x     |
| Sema VM          | 9.26s  | 7.8x     |
| Sema tree-walker | 19.32s | 16.2x    |

---

## Analysis: Where the Time Goes

Profiling `tak(18,12,6)` × 500 iterations (~22M function calls, ~66M arithmetic ops):

| Bottleneck          | Janet approach                                          | Sema VM current                                                               | Est. impact |
| ------------------- | ------------------------------------------------------- | ----------------------------------------------------------------------------- | ----------- |
| **Dispatch**        | Computed gotos, 32-bit fixed-width instructions         | `match` on byte opcode, variable-length encoding                              | ~2x         |
| **Arithmetic**      | Inline NaN-boxed f64 ops, no branching for common case  | `view()` pattern match → reconstruct Value                                    | ~1.5x       |
| **Call frames**     | Pointer bump on flat `Janet *data` array, no heap alloc | `Vec::push(CallFrame)` with `Rc<Closure>` clone, `vec![None; n]` for upvalues | ~2x         |
| **Variable access** | `stack[A]` — direct register index in instruction word  | `self.stack[base + slot]` — extra indirection through `self.frames[fi]`       | ~1.3x       |
| **Tail calls**      | Overwrite current frame in-place, reset pc              | Clone closure Rc, rebuild upvalue vec, resize stack                           | ~1.5x       |
| **Clone/Drop**      | No refcounting (tracing GC)                             | `Value::clone()` bumps Rc on every push/load, Drop on every pop               | ~1.5x       |

These multiply together: 2 × 1.5 × 2 × 1.3 × 1.5 × 1.5 ≈ **17x** theoretical overhead, which aligns with the measured 7.8x gap (some overlap between categories).

---

## Phase 1: Hot-Path Arithmetic Without `view()` (Est. 1.5-2x speedup)

**Problem:** Every `Add`/`Sub`/`Mul`/`Lt` calls `a.view()` and `b.view()` which decodes the NaN-box, and for heap types reconstructs an `Rc` (bumping refcount) into a `ValueView` enum, only to immediately pattern-match it. For the common int+int case, this is ~10 instructions of overhead per operand.

**Solution:** Add `try_as_small_int()` fast-path that checks the NaN-box tag bits directly and extracts the i64 without constructing a `ValueView`.

**Files:**

- Modify: `crates/sema-core/src/value.rs` — add `try_as_small_int(&self) -> Option<i64>` inline method
- Modify: `crates/sema-vm/src/vm.rs` — rewrite `vm_add`, `vm_sub`, `vm_mul`, `vm_lt`, and dispatch sites

### Task 1.1: Add `try_as_small_int()` to Value

**Step 1:** Add method to `Value` in `crates/sema-core/src/value.rs`:

```rust
/// Fast-path: extract small int without constructing ValueView.
/// Returns None if not a small int (big ints, floats, non-numbers).
#[inline(always)]
pub fn try_as_small_int(&self) -> Option<i64> {
    let bits = self.0;
    if bits & TAG_MASK == make_boxed_const(TAG_INT_SMALL, 0) & TAG_MASK {
        // Sign-extend 45-bit payload
        let payload = (bits & PAYLOAD_MASK) as i64;
        let shifted = payload << (64 - 45);
        Some(shifted >> (64 - 45))
    } else {
        None
    }
}
```

**Step 2:** Write a unit test in the same file's `#[cfg(test)]` module:

```rust
#[test]
fn test_try_as_small_int() {
    assert_eq!(Value::int(42).try_as_small_int(), Some(42));
    assert_eq!(Value::int(-1).try_as_small_int(), Some(-1));
    assert_eq!(Value::int(0).try_as_small_int(), Some(0));
    assert_eq!(Value::float(42.0).try_as_small_int(), None);
    assert_eq!(Value::nil().try_as_small_int(), None);
    assert_eq!(Value::bool(true).try_as_small_int(), None);
    assert_eq!(Value::string("hi").try_as_small_int(), None);
}
```

**Step 3:** Run: `cargo test -p sema-core -- test_try_as_small_int`

**Step 4:** Commit: `perf(vm): add try_as_small_int() fast-path on Value`

### Task 1.2: Fast-path arithmetic in VM

**Step 1:** Rewrite `vm_add` (and similarly `vm_sub`, `vm_mul`, `vm_lt`) in `crates/sema-vm/src/vm.rs`:

```rust
#[inline(always)]
fn vm_add(a: &Value, b: &Value) -> Result<Value, SemaError> {
    // Fast path: small int + small int (covers ~95% of arithmetic in benchmarks)
    if let (Some(x), Some(y)) = (a.try_as_small_int(), b.try_as_small_int()) {
        return Ok(Value::int(x.wrapping_add(y)));
    }
    // Slow path: full view() dispatch
    vm_add_slow(a, b)
}

#[cold]
fn vm_add_slow(a: &Value, b: &Value) -> Result<Value, SemaError> {
    use sema_core::ValueView;
    match (a.view(), b.view()) {
        (ValueView::Int(x), ValueView::Int(y)) => Ok(Value::int(x.wrapping_add(y))),
        (ValueView::Float(x), ValueView::Float(y)) => Ok(Value::float(x + y)),
        (ValueView::Int(x), ValueView::Float(y)) => Ok(Value::float(x as f64 + y)),
        (ValueView::Float(x), ValueView::Int(y)) => Ok(Value::float(x + y as f64)),
        (ValueView::String(x), ValueView::String(y)) => {
            let mut s = (*x).clone();
            s.push_str(&y);
            Ok(Value::string(&s))
        }
        _ => Err(SemaError::type_error(
            "number or string",
            format!("{} and {}", a.type_name(), b.type_name()),
        )),
    }
}
```

**Step 2:** Apply same pattern to `vm_sub`, `vm_mul`, `vm_lt`.

**Step 3:** Run: `cargo test -p sema --test vm_integration_test`

**Step 4:** Benchmark: `hyperfine --runs 5 --warmup 1 -n "vm: tak" "./target/release/sema --no-llm --vm examples/benchmarks/tak.sema"`

**Step 5:** Commit: `perf(vm): fast-path small-int arithmetic bypassing view()`

---

## Phase 2: Eliminate Per-Call Heap Allocations (Est. 1.5-2x speedup)

**Problem:** Every `call_vm_closure` does:

1. `Rc::clone(&closure)` — atomic refcount bump
2. `vec![None; n_locals]` — heap allocation for `open_upvalues`
3. `Vec::push(CallFrame)` — may reallocate `frames` vec

For `tak(18,12,6)` with 22M calls, that's 22M `Vec` allocations for upvalue slots that are almost never used.

**Solution:** Lazy upvalue allocation + pre-allocated frame pool.

**Files:**

- Modify: `crates/sema-vm/src/vm.rs` — `CallFrame`, `call_vm_closure`, `tail_call_vm_closure`, `LoadLocal`, `StoreLocal`, `MakeClosure`

### Task 2.1: Lazy upvalue Vec allocation

**Step 1:** Change `CallFrame.open_upvalues` from `Vec<Option<Rc<UpvalueCell>>>` to `Option<Vec<Option<Rc<UpvalueCell>>>>`:

```rust
struct CallFrame {
    closure: Rc<Closure>,
    pc: usize,
    base: usize,
    n_locals: usize, // cache for lazy alloc sizing
    open_upvalues: Option<Vec<Option<Rc<UpvalueCell>>>>,
}
```

**Step 2:** In `call_vm_closure` and `tail_call_vm_closure`, set `open_upvalues: None` instead of `vec![None; n_locals]`.

**Step 3:** In `MakeClosure` opcode (opcode 19), when a local is captured, lazily initialize:

```rust
let upvalues = frame.open_upvalues.get_or_insert_with(|| vec![None; frame.n_locals]);
```

**Step 4:** Update `LoadLocal`/`StoreLocal` to only check upvalues when `open_upvalues.is_some()`:

```rust
// LoadLocal fast path
6 => {
    let slot = read_u16_inline!(code, pc) as usize;
    self.frames[fi].pc = pc;
    let val = if self.frames[fi].open_upvalues.is_some() {
        if let Some(Some(cell)) = self.frames[fi].open_upvalues.as_ref().unwrap().get(slot) {
            cell.value.borrow().clone()
        } else {
            self.stack[base + slot].clone()
        }
    } else {
        self.stack[base + slot].clone()
    };
    self.stack.push(val);
}
```

**Step 5:** Run: `cargo test -p sema --test vm_integration_test`

**Step 6:** Benchmark against previous.

**Step 7:** Commit: `perf(vm): lazy upvalue vec allocation — skip heap alloc when no closures captured`

### Task 2.2: Inline CallFrame fields to avoid Rc clone

**Step 1:** Instead of storing `Rc<Closure>` in the call frame, store the raw components:

```rust
struct CallFrame {
    func: Rc<Function>,           // shared, Rc clone is cheap
    upvalues: Rc<Vec<Rc<UpvalueCell>>>,  // shared from closure
    pc: usize,
    base: usize,
    n_locals: usize,
    open_upvalues: Option<Vec<Option<Rc<UpvalueCell>>>>,
}
```

This avoids cloning the `Rc<Closure>` wrapper and lets us access `func` and `upvalues` directly.

**Step 2:** Update all sites that access `self.frames[fi].closure.func` → `self.frames[fi].func` and `self.frames[fi].closure.upvalues` → `self.frames[fi].upvalues`.

**Step 3:** Run tests, benchmark, commit: `perf(vm): inline closure fields in CallFrame`

---

## Phase 3: Register-Based Instruction Encoding (Est. 1.5-2x speedup)

**Problem:** Sema's VM is stack-based — every operation pops operands and pushes results. This means `(+ a b)` compiles to `LoadLocal 0 / LoadLocal 1 / Add` — 3 instructions with 3 stack pushes and 2 pops. Janet encodes this as a single instruction: `add dest, src1, src2` with register indices packed into a 32-bit word.

**Solution:** Move from stack-based variable-length byte encoding to register-based fixed-width 32-bit instructions.

> **Note:** This is the biggest single change and touches the compiler, instruction encoding, and entire VM dispatch loop. It should be done incrementally — start with a hybrid approach.

**Files:**

- Modify: `crates/sema-vm/src/chunk.rs` — new `Op32` instruction format
- Modify: `crates/sema-vm/src/compiler.rs` — register allocator, emit 32-bit instructions
- Modify: `crates/sema-vm/src/vm.rs` — new dispatch loop
- Modify: `crates/sema-vm/src/disasm.rs` — updated disassembler
- Modify: `crates/sema-vm/src/emit.rs` — new emitter for 32-bit ops

### Task 3.1: Design the instruction format

Janet's format (reference):

```
 31       24 23    16 15      8 7       0
┌──────────┬────────┬─────────┬─────────┐
│   D/C    │   B    │    A    │ opcode  │
└──────────┴────────┴─────────┴─────────┘

Formats:
  SSS: opcode(8) | A(8) | B(8) | C(8)     — 3 register operands
  SS:  opcode(8) | A(8) | D(16)            — dest + 16-bit operand
  S:   opcode(8) | E(24)                   — one 24-bit operand
  SI:  opcode(8) | A(8) | imm16(16)        — dest + signed immediate
```

Sema's new format (proposal):

```
 31       24 23    16 15      8 7       0
┌──────────┬────────┬─────────┬─────────┐
│   C/D    │   B    │    A    │ opcode  │
└──────────┴────────┴─────────┴─────────┘

ADD   A, B, C    → stack[base+A] = stack[base+B] + stack[base+C]
LOADI A, imm16   → stack[base+A] = Value::int(sign_extend(imm16))
CALL  A, B, argc → result in A, func in B, argc in C
JMP   offset24   → pc += sign_extend(E)
```

**Step 1:** Define the new instruction format in `chunk.rs`:

```rust
/// A 32-bit instruction word.
/// Fields are extracted via bit manipulation (no byte reads).
#[derive(Debug, Clone, Copy)]
pub struct Inst(pub u32);

impl Inst {
    #[inline(always)] pub fn opcode(self) -> u8 { (self.0 & 0xFF) as u8 }
    #[inline(always)] pub fn a(self) -> u8 { ((self.0 >> 8) & 0xFF) as u8 }
    #[inline(always)] pub fn b(self) -> u8 { ((self.0 >> 16) & 0xFF) as u8 }
    #[inline(always)] pub fn c(self) -> u8 { ((self.0 >> 24) & 0xFF) as u8 }
    #[inline(always)] pub fn d(self) -> u16 { ((self.0 >> 16) & 0xFFFF) as u16 }
    #[inline(always)] pub fn ds(self) -> i16 { ((self.0 >> 16) as i16) }
    #[inline(always)] pub fn e(self) -> u32 { self.0 >> 8 }
    #[inline(always)] pub fn es(self) -> i32 { (self.0 as i32) >> 8 }
}
```

**Step 2:** Write unit tests for bit extraction.

**Step 3:** Commit: `feat(vm): add 32-bit instruction format`

### Task 3.2: Implement register allocator in compiler

This replaces the current stack-position tracking with explicit register (slot) allocation. Each function has a fixed `slotcount` (like Janet's `JanetFuncDef.slotcount`).

**Key design:**

- Locals occupy slots 0..n_params+n_locals
- Temporaries occupy slots above locals
- Compiler tracks a "next free slot" watermark
- Each expression compilation returns the slot it wrote its result to

This is a significant rewrite of `compiler.rs`. Break into sub-tasks:

1. Add `RegisterAllocator` struct that tracks free/used slots
2. Change `compile_expr` to take a `dest: u8` parameter and write result there
3. Convert each expression compiler (if, let, call, etc.) to use register ops
4. Update the emitter to output `Inst` words instead of byte sequences

**Step 4:** Run all VM integration tests after each sub-task.

**Step 5:** Commit incrementally.

### Task 3.3: Rewrite VM dispatch loop for 32-bit instructions

```rust
fn run(&mut self, ctx: &EvalContext) -> Result<Value, SemaError> {
    let mut pc: usize;
    let mut base: usize;
    let mut code: &[Inst]; // now &[u32] essentially

    // Restore from top frame
    // ...

    loop {
        let inst = code[pc];
        pc += 1;

        match inst.opcode() {
            op::ADD => {
                let a = inst.a() as usize;
                let b = inst.b() as usize;
                let c = inst.c() as usize;
                let bv = &self.stack[base + b];
                let cv = &self.stack[base + c];
                // Fast-path: small ints
                if let (Some(x), Some(y)) = (bv.try_as_small_int(), cv.try_as_small_int()) {
                    self.stack[base + a] = Value::int(x.wrapping_add(y));
                } else {
                    self.stack[base + a] = vm_add_slow(bv, cv)?;
                }
            }
            op::LOADI => {
                let a = inst.a() as usize;
                let imm = inst.ds() as i64;
                self.stack[base + a] = Value::int(imm);
            }
            op::LOAD_LOCAL => {
                // In register VM, this is just a MOV: stack[base+A] = stack[base+B]
                let a = inst.a() as usize;
                let b = inst.b() as usize;
                self.stack[base + a] = self.stack[base + b].clone();
            }
            op::CALL => {
                // A = dest, B = func slot, C = argc
                // args are in slots B+1..B+1+C
                // ...
            }
            // ...
        }
    }
}
```

**Key advantage:** `code[pc]` is a single `u32` read. No variable-length decoding. The CPU can prefetch `code[pc+1]` while executing `code[pc]`.

**Step 4:** Run tests, benchmark, commit.

---

## Phase 4: Computed Gotos / Dispatch Table (Est. 1.2-1.5x speedup)

**Problem:** A `match` statement on opcode compiles to either a jump table or a series of comparisons. Computed gotos (GCC/Clang) eliminate the branch predictor penalty of the indirect jump by threading each handler's `goto *table[next_opcode]` through the end of the previous handler.

**Solution:** Rust doesn't have computed gotos natively, but there are workarounds:

**Files:**

- Modify: `crates/sema-vm/src/vm.rs` — dispatch loop

### Task 4.1: Research and implement best dispatch for Rust

**Option A: Rely on LLVM** — A `match` with contiguous u8 variants already compiles to a jump table. Profile to verify.

**Option B: `unsafe` function pointer table:**

```rust
type Handler = unsafe fn(vm: &mut VM, inst: Inst, ...) -> ...;
static DISPATCH: [Handler; 256] = [ ... ];

loop {
    let inst = code[pc];
    pc += 1;
    unsafe { DISPATCH[inst.opcode() as usize](self, inst, ...) };
}
```

**Option C: `#[repr(u8)]` enum + `unreachable_unchecked`** — Encode opcodes as a `#[repr(u8)]` enum and transmute, letting LLVM know the match is exhaustive.

**Step 1:** Profile the current dispatch loop with `samply` to measure branch misprediction rate.

**Step 2:** Try Option C first (safest, likely sufficient).

**Step 3:** Benchmark A vs C. If <5% difference, keep the simpler `match`.

**Step 4:** Commit: `perf(vm): optimize dispatch loop`

---

## Phase 5: Reduce Clone/Drop Overhead (Est. 1.2-1.5x speedup)

**Problem:** Every `self.stack[base + slot].clone()` bumps an `Rc` refcount (atomic `increment_strong_count`). Every `self.stack.pop()` drops a `Value` (may `decrement_strong_count` + `dealloc`). For TAK with 66M arithmetic ops, that's ~200M refcount operations.

Janet avoids this entirely because it uses a **tracing GC** — values are just copied (8 bytes, `memcpy`), no refcount.

**Solution:** Short of switching to a tracing GC (out of scope), we can minimize unnecessary clones.

**Files:**

- Modify: `crates/sema-vm/src/vm.rs` — stack operations
- Modify: `crates/sema-core/src/value.rs` — add `is_immediate()` method

### Task 5.1: Skip Rc ops for immediate values

NaN-boxed immediates (ints, bools, nil, symbols, keywords, chars) don't have Rc pointers — their `clone()` and `drop()` are no-ops. But the compiler doesn't know that at compile time. Add a branch:

```rust
/// Clone without refcount bump for immediates.
/// For heap values, falls back to regular clone.
#[inline(always)]
pub fn cheap_clone(&self) -> Value {
    if self.is_immediate() {
        Value(self.0)  // just copy the u64, no Rc bump
    } else {
        self.clone()   // heap value, need Rc bump
    }
}
```

Where `is_immediate()` checks if the tag is one of: `NIL`, `TRUE`, `FALSE`, `INT_SMALL`, `CHAR`, `SYMBOL`, `KEYWORD`.

**Step 1:** Add `is_immediate()` and `cheap_clone()` to `Value`.

**Step 2:** Replace `self.stack[x].clone()` with `self.stack[x].cheap_clone()` in hot paths (LoadLocal, arithmetic operand reads).

**Step 3:** Benchmark. For TAK (all small ints), this should eliminate nearly all refcount overhead.

**Step 4:** Commit: `perf(vm): skip Rc refcount for immediate values in hot paths`

### Task 5.2: In-place stack writes for register VM

With a register-based VM, `ADD A, B, C` writes directly to `stack[base+A]`. The old value at that slot needs to be dropped. For immediates, the drop is free. For heap values, we need the drop.

Use `std::mem::replace` to swap without double-refcount:

```rust
// Instead of:
self.stack[base + a] = result;  // drops old, clones new

// Use:
let _old = std::mem::replace(&mut self.stack[base + a], result);
// _old drops at end of scope — single decrement, no extra increment
```

This is already what assignment does in Rust, but making it explicit helps with the `unsafe` optimizations in Phase 6.

---

## Phase 6: Advanced Optimizations (Est. 1.1-1.3x each)

These are smaller, incremental wins to apply after the big architectural changes land.

### Task 6.1: Inline caching for global lookups

**Problem:** `LoadGlobal` does a hashmap lookup every time. In a tight loop calling a global function, the same lookup repeats millions of times.

**Solution:** Inline cache — store the last-seen (key, value) pair in the instruction's operand. On hit, skip the hashmap entirely.

```rust
// Pseudo-code for inline-cached global load:
op::LOAD_GLOBAL_CACHED => {
    let cache_idx = inst.d() as usize;
    let cached = &self.global_cache[cache_idx];
    if cached.generation == self.global_generation {
        self.stack[base + inst.a() as usize] = cached.value.cheap_clone();
    } else {
        // Cache miss: do full lookup, update cache
        let spur = self.global_spurs[cache_idx];
        let val = self.globals.get(spur).ok_or(...)?;
        self.global_cache[cache_idx] = CacheEntry { value: val.clone(), generation: self.global_generation };
        self.stack[base + inst.a() as usize] = val;
    }
}
```

### Task 6.2: Superinstructions for common patterns

Identify hot instruction sequences and fuse them:

| Pattern                                       | Fused instruction     | Savings                     |
| --------------------------------------------- | --------------------- | --------------------------- |
| `LoadLocal A; LoadLocal B; Add; StoreLocal C` | `ADD_LOCAL C, A, B`   | 3 dispatches                |
| `LoadLocal A; LoadConst imm; Lt`              | `LT_IMM dest, A, imm` | 2 dispatches                |
| `Call N; Return`                              | `TAILCALL N`          | 1 dispatch + frame pop/push |

**Implementation:** Add a peephole optimization pass over the compiled instruction stream before execution.

### Task 6.3: Pre-sized stack allocation

**Problem:** `self.stack` grows dynamically with `Vec::push`, causing reallocations and bounds checks.

**Solution:** Each function's `max_stack` (already tracked in `Chunk`) tells us the maximum stack depth. Pre-allocate the full stack at frame entry:

```rust
// On function entry:
let needed = base + func.chunk.max_stack as usize;
if self.stack.len() < needed {
    self.stack.resize(needed, Value::nil());
}
// Now all stack accesses are in-bounds — can use get_unchecked in release mode.
```

### Task 6.4: `unsafe` stack access in release mode

Once the stack is pre-sized, bounds checks on `self.stack[base + slot]` are redundant. In the hot loop:

```rust
#[inline(always)]
unsafe fn stack_get(&self, idx: usize) -> &Value {
    debug_assert!(idx < self.stack.len());
    self.stack.get_unchecked(idx)
}
```

This eliminates bounds checks from every opcode handler. Estimated ~5-10% on compute-heavy benchmarks.

---

## Expected Cumulative Results

| Phase    | Change                    | Est. speedup | Cumulative time (tak) |
| -------- | ------------------------- | ------------ | --------------------- |
| Baseline | —                         | —            | 9.26s                 |
| Phase 1  | Fast-path arithmetic      | 1.5-2x       | ~5.5s                 |
| Phase 2  | Eliminate per-call allocs | 1.3-1.5x     | ~4.0s                 |
| Phase 3  | Register-based encoding   | 1.5-2x       | ~2.3s                 |
| Phase 4  | Dispatch optimization     | 1.1-1.3x     | ~1.9s                 |
| Phase 5  | Reduce clone/drop         | 1.1-1.3x     | ~1.6s                 |
| Phase 6  | Advanced opts             | 1.1-1.3x     | ~1.3s                 |

**Target: ~1.3-2.0s** — within 1.1-1.7x of Janet's 1.19s.

> Note: Estimates are rough. Phases 1-2 are the highest confidence / lowest risk. Phase 3 is the largest change. Phases 4-6 are polish. Measure after each phase and re-evaluate.

---

## Implementation Order & Dependencies

```
Phase 1 (arithmetic fast-path)     ← standalone, do first
    ↓
Phase 5.1 (cheap_clone)            ← standalone, pairs well with Phase 1
    ↓
Phase 2 (lazy upvalues, inline CF) ← standalone
    ↓
Phase 3 (register-based)           ← biggest change, needs careful design
    ↓
Phase 4 (dispatch)                 ← benefits most after Phase 3
    ↓
Phase 6 (advanced)                 ← incremental, apply as needed
```

Phases 1, 2, and 5.1 can be done independently and benchmarked. Phase 3 is the keystone change and will likely require 2-3 days. Phase 4 and 6 are diminishing returns — apply based on profiling data.

---

## Benchmark Protocol

After each phase, run:

```bash
# Build release
cargo build --release

# Run benchmarks (5 runs, 1 warmup)
hyperfine --runs 5 --warmup 1 --style full \
  -n "sema-vm: tak" "./target/release/sema --no-llm --vm examples/benchmarks/tak.sema" \
  -n "janet: tak" "janet examples/benchmarks/tak.janet"

hyperfine --runs 5 --warmup 1 --style full \
  -n "sema-vm: nqueens" "./target/release/sema --no-llm --vm examples/benchmarks/nqueens.sema" \
  -n "janet: nqueens" "janet examples/benchmarks/nqueens.janet"

# Profile with samply if regression or unexpected result
make profile PROFILE_BENCH=tak PROFILE_MODE=vm
```

Record results in `docs/benchmarks/vm-optimization-log.md` with git SHA, date, and phase.

---

## What This Plan Does NOT Cover

- **Tracing GC**: Would eliminate all Rc overhead but is a massive architectural change (months of work). The clone/drop mitigations in Phase 5 get us ~80% of the benefit.
- **JIT compilation**: Cranelift/LLVM JIT would leap past Janet but is a separate project.
- **Multi-threading**: Janet is also single-threaded; not relevant for this comparison.
- **String interning for runtime strings**: Already done via `lasso`.
- **Tree-walker optimizations**: Out of scope — the VM is the performance path going forward.
