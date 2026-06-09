# VM Performance Optimization Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Achieve 2-3× speedup on the bytecode VM through targeted optimizations of the dispatch loop, call path, and allocation patterns.

**Architecture:** The VM is a stack-based bytecode interpreter with NaN-boxed 8-byte values. Current bottlenecks (confirmed by analysis of 31.8M calls in tak benchmark): (1) per-instruction frame re-indexing and pc writeback, (2) heap allocation of `open_upvalues` on every call, (3) Rc refcount bumps when type-testing callees in `call_value`, (4) `Env::get()` with `RefCell::borrow()` + hashmap lookup for globals.

**Tech Stack:** Rust 2021, single-threaded (Rc), NaN-boxed values, hashbrown HashMap for Env.

**Baseline:** tak(18,12,6) × 500 = **5.97s** (VM), **20.15s** (tree-walker)

---

### Task 1: Restructure dispatch loop — cache frame locals

**Files:**
- Modify: `crates/sema-vm/src/vm.rs` — the `run()` method (lines 91-548)

**Problem:** Every single opcode re-reads `self.frames[fi]` (bounds-checked indexing), reloads `code/base/pc`, and writes back `self.frames[fi].pc = pc`. With ~billions of instructions in the benchmark, this is pure overhead.

**Step 1: Add outer/inner loop structure**

Replace the current `loop { ... }` in `run()` with a two-level loop. The outer loop caches frame-local variables. The inner loop dispatches opcodes using those cached locals, only breaking to the outer loop when frames change (Call/TailCall/Return).

```rust
fn run(&mut self, ctx: &EvalContext) -> Result<Value, SemaError> {
    // macros stay the same...

    'dispatch: loop {
        let fi = self.frames.len() - 1;
        let frame = &self.frames[fi];
        let code = frame.closure.func.chunk.code.as_ptr();
        let code_len = frame.closure.func.chunk.code.len();
        let consts = &frame.closure.func.chunk.consts;
        let base = frame.base;
        let mut pc = frame.pc;
        drop(frame); // release borrow

        loop {
            debug_assert!(pc < code_len);
            let op_byte = unsafe { *code.add(pc) };
            pc += 1;

            match op_byte {
                // Op::Nil = 1
                1 => { self.stack.push(Value::nil()); }

                // Op::LoadLocal = 6
                6 => {
                    let slot = unsafe {
                        u16::from_le_bytes([*code.add(pc), *code.add(pc + 1)]) as usize
                    };
                    pc += 2;
                    // Fast path: skip upvalue check (most locals aren't captured)
                    let val = self.stack[base + slot].clone();
                    self.stack.push(val);
                }

                // Op::Call = 16
                16 => {
                    let argc = unsafe {
                        u16::from_le_bytes([*code.add(pc), *code.add(pc + 1)]) as usize
                    };
                    pc += 2;
                    self.frames[fi].pc = pc; // writeback only here
                    self.call_value(argc, ctx)?;
                    continue 'dispatch; // re-cache frame locals
                }

                // Op::Return = 18
                18 => {
                    let result = self.stack.pop().unwrap_or(Value::nil());
                    let frame = self.frames.pop().unwrap();
                    self.stack.truncate(frame.base);
                    if self.frames.is_empty() {
                        return Ok(result);
                    }
                    self.stack.push(result);
                    continue 'dispatch;
                }

                // ... all other opcodes
                _ => { /* keep using op_byte matches for all opcodes */ }
            }
        }
    }
}
```

**Key rules:**
- Only write back `self.frames[fi].pc = pc` before Call/TailCall/MakeClosure/exception paths
- Use raw pointer `code` for bytecode reads (avoids slice bounds checks in inner loop)
- Match on `u8` directly, not `Op` enum (eliminates `from_u8` + `Option` branch)
- `continue 'dispatch` after any opcode that changes the frame stack

**Step 2: Handle LoadLocal upvalue check efficiently**

For `LoadLocal`, the upvalue check (`open_upvalues.get(slot)`) is only needed when a local has been captured. Most functions don't have captures. Add a `has_captures` check:

