# Lua-Style Open Upvalues Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the eager-close upvalue model with Lua-style open upvalues, eliminating the dual-write pattern from 10 LoadLocal/StoreLocal opcodes and simplifying the VM's closure model.

**Architecture:** Open upvalues hold a stack index instead of a copied value. LoadLocal/StoreLocal become unconditional stack access. LoadUpvalue/StoreUpvalue resolve open cells through the stack. Upvalues are closed (value copied from stack into cell) at 4 sites: Return, TailCall, exception unwinding, and before non-VM calls. This is purely a vm.rs runtime change — the compiler, resolver, and bytecode format are unaffected.

**Tech Stack:** Rust, sema-vm crate

---

## File Map

| File | Action | Responsibility |
|------|--------|----------------|
| `crates/sema-vm/src/vm.rs` | Modify | All runtime changes: UpvalueCell enum, close_upvalues, opcode simplification, debug methods |
| `crates/sema/tests/dual_eval_core_test.rs` | Modify | New dual-eval tests for open upvalue semantics |
| `crates/sema-vm/src/resolve.rs` | No change | Already Lua-style (ParentLocal/ParentUpvalue) |
| `crates/sema-vm/src/compiler.rs` | No change | MakeClosure emission format unchanged |
| `crates/sema-vm/src/serialize.rs` | No change | Serializes UpvalueDesc (compile-time), not UpvalueCell (runtime) |

---

## Chunk 1: Type Shape Change (No Behavioral Change)

### Task 1: Change UpvalueCell to Open/Closed enum

**Files:**
- Modify: `crates/sema-vm/src/vm.rs:13-25` (UpvalueCell, Closure structs)

- [ ] **Step 1: Change UpvalueCell to use UpvalueState enum**

Replace the current `UpvalueCell` at vm.rs:13-25:

```rust
/// State of a captured variable (upvalue).
#[derive(Debug)]
pub enum UpvalueState {
    /// Points into the VM stack while the defining frame is alive.
    Open { frame_base: usize, slot: usize },
    /// Owns the value after the defining frame has exited.
    Closed(Value),
}

/// A mutable cell for captured variables (upvalues).
#[derive(Debug)]
pub struct UpvalueCell {
    pub state: RefCell<UpvalueState>,
}

impl UpvalueCell {
    pub fn new_closed(value: Value) -> Self {
        UpvalueCell {
            state: RefCell::new(UpvalueState::Closed(value)),
        }
    }

    pub fn new_open(frame_base: usize, slot: usize) -> Self {
        UpvalueCell {
            state: RefCell::new(UpvalueState::Open { frame_base, slot }),
        }
    }
}
```

- [ ] **Step 2: Fix all compilation errors from the struct change**

Every site that accesses `cell.value` must change to handle the enum. For now, all cells are still created as `Closed` (same semantics), so only the `Closed` arm is reachable.

Update `make_closure` (vm.rs:1740-1741) — change `UpvalueCell::new(val)` to `UpvalueCell::new_closed(val)`:

```rust
let cell = Rc::new(UpvalueCell::new_closed(val));
```

Update `LoadUpvalue` (vm.rs:599-602):

```rust
op::LOAD_UPVALUE => {
    let idx = read_u16!(code, pc) as usize;
    let val = {
        let state = self.frames[fi].closure.upvalues[idx].state.borrow();
        match &*state {
            UpvalueState::Closed(v) => v.clone(),
            UpvalueState::Open { frame_base, slot } => {
                self.stack[*frame_base + *slot].clone()
            }
        }
    };
    self.stack.push(val);
}
```

Update `StoreUpvalue` (vm.rs:604-607):

```rust
op::STORE_UPVALUE => {
    let idx = read_u16!(code, pc) as usize;
    let val = unsafe { pop_unchecked(&mut self.stack) };
    let mut state = self.frames[fi].closure.upvalues[idx].state.borrow_mut();
    match &mut *state {
        UpvalueState::Closed(v) => *v = val,
        UpvalueState::Open { frame_base, slot } => {
            self.stack[*frame_base + *slot] = val;
        }
    }
}
```

Update `LoadLocal` dual-write paths (vm.rs:568-596) — the `has_open_upvalues` branches now read from `cell.state`:

```rust
// In the has_open_upvalues branch of LoadLocal:
if let Some(Some(cell)) = open.get(slot) {
    let state = cell.state.borrow();
    match &*state {
        UpvalueState::Closed(v) => v.clone(),
        UpvalueState::Open { frame_base, slot: s } => {
            self.stack[*frame_base + *s].clone()
        }
    }
}
```