In `CallFrame`, add:
```rust
struct CallFrame {
    // ... existing fields ...
    has_open_upvalues: bool, // true only when any upvalue has been opened
}
```

Then `LoadLocal` becomes:
```rust
6 => {
    let slot = /* read u16 */;
    let val = if self.frames[fi].has_open_upvalues {
        if let Some(Some(cell)) = self.frames[fi].open_upvalues.get(slot) {
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

**Step 3: Run tests**

Run: `cargo test -p sema-vm && cargo test -p sema --test integration_test && cargo test -p sema --test vm_integration_test`
Expected: All pass

**Step 4: Benchmark**

Run: `make bench-vm`
Expected: Measurable improvement on tak (target: 15-30% faster)

**Step 5: Commit**

```bash
git add crates/sema-vm/src/vm.rs
git commit -m "perf(vm): restructure dispatch loop with cached frame locals"
```

---

### Task 2: Eliminate per-call `open_upvalues` heap allocation

**Files:**
- Modify: `crates/sema-vm/src/vm.rs` — `CallFrame`, `call_vm_closure`, `tail_call_vm_closure`
- Modify: `crates/sema-vm/src/chunk.rs` — `Function` struct

**Problem:** Every call to a VM closure allocates `vec![None; n_locals]` (31.8M heap allocations in tak). Most functions (like tak) never capture any locals as upvalues.

**Step 1: Add `captures_any_local` flag to Function**

In `crates/sema-vm/src/chunk.rs`:
```rust
pub struct Function {
    // ... existing fields ...
    /// True if any inner closure captures a local from this function.
    pub captures_any_local: bool,
}
```

**Step 2: Set the flag in the compiler**

In `crates/sema-vm/src/compiler.rs`, in `compile_lambda`:
```rust
let captures_any_local = def.upvalues.iter().any(|uv| matches!(uv, UpvalueDesc::ParentLocal(_)));
// Actually, this flag is about whether THIS function's locals are captured by children.
// Check if any child's upvalue_descs reference ParentLocal.
```

Actually, the simpler approach: compute it from the body. If the function's body contains any `MakeClosure` instruction that captures a local (is_local=true), then `captures_any_local = true`. But even simpler: check at runtime in `call_vm_closure` — if `n_upvalues == 0` for the function being called AND no child closures capture locals, skip the allocation.

Simplest correct approach: check if `func.chunk.code` contains any `MakeClosure` opcode. But that's expensive. Instead, set the flag during compilation based on whether any inner lambda has `UpvalueDesc::ParentLocal` referencing this function's locals.

**Simplest approach that works now:** Lazily allocate. Change `open_upvalues` from `Vec<Option<Rc<UpvalueCell>>>` to `Option<Vec<Option<Rc<UpvalueCell>>>>`. Only allocate when `MakeClosure` actually tries to capture a local.

```rust
struct CallFrame {
    closure: Rc<Closure>,
    pc: usize,
    base: usize,
    open_upvalues: Option<Vec<Option<Rc<UpvalueCell>>>>,
}
```

In `call_vm_closure` / `tail_call_vm_closure`:
```rust
open_upvalues: None, // don't allocate!
```

In `make_closure`, when capturing a local:
```rust
let frame = self.frames.last_mut().unwrap();
let open = frame.open_upvalues.get_or_insert_with(|| vec![None; n_locals_for_this_frame]);
```

In `LoadLocal` / `StoreLocal`:
```rust
// Only check upvalues if they exist
if let Some(ref open) = self.frames[fi].open_upvalues {
    if let Some(Some(cell)) = open.get(slot) {
        // use cell
    }
}
```

**Step 3: Update all open_upvalues access sites**

grep for `open_upvalues` in vm.rs and update:
- `execute()` — initial frame: use `None`
- `call_vm_closure()` — use `None`
- `tail_call_vm_closure()` — use `None`
- `make_closure()` — lazy init
- `LoadLocal` — conditional check
- `StoreLocal` — conditional check
- fallback closure in `make_closure` — use `None`

**Step 4: Run tests**

Run: `cargo test -p sema-vm && cargo test -p sema --test vm_integration_test`
Expected: All pass (including `test_vm_counter_closure`, `test_vm_shared_mutable_upvalue`)

**Step 5: Benchmark**

Run: `make bench-vm`
Expected: Significant improvement on tak (31.8M fewer heap allocations)

**Step 6: Commit**

```bash
git add crates/sema-vm/src/vm.rs crates/sema-vm/src/chunk.rs
git commit -m "perf(vm): lazily allocate open_upvalues — skip heap alloc for non-capturing calls"
```

---

### Task 3: Avoid Rc refcount bumps in call_value dispatch

**Files:**
- Modify: `crates/sema-core/src/value.rs` — add `tag()` and `as_native_fn_ref()` methods
- Modify: `crates/sema-vm/src/vm.rs` — `call_value` and `tail_call_value`

**Problem:** `call_value` calls `func_val.as_native_fn_rc()` which does `Rc::increment_strong_count` + returns `Rc<NativeFn>`, then drops it. For 31.8M calls that's 63.6M unnecessary refcount operations.

**Step 1: Add `tag()` method to Value**

In `crates/sema-core/src/value.rs`:
```rust
/// Get the NaN-boxing tag without any refcount changes. Returns None for floats.
#[inline(always)]
pub fn tag(&self) -> Option<u64> {
    if is_boxed(self.0) {
        Some(get_tag(self.0))
    } else {
        None // float
    }
}

/// Check if this value is a NativeFn without bumping refcount.
#[inline(always)]
pub fn is_native_fn(&self) -> bool {
    is_boxed(self.0) && get_tag(self.0) == TAG_NATIVE_FN
}

/// Borrow the NativeFn without Rc clone (no refcount bump).
/// SAFETY: Caller must ensure the value IS a NativeFn.
#[inline(always)]
pub unsafe fn as_native_fn_unchecked(&self) -> &NativeFn {
    self.borrow_ref::<NativeFn>()
}

/// Check if this value is a Lambda without bumping refcount.
#[inline(always)]
pub fn is_lambda(&self) -> bool {
    is_boxed(self.0) && get_tag(self.0) == TAG_LAMBDA
}
```

**Step 2: Rewrite call_value to use tag dispatch**

```rust
fn call_value(&mut self, argc: usize, ctx: &EvalContext) -> Result<(), SemaError> {
    let func_idx = self.stack.len() - 1 - argc;
    let func_val = &self.stack[func_idx];

    // Check tag without refcount bump
    let tag = if is_boxed(func_val.0) { get_tag(func_val.0) } else { u64::MAX };

    match tag {
        TAG_NATIVE_FN => {
            let native = unsafe { func_val.as_native_fn_unchecked() };
            // Check for VM closure payload
            if let Some(payload) = &native.payload {
                if let Some(vmc) = payload.downcast_ref::<VmClosurePayload>() {
                    return self.call_vm_closure(vmc, argc);
                }
            }
            // Regular native call — need to collect args
            let args_start = func_idx + 1;
            let args: Vec<Value> = self.stack[args_start..].to_vec();
            self.stack.truncate(func_idx);
            let result = (native.func)(ctx, &args)?;
            self.stack.push(result);
            Ok(())
        }
        TAG_LAMBDA => {
            let func_val = self.stack[func_idx].clone(); // clone needed for call_callback
            let args_start = func_idx + 1;
            let args: Vec<Value> = self.stack[args_start..].to_vec();
            self.stack.truncate(func_idx);
            let result = sema_core::call_callback(ctx, &func_val, &args)?;
            self.stack.push(result);
            Ok(())
        }
        TAG_KEYWORD => {
            // ... keyword-as-function (existing code)
        }
        _ => {
            // Fallback: call_callback
            let func_val = self.stack[func_idx].clone();
            let args_start = func_idx + 1;
            let args: Vec<Value> = self.stack[args_start..].to_vec();
            self.stack.truncate(func_idx);
            let result = sema_core::call_callback(ctx, &func_val, &args)?;
            self.stack.push(result);
            Ok(())
        }
    }
}
```

**Note:** The `as_native_fn_unchecked` borrows the NativeFn without cloning the Rc. This is safe because the Value on the stack holds the Rc alive. But we must not drop/truncate the stack while holding the borrow. The current code already collects args before truncating, so this is fine — but we need to be careful that the borrow of `native` doesn't outlive the stack slot. The payload check and `(native.func)(ctx, &args)` call happen before truncation. Actually, we need to be careful: `self.call_vm_closure` modifies the stack. The `native` reference is into `self.stack[func_idx]` which is below the args... actually the code already clones `func_val` with `.clone()` today, so the borrow approach needs care.

**Safer approach:** Use the tag constants directly (export them or use a helper). Actually, the simplest safe optimization: don't clone `func_val` eagerly. Check the tag first using the raw u64 bits, then only clone when actually needed:

```rust
fn call_value(&mut self, argc: usize, ctx: &EvalContext) -> Result<(), SemaError> {
    let func_idx = self.stack.len() - 1 - argc;

    // Peek at tag bits without cloning (no refcount bump)
    let bits = self.stack[func_idx].0;
    let tag = if is_boxed(bits) { get_tag(bits) } else { u64::MAX };

    if tag == TAG_NATIVE_FN {
        // Clone only the Rc<NativeFn>, not the full Value
        let native = unsafe { self.stack[func_idx].get_rc::<NativeFn>() };
        if let Some(payload) = &native.payload {
            if let Some(vmc) = payload.downcast_ref::<VmClosurePayload>() {
                return self.call_vm_closure(vmc, argc);
            }
        }
        let args_start = func_idx + 1;
        let args: Vec<Value> = self.stack[args_start..].to_vec();
        self.stack.truncate(func_idx);
        let result = (native.func)(ctx, &args)?;
        self.stack.push(result);
        Ok(())
    } else { /* ... */ }
}
```

Wait, `get_rc` still bumps refcount. The real win is to avoid the clone+drop *entirely* by checking the VmClosurePayload via raw pointer. Let me think...

**Actually the simplest high-value change**: Since `call_vm_closure` is the hot path (31.8M calls), and the bottleneck is the `as_native_fn_rc` → `payload.downcast_ref` chain, we can add a **dedicated NaN-boxing tag for VM closures** (`TAG_VM_CLOSURE = 24`) that the VM can detect with a single tag check — no Rc clone, no Any downcast.

This is the advanced path from the oracle's recommendation. But it requires changes across value.rs + vm.rs + make_closure. Let's do the simpler version first: expose `Value.0` bits for VM-internal tag checking.

**Pragmatic step:** Add `Value::raw_bits()` and re-export the tag constants needed by the VM:

```rust
// In value.rs
#[inline(always)]
pub fn raw_tag(&self) -> u64 {
    if is_boxed(self.0) { get_tag(self.0) } else { u64::MAX }
}