Apply the same pattern to `StoreLocal` (vm.rs:585-596) and all specialized variants `LoadLocal0-3` (vm.rs:1012-1075) and `StoreLocal0-3` (vm.rs:1147-1190).

Update `debug_locals` (vm.rs:1923-1925):

```rust
let val = if let Some(ref open) = frame.open_upvalues {
    if let Some(Some(cell)) = open.get(slot as usize) {
        let state = cell.state.borrow();
        match &*state {
            UpvalueState::Closed(v) => v.clone(),
            UpvalueState::Open { frame_base, slot: s } => {
                self.stack.get(*frame_base + *s).cloned().unwrap_or(Value::nil())
            }
        }
    } else {
        self.stack.get(idx).cloned().unwrap_or(Value::nil())
    }
} else {
    self.stack.get(idx).cloned().unwrap_or(Value::nil())
};
```

Update `debug_upvalues` (vm.rs:1952):

```rust
let val = {
    let state = uv.state.borrow();
    match &*state {
        UpvalueState::Closed(v) => v.clone(),
        UpvalueState::Open { frame_base, slot } => {
            self.stack.get(*frame_base + *slot).cloned().unwrap_or(Value::nil())
        }
    }
};
```

- [ ] **Step 3: Build and run all tests**

Run: `cargo test 2>&1 | grep -E 'FAILED|^test result'`
Expected: All tests pass. Zero behavioral change — all cells are still created as Closed.

- [ ] **Step 4: Commit**

```bash
git add crates/sema-vm/src/vm.rs
git commit -m "refactor(vm): introduce UpvalueState enum (Closed-only, no behavioral change)"
```

---

## Chunk 2: Add close_upvalues Helper and Close Sites

### Task 2: Implement close_upvalues and add to all frame-exit paths

**Files:**
- Modify: `crates/sema-vm/src/vm.rs` (new method + 3 call sites)

- [ ] **Step 1: Add close_upvalues_for_frame method to VM**

Add this method to the `impl VM` block (after `make_closure`, around line 1830):

```rust
/// Close all open upvalues for the given frame.
/// Must be called BEFORE the frame's stack region is truncated or overwritten.
#[inline]
fn close_upvalues_for_frame(&mut self, frame: &mut CallFrame) {
    if let Some(ref open) = frame.open_upvalues {
        let base = frame.base;
        for (slot, maybe_cell) in open.iter().enumerate() {
            if let Some(cell) = maybe_cell {
                let mut state = cell.state.borrow_mut();
                if matches!(&*state, UpvalueState::Open { .. }) {
                    *state = UpvalueState::Closed(self.stack[base + slot].clone());
                }
            }
        }
    }
}
```

- [ ] **Step 2: Add close step to Return opcode**

Modify the `RETURN` handler (vm.rs:693-706). Close upvalues before popping the frame:

```rust
op::RETURN => {
    let result = if !self.stack.is_empty() {
        unsafe { pop_unchecked(&mut self.stack) }
    } else {
        Value::nil()
    };
    let frame = self.frames.last_mut().unwrap();
    self.close_upvalues_for_frame(frame);
    let frame = self.frames.pop().unwrap();
    self.stack.truncate(frame.base);
    if self.frames.is_empty() {
        return Ok(crate::debug::VmExecResult::Finished(result));
    }
    self.stack.push(result);
    continue 'dispatch;
}
```

Note: `close_upvalues_for_frame` takes `&mut CallFrame` but we also need `&mut self` for `self.stack`. This creates a borrow conflict. To resolve, extract the open_upvalues from the frame first:

```rust
op::RETURN => {
    let result = if !self.stack.is_empty() {
        unsafe { pop_unchecked(&mut self.stack) }
    } else {
        Value::nil()
    };
    // Close open upvalues before popping
    if let Some(ref open) = self.frames.last().unwrap().open_upvalues {
        let base = self.frames.last().unwrap().base;
        for (slot, maybe_cell) in open.iter().enumerate() {
            if let Some(cell) = maybe_cell {
                let mut state = cell.state.borrow_mut();
                if matches!(&*state, UpvalueState::Open { .. }) {
                    *state = UpvalueState::Closed(self.stack[base + slot].clone());
                }
            }
        }
    }
    let frame = self.frames.pop().unwrap();
    self.stack.truncate(frame.base);
    if self.frames.is_empty() {
        return Ok(crate::debug::VmExecResult::Finished(result));
    }
    self.stack.push(result);
    continue 'dispatch;
}
```

- [ ] **Step 3: Add close step to tail_call_vm_closure**

Modify `tail_call_vm_closure` (vm.rs:1624-1673). Close upvalues BEFORE `copy_args_to_locals` overwrites the stack:

```rust
fn tail_call_vm_closure(&mut self, closure: Rc<Closure>, argc: usize) -> Result<(), SemaError> {
    // ... arity checks (unchanged) ...

    // Close open upvalues BEFORE overwriting the frame's stack slots
    if let Some(ref open) = self.frames.last().unwrap().open_upvalues {
        let base = self.frames.last().unwrap().base;
        for (slot, maybe_cell) in open.iter().enumerate() {
            if let Some(cell) = maybe_cell {
                let mut state = cell.state.borrow_mut();
                if matches!(&*state, UpvalueState::Open { .. }) {
                    *state = UpvalueState::Closed(self.stack[base + slot].clone());
                }
            }
        }
    }

    // Copy args directly into current frame's base — no Vec allocation
    let func_idx = self.stack.len() - 1 - argc;
    let base = self.frames.last().unwrap().base;
    // ... rest unchanged ...
}
```

- [ ] **Step 4: Add close step to handle_exception**

Modify `handle_exception` (vm.rs:1835-1884). Close upvalues before each `stack.truncate`:

At line 1857 (handler found, partial truncation — close locals above stack_depth AND clear entries so future captures at those slots create fresh cells):

```rust
if let Some(entry) = found {
    // Close and clear upvalues for locals that will be truncated
    if let Some(ref mut open) = self.frames.last_mut().unwrap().open_upvalues {
        let base = self.frames.last().unwrap().base;
        let depth = entry.stack_depth as usize;
        close_open_upvalues_above(open, &self.stack, base, depth);
    }
    let base = self.frames.last().unwrap().base;
    self.stack.truncate(base + entry.stack_depth as usize);
    // ... rest unchanged ...
}
```

At line 1870 (no handler, pop frame — close all upvalues):

```rust
// No handler in this frame, close upvalues and pop
{
    let frame = self.frames.last().unwrap();
    if let Some(ref open) = frame.open_upvalues {
        let base = frame.base;
        for (slot, maybe_cell) in open.iter().enumerate() {
            if let Some(cell) = maybe_cell {
                let mut state = cell.state.borrow_mut();
                if matches!(&*state, UpvalueState::Open { .. }) {
                    *state = UpvalueState::Closed(self.stack[base + slot].clone());
                }
            }
        }
    }
}
let frame = self.frames.pop().unwrap();
self.stack.truncate(frame.base);
```

- [ ] **Step 5: Extract a close helper to avoid duplication**

The close loop appears 4 times. Extract a standalone function:

```rust
/// Close all open upvalues in the given open_upvalues vec, reading from the stack.
fn close_open_upvalues(
    open: &[Option<Rc<UpvalueCell>>],
    stack: &[Value],
    base: usize,
) {
    for (slot, maybe_cell) in open.iter().enumerate() {
        if let Some(cell) = maybe_cell {
            let mut state = cell.state.borrow_mut();
            if matches!(&*state, UpvalueState::Open { .. }) {
                *state = UpvalueState::Closed(stack[base + slot].clone());
            }
        }
    }
}

/// Close open upvalues above a given slot threshold AND clear the entries.
/// Clearing is necessary so that if the handler body later captures locals at
/// the same slots, `make_closure` creates fresh open cells instead of reusing
/// stale closed ones.
fn close_open_upvalues_above(
    open: &mut [Option<Rc<UpvalueCell>>],
    stack: &[Value],
    base: usize,
    min_slot: usize,
) {
    for (slot, maybe_cell) in open.iter_mut().enumerate() {
        if slot >= min_slot {
            if let Some(cell) = maybe_cell {
                let mut state = cell.state.borrow_mut();
                if matches!(&*state, UpvalueState::Open { .. }) {
                    *state = UpvalueState::Closed(stack[base + slot].clone());
                }
            }
            *maybe_cell = None; // Clear so future captures create fresh cells
        }
    }
}
```

Then all 4 sites call `close_open_upvalues(&open, &self.stack, base)` or the `_above` variant. This avoids the `&mut self` borrow conflict since the helper borrows `&[Value]` and `&[Option<Rc<UpvalueCell>>]` separately.

- [ ] **Step 6: Build and run all tests**