pub const TAG_NATIVE_FN_VALUE: u64 = TAG_NATIVE_FN;
pub const TAG_LAMBDA_VALUE: u64 = TAG_LAMBDA;
pub const TAG_KEYWORD_VALUE: u64 = TAG_KEYWORD;
```

Then in `call_value`, check `func_val.raw_tag()` and only clone when needed.

**Step 3: Run tests**

Run: `cargo test -p sema-vm && cargo test -p sema --test vm_integration_test`

**Step 4: Benchmark**

Expected: 10-20% improvement from reduced refcount traffic

**Step 5: Commit**

```bash
git add crates/sema-core/src/value.rs crates/sema-vm/src/vm.rs
git commit -m "perf(vm): avoid Rc refcount bumps in call_value dispatch path"
```

---

### Task 4: Inline arithmetic helpers and add NaN-box fast paths

**Files:**
- Modify: `crates/sema-vm/src/vm.rs` — arithmetic opcode handlers + `vm_add/sub/mul/lt` helpers

**Problem:** The specialized int opcodes (AddInt, SubInt, etc.) pop values, check `as_int()`, then push result. `as_int()` does `is_boxed` + `get_tag` + sign extension. For the inner loop, we can operate directly on the NaN-boxed u64 bits.

**Step 1: Mark arithmetic helpers as `#[inline(always)]`**