Run: `cargo test 2>&1 | grep -E 'FAILED|^test result'`
Expected: All tests pass. Close helpers exist but are no-ops since all cells are still Closed.

- [ ] **Step 7: Commit**

```bash
git add crates/sema-vm/src/vm.rs
git commit -m "refactor(vm): add close_upvalues helpers at Return, TailCall, exception sites"
```

---

## Chunk 3: Add Close Before Non-VM Calls

### Task 3: Close upvalues before native/callback dispatch

**Files:**
- Modify: `crates/sema-vm/src/vm.rs` (3 call dispatch sites)

The NativeFn fallback in `make_closure` creates a **fresh VM** with its own stack. If a closure's upvalue is still Open (pointing into the original VM's stack), the fresh VM can't resolve it. We must close current-frame upvalues before any code path that might trigger the NativeFn fallback.

- [ ] **Step 1: Add close before CallNative dispatch**

At the `CALL_NATIVE` handler (vm.rs:715-740), before the native function is called:

```rust
op::CALL_NATIVE => {
    let native_id = read_u16!(code, pc) as usize;
    let argc = read_u16!(code, pc) as usize;
    self.frames[fi].pc = pc;
    let saved_pc = pc - op::SIZE_CALL_NATIVE;

    // Close open upvalues before non-VM call (native may invoke VM closures via callback)
    if let Some(ref open) = self.frames[fi].open_upvalues {
        close_open_upvalues(open, &self.stack, base);
    }

    // ... rest of CallNative unchanged ...
}
```

- [ ] **Step 2: Add close before call_value non-VM paths**

In `call_value` (vm.rs:1358-1422), before the regular NativeFn call (line 1389) and before `call_callback` (line 1416):

```rust
// At line 1386-1392, before regular native fn call:
// Close upvalues before non-VM native call
if let Some(ref open) = self.frames.last().unwrap().open_upvalues {
    close_open_upvalues(open, &self.stack, self.frames.last().unwrap().base);
}
let func_rc = self.stack[func_idx].as_native_fn_rc().unwrap();
// ...

// At line 1410-1416, before call_callback:
// Close upvalues before callback dispatch
if let Some(ref open) = self.frames.last().unwrap().open_upvalues {
    close_open_upvalues(open, &self.stack, self.frames.last().unwrap().base);
}
let func_val = self.stack[func_idx].clone();
// ...
```

- [ ] **Step 3: Add close before call_value_with non-VM paths**

In `call_value_with` (vm.rs:1457-1495), same treatment. Before native fn call (line 1466) and before `call_callback` (line 1489):

```rust
// Before native fn call in call_value_with:
if let Some(ref open) = self.frames.last().unwrap().open_upvalues {
    close_open_upvalues(open, &self.stack, self.frames.last().unwrap().base);
}

// Before call_callback in call_value_with:
if let Some(ref open) = self.frames.last().unwrap().open_upvalues {
    close_open_upvalues(open, &self.stack, self.frames.last().unwrap().base);
}
```

- [ ] **Step 4: Build and run all tests**

Run: `cargo test 2>&1 | grep -E 'FAILED|^test result'`
Expected: All tests pass. Still no behavioral change — cells are all Closed.

- [ ] **Step 5: Commit**

```bash
git add crates/sema-vm/src/vm.rs
git commit -m "refactor(vm): close upvalues before non-VM calls (prep for open upvalues)"
```

---

## Chunk 4: Behavioral Flip — Open Upvalues

### Task 4: Switch MakeClosure to create Open cells

**Files:**
- Modify: `crates/sema-vm/src/vm.rs:1728-1745` (make_closure)

- [ ] **Step 1: Change MakeClosure to emit Open cells**

In `make_closure` (vm.rs:1728-1745), change the `ParentLocal` branch from eagerly copying the value to creating an Open cell:

```rust
if *is_local {
    let frame = self.frames.last_mut().unwrap();
    let n_locals = frame.closure.func.chunk.n_locals as usize;
    let open = frame
        .open_upvalues
        .get_or_insert_with(|| vec![None; n_locals]);
    let cell = if let Some(existing) = &open[*idx] {
        existing.clone()
    } else {
        // Create an OPEN cell pointing to the stack slot
        let cell = Rc::new(UpvalueCell::new_open(frame.base, *idx));
        open[*idx] = Some(cell.clone());
        cell
    };
    upvalues.push(cell);
}
```

Note: `frame.base` must be captured from `self.frames.last()` — the existing code already has `let base = frame.base;` at line 1720. Use that.

- [ ] **Step 2: Build and run all tests**

Run: `cargo test 2>&1 | grep -E 'FAILED|^test result'`
Expected: All tests pass. The close sites from Chunks 2-3 ensure cells are closed before any code reads them from a different VM context. The dual-write in LoadLocal/StoreLocal still keeps the stack value and cell in sync (the cell now reads from the stack via Open, the stack writes are the canonical path, and the dual-write updates the cell which is... wait, the dual-write writes to the cell's `.value` which no longer exists).

**IMPORTANT**: This step may cause compilation errors or test failures because the dual-write code in StoreLocal still tries to write to the cell. Under the new model, StoreLocal writes to the stack, and the Open cell reads from the stack — so the dual-write is both unnecessary AND uses the wrong API (`.value` is gone). We need to do Task 5 atomically with this task.

- [ ] **Step 3: If tests fail, proceed immediately to Task 5**

### Task 5: Strip dual-write from LoadLocal/StoreLocal (10 opcodes)

**Files:**
- Modify: `crates/sema-vm/src/vm.rs` (10 opcode handlers + `has_open_upvalues` flag)

This task MUST be done atomically with Task 4.

- [ ] **Step 1: Simplify LoadLocal**

Replace vm.rs:568-583:

```rust
op::LOAD_LOCAL => {
    let slot = read_u16!(code, pc) as usize;
    self.stack.push(self.stack[base + slot].clone());
}
```

- [ ] **Step 2: Simplify StoreLocal**

Replace vm.rs:585-596:

```rust
op::STORE_LOCAL => {
    let slot = read_u16!(code, pc) as usize;
    let val = unsafe { pop_unchecked(&mut self.stack) };
    self.stack[base + slot] = val;
}
```

- [ ] **Step 3: Simplify LoadLocal0-3**

Replace vm.rs:1012-1075 (4 opcodes):

```rust
op::LOAD_LOCAL0 => {
    self.stack.push(self.stack[base].clone());
}
op::LOAD_LOCAL1 => {
    self.stack.push(self.stack[base + 1].clone());
}
op::LOAD_LOCAL2 => {
    self.stack.push(self.stack[base + 2].clone());
}
op::LOAD_LOCAL3 => {
    self.stack.push(self.stack[base + 3].clone());
}
```

- [ ] **Step 4: Simplify StoreLocal0-3**

Replace vm.rs:1147-1190 (4 opcodes):

```rust
op::STORE_LOCAL0 => {
    let val = unsafe { pop_unchecked(&mut self.stack) };
    self.stack[base] = val;
}
op::STORE_LOCAL1 => {
    let val = unsafe { pop_unchecked(&mut self.stack) };
    self.stack[base + 1] = val;
}
op::STORE_LOCAL2 => {
    let val = unsafe { pop_unchecked(&mut self.stack) };
    self.stack[base + 2] = val;
}
op::STORE_LOCAL3 => {
    let val = unsafe { pop_unchecked(&mut self.stack) };
    self.stack[base + 3] = val;
}
```

- [ ] **Step 5: Remove has_open_upvalues flag**

Remove from the dispatch loop preamble (vm.rs:376):

```rust
// DELETE: let has_open_upvalues = frame.open_upvalues.is_some();
```

- [ ] **Step 6: Simplify debug_locals**

Replace the open_upvalues branching in `debug_locals` (vm.rs:1923-1930) — locals are always canonical in the stack:

```rust
pub fn debug_locals(&self, frame_idx: usize) -> Vec<crate::debug::DapVariable> {
    let Some(frame) = self.frames.get(frame_idx) else {
        return Vec::new();
    };
    let func = &frame.closure.func;
    let mut vars = Vec::new();
    for &(slot, spur) in &func.local_names {
        let idx = frame.base + slot as usize;
        let val = self.stack.get(idx).cloned().unwrap_or(Value::nil());
        vars.push(crate::debug::DapVariable {
            name: sema_core::resolve(spur),
            value: sema_core::pretty_print(&val, 80),
            type_name: val.type_name().to_string(),
            variables_reference: 0,
        });
    }
    vars
}
```

- [ ] **Step 7: Build and run all tests**

Run: `cargo test 2>&1 | grep -E 'FAILED|^test result'`
Expected: All tests pass. This is the behavioral flip — upvalues are now truly open.

- [ ] **Step 8: Commit**

```bash
git add crates/sema-vm/src/vm.rs
git commit -m "feat(vm): switch to Lua-style open upvalues

Open upvalues hold a stack index instead of an eagerly-copied value.
Upvalues are closed at frame exit (Return, TailCall, exception unwind)
and before non-VM calls (CallNative, call_callback).

LoadLocal/StoreLocal (10 opcodes) are now unconditional stack access,
eliminating the has_open_upvalues branch and dual-write pattern."
```

---

## Chunk 5: Tests and Verification

### Task 6: Add comprehensive dual-eval tests for open upvalue semantics

**Files:**
- Modify: `crates/sema/tests/dual_eval_core_test.rs`

- [ ] **Step 1: Add upvalue-specific dual-eval tests**

Add to the existing `dual_eval_tests!` block in `dual_eval_core_test.rs`, after the existing closure tests:

```rust
// Open upvalue close semantics
upvalue_close_on_return: "(begin
    (define (make-getter)
      (define n 42)
      (lambda () n))
    ((make-getter)))" => Value::int(42),
upvalue_shared_cell: "(begin
    (define (make-shared)
      (define n 0)
      (define inc (lambda () (set! n (+ n 1))))
      (define get (lambda () n))
      (list inc get))
    (define p (make-shared))
    ((first p))
    ((first p))
    ((second p)))" => Value::int(2),
upvalue_late_mutation: "(begin
    (define (make-late)
      (define n 0)
      (define f (lambda () n))
      (set! n 42)
      f)
    ((make-late)))" => Value::int(42),
upvalue_multi_level_close: "(begin
    (define (outer)
      (define x 1)
      (define (middle)
        (lambda () x))
      (set! x 99)
      (middle))
    (((outer))))" => Value::int(99),
upvalue_tail_call_closes: "(begin
    (define captured #f)
    (define (setup)
      (define x 10)
      (set! captured (lambda () x))
      (set! x 20)
      :done)
    (setup)
    (captured))" => Value::int(20),
upvalue_exception_closes: r#"(begin
    (define escaped #f)
    (try
      (begin
        (define x 0)
        (set! escaped (lambda () x))
        (set! x 99)
        (throw "boom"))
      (catch e (escaped))))"# => Value::int(99),
upvalue_closure_via_hof: "(begin
    (define (test)
      (define n 42)
      (map (lambda (x) n) (list 1 2 3)))
    (test))" => Value::list(vec![Value::int(42), Value::int(42), Value::int(42)]),
upvalue_mutable_via_hof: "(begin
    (define (test)
      (define n 0)
      (define inc (lambda (x) (set! n (+ n 1)) n))
      (map inc (list 1 2 3)))
    (test))" => Value::list(vec![Value::int(1), Value::int(2), Value::int(3)]),
```

- [ ] **Step 2: Run the new tests**

Run: `cargo test -p sema-lang --test dual_eval_core_test -- upvalue_ 2>&1`
Expected: All new tests pass on both `_tw` and `_vm` variants.

- [ ] **Step 3: Run the full test suite**

Run: `cargo test 2>&1 | grep -E 'FAILED|^test result'`
Expected: All tests pass.

- [ ] **Step 4: Run nqueens benchmark on VM to verify correctness**

Run: `cargo build --release && ./target/release/sema --vm examples/benchmarks/nqueens.sema`
Expected: Correct output (92 solutions for 8-queens).

- [ ] **Step 5: Run all benchmarks to verify no regression**

Run: `for b in tak nqueens mandelbrot deriv closure-storm higher-order-fold upvalue-counter; do echo "=== $b ===" && hyperfine --warmup 2 --runs 5 "./target/release/sema --vm examples/benchmarks/${b}.sema" 2>&1 | grep 'Time'; done`
Expected: Results within noise of baseline (saved in `/Users/helge/.claude/projects/-Users-helge-code-sema-lisp/memory/benchmarks.md`).

- [ ] **Step 6: Commit tests**

```bash
git add crates/sema/tests/dual_eval_core_test.rs
git commit -m "test(vm): add dual-eval tests for open upvalue close semantics"
```

### Task 7: Update documentation

**Files:**
- Modify: `docs/vm-improvements.md`

- [ ] **Step 1: Move item 7 to Completed section**

Move the "Lua-style open upvalues" section from "Remaining" to "Completed" and update the summary matrix status to "Done".

- [ ] **Step 2: Commit**

```bash
git add docs/vm-improvements.md
git commit -m "docs: mark open upvalues as completed in VM improvements tracker"
```