```rust
#[inline(always)]
fn vm_add(a: &Value, b: &Value) -> Result<Value, SemaError> { ... }
#[inline(always)]
fn vm_sub(a: &Value, b: &Value) -> Result<Value, SemaError> { ... }
// etc.
```

**Step 2: Add a raw int extraction helper for the VM**

Since we know the NaN-boxing layout, we can extract small ints without the full `as_int()` ceremony:

```rust
/// Fast inline check: is this Value a small int?
#[inline(always)]
fn is_small_int(v: &Value) -> bool {
    is_boxed(v.0) && get_tag(v.0) == TAG_INT_SMALL
}

/// Extract small int payload (caller must verify is_small_int first).
#[inline(always)]
fn small_int_payload(v: &Value) -> i64 {
    let payload = get_payload(v.0);
    if payload & INT_SIGN_BIT != 0 {
        (payload | !PAYLOAD_MASK) as i64
    } else {
        payload as i64
    }
}
```

Then AddInt becomes:
```rust
// Op::AddInt
37 => {
    let len = self.stack.len();
    let b = &self.stack[len - 1];
    let a = &self.stack[len - 2];
    if is_small_int(a) && is_small_int(b) {
        let result = Value::int(small_int_payload(a).wrapping_add(small_int_payload(b)));
        self.stack.truncate(len - 2);
        self.stack.push(result);
    } else {
        let b = self.stack.pop().unwrap();
        let a = self.stack.pop().unwrap();
        // fallback
        self.stack.push(vm_add(&a, &b)?);
    }
}
```

Actually even better — peek at the stack without popping:

```rust
37 => {
    let len = self.stack.len();
    unsafe {
        let b = self.stack.get_unchecked(len - 1);
        let a = self.stack.get_unchecked(len - 2);
        if is_boxed(a.0) && get_tag(a.0) == TAG_INT_SMALL
            && is_boxed(b.0) && get_tag(b.0) == TAG_INT_SMALL
        {
            let ax = { let p = get_payload(a.0); if p & INT_SIGN_BIT != 0 { (p | !PAYLOAD_MASK) as i64 } else { p as i64 } };
            let bx = { let p = get_payload(b.0); if p & INT_SIGN_BIT != 0 { (p | !PAYLOAD_MASK) as i64 } else { p as i64 } };
            // Write result directly, no clone/drop of old values since they're immediates
            *self.stack.get_unchecked_mut(len - 2) = Value::int(ax.wrapping_add(bx));
            self.stack.set_len(len - 1);
        } else {
            // slow path
        }
    }
}
```

**Step 3: Run tests**

Run: `cargo test -p sema-vm`

**Step 4: Benchmark**

Expected: 5-15% improvement on arithmetic-heavy benchmarks

**Step 5: Commit**

```bash
git add crates/sema-vm/src/vm.rs
git commit -m "perf(vm): inline arithmetic helpers and add NaN-box fast paths"
```

---

### Task 5: Add specialized opcodes for common patterns

**Files:**
- Modify: `crates/sema-vm/src/opcodes.rs` — add new opcodes
- Modify: `crates/sema-vm/src/compiler.rs` — emit specialized opcodes
- Modify: `crates/sema-vm/src/vm.rs` — handle new opcodes
- Modify: `crates/sema-vm/src/disasm.rs` — disassemble new opcodes

**Step 1: Add LoadLocal0..3 opcodes (zero-operand local loads)**

In `opcodes.rs`, add after the existing opcodes:
```rust
// Specialized zero-operand locals (most common slots)
LoadLocal0, // = 42
LoadLocal1, // = 43
LoadLocal2, // = 44
LoadLocal3, // = 45
```

Update `from_u8` to include 42-45.

**Step 2: Emit LoadLocal0..3 in compiler**

In `compile_var_load`:
```rust
VarResolution::Local { slot } => {
    match slot {
        0 => self.emit.emit_op(Op::LoadLocal0),
        1 => self.emit.emit_op(Op::LoadLocal1),
        2 => self.emit.emit_op(Op::LoadLocal2),
        3 => self.emit.emit_op(Op::LoadLocal3),
        _ => {
            self.emit.emit_op(Op::LoadLocal);
            self.emit.emit_u16(slot);
        }
    }
}
```

**Step 3: Handle in VM dispatch**

```rust
42 => { // LoadLocal0
    let val = self.stack[base].clone();
    self.stack.push(val);
}
43 => { // LoadLocal1
    let val = self.stack[base + 1].clone();
    self.stack.push(val);
}
// etc.
```

**Step 4: Update disassembler and `patch_closure_func_ids`**

Add the new opcodes to `disasm.rs` and ensure `patch_closure_func_ids` handles them (single-byte, no operands).

**Step 5: Run tests**

Run: `cargo test -p sema-vm && cargo test -p sema --test vm_integration_test`

**Step 6: Benchmark and commit**

```bash
git commit -m "perf(vm): add LoadLocal0..3 specialized opcodes"
```

---

### Task 6: Optimize global lookups for the VM

**Files:**
- Modify: `crates/sema-core/src/value.rs` — add `Env` direct-slot access
- Modify: `crates/sema-vm/src/vm.rs` — global lookup fast path

**Problem:** Every `LoadGlobal` does `RefCell::borrow()` + hashmap lookup via `Env::get()`. For recursive calls to global functions (like `tak`), this is the dominant overhead.

**Step 1: Add a version counter and direct-lookup cache to Env**

In `value.rs`, add a version counter to Env:
```rust
pub struct Env {
    pub bindings: Rc<RefCell<SpurMap<Spur, Value>>>,
    pub parent: Option<Rc<Env>>,
    pub version: Cell<u64>,
}
```

Increment `version` in `set`, `set_existing`, `update`, `take`, `take_anywhere`.

**Step 2: Add a global cache to VM**

```rust
pub struct VM {
    // ... existing ...
    /// Cache for global lookups: maps Spur → (cached_value, env_version)
    global_cache: hashbrown::HashMap<u32, (Value, u64)>,
}
```

In `LoadGlobal`:
```rust
// Check cache first
let spur_bits = read_u32_inline!(code, pc);
let spur: Spur = unsafe { std::mem::transmute(spur_bits) };
let version = self.globals.version.get();
if let Some((cached, ver)) = self.global_cache.get(&spur_bits) {
    if *ver == version {
        self.stack.push(cached.clone());
        continue;
    }
}
// Cache miss — do full lookup
match self.globals.get(spur) {
    Some(val) => {
        self.global_cache.insert(spur_bits, (val.clone(), version));
        self.stack.push(val);
    }
    None => { /* error path */ }
}
```

**Step 3: Run tests, benchmark, commit**

Run: `cargo test -p sema-vm && cargo test -p sema --test vm_integration_test`

Expected: Significant speedup for recursive global calls

```bash
git commit -m "perf(vm): add global lookup cache with version invalidation"
```

---

## Execution Order and Expected Impact

| Task | Description | Expected Speedup | Risk |
|------|-------------|-----------------|------|
| 1 | Restructure dispatch loop | 15-30% | Low |
| 2 | Lazy open_upvalues | 10-30% | Low |
| 3 | Avoid Rc bumps in call_value | 10-20% | Medium |
| 4 | Inline arithmetic fast paths | 5-15% | Low |
| 5 | Specialized LoadLocal0..3 | 3-8% | Low |
| 6 | Global lookup cache | 15-40% | Medium |

**Combined target:** 2-3× speedup (tak from ~6s to ~2-3s)

Tasks 1-2 are highest priority and lowest risk. Task 6 has the highest individual potential but requires Env changes across crates.
