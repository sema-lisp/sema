# VM Improvements #7, #8, #9 Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Three independent VM improvements — unify CoreExpr/ResolvedExpr types (#8), add per-instruction inline cache for globals (#9), and switch to Lua-style open upvalues (#7).

**Architecture:** Each improvement is independent and executed in order: #8 (reduces maintenance surface), #9 (perf: global access), #7 (perf/correctness: upvalue model). All changes are in `crates/sema-vm/src/`.

**Tech Stack:** Rust, sema-vm crate. No new dependencies.

---

## Chunk 1: Task 1 — Unify CoreExpr and ResolvedExpr (#8)

### Overview

Replace the duplicated `CoreExpr` and `ResolvedExpr` enums (~30 variants each, ~260 lines total) with a single generic `Expr<V>` parameterized on the variable binding type. Type aliases preserve the existing names:

```rust
pub type CoreExpr = Expr<Spur>;
pub type ResolvedExpr = Expr<VarRef>;
```

### Files

- **Modify:** `crates/sema-vm/src/core_expr.rs` — merge enums, genericize helper structs
- **Modify:** `crates/sema-vm/src/lib.rs` — remove `ResolvedLambda` from `pub use` re-exports
- **Modify:** `crates/sema-vm/src/lower.rs` — update `LambdaDef` constructions to include empty upvalues/n_locals
- **Modify:** `crates/sema-vm/src/optimize.rs` — match arms stay structurally identical (V=Spur)
- **Modify:** `crates/sema-vm/src/resolve.rs` — update match on input (V=Spur), construction of output (V=VarRef)
- **Modify:** `crates/sema-vm/src/compiler.rs` — update match on ResolvedExpr (V=VarRef)

### Task 1.1: Define Expr\<V\> and generic helper types

- [ ] **Step 1: Rewrite core_expr.rs with the unified generic enum**

Replace both `CoreExpr` and `ResolvedExpr` with a single `Expr<V>`:

```rust
use sema_core::{Span, Spur, Value};

// --- Variable resolution types (unchanged) ---
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VarResolution {
    Local { slot: u16 },
    Upvalue { index: u16 },
    Global { spur: Spur },
}

#[derive(Debug, Clone, Copy)]
pub struct VarRef {
    pub name: Spur,
    pub resolution: VarResolution,
}

// Re-export UpvalueDesc from chunk.rs
pub use crate::chunk::UpvalueDesc;

// --- Unified expression IR ---

/// Core expression IR, generic over variable binding type V.
/// - `Expr<Spur>` = unresolved (CoreExpr): variables referenced by name
/// - `Expr<VarRef>` = resolved (ResolvedExpr): variables resolved to slots/upvalues/globals
#[derive(Debug, Clone)]
pub enum Expr<V> {
    Const(Value),
    Var(V),
    If {
        test: Box<Expr<V>>,
        then: Box<Expr<V>>,
        else_: Box<Expr<V>>,
    },
    Begin(Vec<Expr<V>>),
    Set(V, Box<Expr<V>>),
    Lambda(LambdaDef<V>),
    Call {
        func: Box<Expr<V>>,
        args: Vec<Expr<V>>,
        tail: bool,
    },
    Define(Spur, Box<Expr<V>>),
    Let {
        bindings: Vec<(V, Expr<V>)>,
        body: Vec<Expr<V>>,
    },
    LetStar {
        bindings: Vec<(V, Expr<V>)>,
        body: Vec<Expr<V>>,
    },
    Letrec {
        bindings: Vec<(V, Expr<V>)>,
        body: Vec<Expr<V>>,
    },
    Do(DoLoop<V>),
    Try {
        body: Vec<Expr<V>>,
        catch_var: V,
        handler: Vec<Expr<V>>,
    },
    Throw(Box<Expr<V>>),
    And(Vec<Expr<V>>),
    Or(Vec<Expr<V>>),
    Quote(Value),
    MakeList(Vec<Expr<V>>),
    MakeVector(Vec<Expr<V>>),
    MakeMap(Vec<(Expr<V>, Expr<V>)>),
    Defmacro {
        name: Spur,
        params: Vec<Spur>,
        rest: Option<Spur>,
        body: Vec<Expr<V>>,
    },
    DefineRecordType {
        type_name: Spur,
        ctor_name: Spur,
        pred_name: Spur,
        field_names: Vec<Spur>,
        field_specs: Vec<(Spur, Spur)>,
    },
    Module {
        name: Spur,
        exports: Vec<Spur>,
        body: Vec<Expr<V>>,
    },
    Import {
        path: Box<Expr<V>>,
        selective: Vec<Spur>,
    },
    Load(Box<Expr<V>>),
    Eval(Box<Expr<V>>),
    Prompt(Vec<PromptEntry<V>>),
    Message {
        role: Box<Expr<V>>,
        parts: Vec<Expr<V>>,
    },
    Deftool {
        name: Spur,
        description: Box<Expr<V>>,
        parameters: Box<Expr<V>>,
        handler: Box<Expr<V>>,
    },
    Defagent {
        name: Spur,
        options: Box<Expr<V>>,
    },
    Delay(Box<Expr<V>>),
    Force(Box<Expr<V>>),
    Macroexpand(Box<Expr<V>>),
    Spanned(Span, Box<Expr<V>>),
}

/// Type aliases preserving existing names.
pub type CoreExpr = Expr<Spur>;
pub type ResolvedExpr = Expr<VarRef>;

#[derive(Debug, Clone)]
pub enum PromptEntry<V> {
    RoleContent { role: String, parts: Vec<Expr<V>> },
    Expr(Expr<V>),
}

#[derive(Debug, Clone)]
pub struct LambdaDef<V> {
    pub name: Option<Spur>,
    pub params: Vec<Spur>,
    pub rest: Option<Spur>,
    pub body: Vec<Expr<V>>,
    /// Upvalue descriptors (empty for unresolved CoreExpr).
    pub upvalues: Vec<UpvalueDesc>,
    /// Number of local slots (0 for unresolved CoreExpr).
    pub n_locals: u16,
}

#[derive(Debug, Clone)]
pub struct DoLoop<V> {
    pub vars: Vec<DoVar<V>>,
    pub test: Box<Expr<V>>,
    pub result: Vec<Expr<V>>,
    pub body: Vec<Expr<V>>,
}

#[derive(Debug, Clone)]
pub struct DoVar<V> {
    pub name: V,
    pub init: Expr<V>,
    pub step: Option<Expr<V>>,
}
```

Key design decisions:
- `LambdaDef<V>` always has `upvalues` and `n_locals` fields (empty/zero for CoreExpr). This avoids a second type parameter.
- `Define(Spur, ...)` always uses `Spur` because defines create global bindings by name, even after resolution.
- `DefineRecordType` fields stay as `Vec<Spur>` since they're always names.
- Removed the separate `ResolvedLambda`, `ResolvedDoLoop`, `ResolvedDoVar`, `ResolvedPromptEntry` types.

- [ ] **Step 2: Verify core_expr.rs compiles in isolation**

Run: `cargo check -p sema-vm 2>&1 | head -50`

Expected: Compilation errors in lower.rs, optimize.rs, resolve.rs, compiler.rs (they reference the old types). That's expected — we fix them in the next steps.

### Task 1.2: Update lower.rs (CoreExpr constructions)

lower.rs only *constructs* CoreExpr variants (never matches on them). The changes are:
1. `LambdaDef` constructions need `upvalues: vec![], n_locals: 0`
2. `DoVar` constructions: field name stays the same (it's `Spur` for CoreExpr)
3. `PromptEntry` → `PromptEntry<Spur>` (but type alias makes this implicit)

- [ ] **Step 3: Update all LambdaDef constructions in lower.rs**

Find every `LambdaDef { name, params, rest, body }` and add the two new fields:

```rust
LambdaDef {
    name,
    params,
    rest,
    body,
    upvalues: vec![],
    n_locals: 0,
}
```

There are approximately 5 construction sites in lower.rs (in `lower_lambda`, `lower_defun`, `lower_defmacro`, and desugared forms like named-let → letrec+lambda).

- [ ] **Step 4: Verify lower.rs compiles**

Run: `cargo check -p sema-vm 2>&1 | grep "lower.rs" | head -20`

Expected: No errors from lower.rs. Remaining errors from optimize.rs, resolve.rs, compiler.rs.

### Task 1.3: Update optimize.rs

optimize.rs matches on `CoreExpr` (V=Spur) and constructs new `CoreExpr` values. Since the enum is now `Expr<Spur>`, the match arms are structurally identical — the only changes are:
1. `DoLoop` → `DoLoop<Spur>` (implicit via type alias)
2. `DoVar` → `DoVar<Spur>` (implicit)
3. `PromptEntry` → `PromptEntry<Spur>` (implicit)
4. `LambdaDef` constructions need `upvalues: vec![], n_locals: 0`

- [ ] **Step 5: Update optimize.rs match arms and constructions**

The match arms should mostly just work since `CoreExpr` is a type alias for `Expr<Spur>`. Fix any type mismatches, primarily in Lambda handling where `LambdaDef` was previously a non-generic type.

- [ ] **Step 6: Verify optimize.rs compiles**

Run: `cargo check -p sema-vm 2>&1 | grep "optimize.rs" | head -20`

### Task 1.4: Update resolve.rs

resolve.rs is the most involved: it matches on `CoreExpr` (Expr<Spur>) and constructs `ResolvedExpr` (Expr<VarRef>). Key changes:
1. Input match arms: `CoreExpr::Var(spur)` stays the same
2. Output constructions: `ResolvedExpr::Lambda(ResolvedLambda { ... })` → `ResolvedExpr::Lambda(LambdaDef { ..., upvalues, n_locals })`
3. Remove references to the old `ResolvedLambda`, `ResolvedDoLoop`, `ResolvedDoVar`, `ResolvedPromptEntry` types

- [ ] **Step 7: Update resolve.rs**

Main changes:
- `resolve_lambda()`: construct `LambdaDef<VarRef>` instead of `ResolvedLambda`
- `resolve_do()`: construct `DoLoop<VarRef>` and `DoVar<VarRef>` instead of `ResolvedDoLoop`/`ResolvedDoVar`
- `resolve_try()`: catch_var is now `VarRef` (already was)
- Let/LetStar/Letrec bindings: `Vec<(VarRef, Expr<VarRef>)>` (already was)
- Prompt entries: construct `PromptEntry<VarRef>` instead of `ResolvedPromptEntry`

Example for `resolve_lambda`:
```rust
// Before:
Ok(ResolvedExpr::Lambda(ResolvedLambda {
    name: def.name,
    params: def.params.clone(),
    rest: def.rest,
    body,
    upvalues: fn_scope.upvalues,
    n_locals: fn_scope.next_slot,
}))

// After (identical — LambdaDef<VarRef> has the same fields):
Ok(ResolvedExpr::Lambda(LambdaDef {
    name: def.name,
    params: def.params.clone(),
    rest: def.rest,
    body,
    upvalues: fn_scope.upvalues,
    n_locals: fn_scope.next_slot,
}))
```

- [ ] **Step 8: Verify resolve.rs compiles**

Run: `cargo check -p sema-vm 2>&1 | grep "resolve.rs" | head -20`

### Task 1.5: Update compiler.rs

compiler.rs matches on `ResolvedExpr` (Expr<VarRef>). Changes:
1. `compile_lambda` receives `&LambdaDef<VarRef>` instead of `&ResolvedLambda`
2. Field access stays the same (upvalues, n_locals, params, rest, body, name)

- [ ] **Step 9: Update compiler.rs**

Replace `ResolvedLambda` references with `LambdaDef<VarRef>`. The `compile_lambda` function signature changes:
```rust
// Before:
fn compile_lambda(&mut self, def: &ResolvedLambda) -> Result<(), SemaError>
// After:
fn compile_lambda(&mut self, def: &LambdaDef<VarRef>) -> Result<(), SemaError>
```

Also update `ResolvedDoLoop`/`ResolvedDoVar` references to `DoLoop<VarRef>`/`DoVar<VarRef>`, and `ResolvedPromptEntry` to `PromptEntry<VarRef>`.

- [ ] **Step 10: Verify compiler.rs compiles**

Run: `cargo check -p sema-vm 2>&1 | grep "compiler.rs" | head -20`

### Task 1.6: Fix remaining references and run tests

- [ ] **Step 11: Fix any remaining references to old types across the workspace**

Search for `ResolvedLambda`, `ResolvedDoLoop`, `ResolvedDoVar`, `ResolvedPromptEntry` in the workspace and update them.

**Critical:** `crates/sema-vm/src/lib.rs` re-exports `ResolvedLambda` on line 18. Remove it from the `pub use` statement. The export list should become:
```rust
CoreExpr, DoLoop, DoVar, LambdaDef, PromptEntry, ResolvedExpr, VarRef, VarResolution,
```

Run: `cargo check -p sema-vm 2>&1 | head -30`

Check other crates that import from sema-vm:
Run: `cargo check 2>&1 | head -50`

- [ ] **Step 12: Run the full test suite**

Run: `make test`

Expected: All tests pass. This is a pure refactor — no behavioral changes.

- [ ] **Step 13: Commit**

```bash
git add crates/sema-vm/src/core_expr.rs crates/sema-vm/src/lib.rs \
  crates/sema-vm/src/lower.rs crates/sema-vm/src/optimize.rs \
  crates/sema-vm/src/resolve.rs crates/sema-vm/src/compiler.rs
git commit -m "refactor(vm): unify CoreExpr and ResolvedExpr into generic Expr<V>

Replace duplicated CoreExpr/ResolvedExpr enums (~260 lines) with a single
Expr<V> parameterized on variable binding type. Type aliases preserve
existing names: CoreExpr = Expr<Spur>, ResolvedExpr = Expr<VarRef>.

Removes ~130 lines of duplicated enum variants and 4 duplicated helper
structs (ResolvedLambda, ResolvedDoLoop, ResolvedDoVar, ResolvedPromptEntry)."
```

---

## Chunk 2: Task 2 — Per-Instruction Inline Cache for Globals (#9)

### Overview

Replace the 256-slot direct-mapped global cache with per-instruction inline caches. Each `LoadGlobal`/`CallGlobal` instruction gets a dedicated cache slot, eliminating hash collisions entirely. Cache hit = version check + array index (O(1), no hashing).

### Approach

Add a `u16 cache_slot` operand to `LoadGlobal` and `CallGlobal` instructions. The compiler assigns incrementing cache slot IDs during compilation. The VM allocates a `Vec<InlineCacheEntry>` sized to the total number of global-access instructions.

**Instruction encoding changes:**
- `LoadGlobal`: `op(1) + spur(4) + cache_slot(2)` = 7 bytes (was 5)
- `CallGlobal`: `op(1) + spur(4) + argc(2) + cache_slot(2)` = 9 bytes (was 7)

### Files

- **Modify:** `crates/sema-vm/src/opcodes.rs` — update SIZE constants
- **Modify:** `crates/sema-vm/src/compiler.rs` — assign cache slot IDs, emit new operand
- **Modify:** `crates/sema-vm/src/vm.rs` — replace global_cache with per-instruction cache, update LoadGlobal/CallGlobal handlers
- **Modify:** `crates/sema-vm/src/emit.rs` — no changes needed (uses generic emit_u16)
- **Modify:** `crates/sema-vm/src/disasm.rs` — update disassembly to show cache_slot
- **Modify:** `crates/sema-vm/src/serialize.rs` — update PC advancement for new instruction sizes
- **Modify:** `crates/sema-vm/src/chunk.rs` — add `n_global_cache_slots` to Chunk

### Task 2.1: Write a benchmark test for global access

- [ ] **Step 1: Add a test that exercises global lookup performance**

In `crates/sema-vm/src/vm.rs` tests section, add a test that confirms correct behavior with the cache (we'll use this to verify the refactor doesn't break anything):

```rust
#[test]
fn test_global_cache_multiple_globals() {
    // Access many different globals to stress the cache
    let globals = make_test_env();
    let ctx = EvalContext::new();
    // not and list are non-intrinsic globals that go through LoadGlobal/CallGlobal
    let result = eval_str(
        "(define x 1) (define y 2) (define z 3) (+ x (+ y z))",
        &globals,
        &ctx,
    )
    .unwrap();
    assert_eq!(result, Value::int(6));
}
```

- [ ] **Step 2: Run the test**

Run: `cargo test -p sema-vm -- test_global_cache_multiple_globals`

Expected: PASS

### Task 2.2: Update instruction encoding

- [ ] **Step 3: Update SIZE constants in opcodes.rs**

```rust
// Before:
pub const SIZE_OP_U32: usize = 5;  // LoadGlobal, StoreGlobal, DefineGlobal

// After — split out LoadGlobal size (StoreGlobal/DefineGlobal stay at 5):
pub const SIZE_LOAD_GLOBAL: usize = 7;   // op + u32 spur + u16 cache_slot
pub const SIZE_CALL_GLOBAL: usize = 9;   // op + u32 spur + u16 argc + u16 cache_slot
```

**Important:** `SIZE_OP_U32` (5 bytes) must NOT change — it's still used by `StoreGlobal` and `DefineGlobal`. The new constants are additions, not replacements. Everywhere that currently groups `LoadGlobal | StoreGlobal | DefineGlobal` in one match arm must be split: `LoadGlobal` gets its own arm with the new size, `StoreGlobal | DefineGlobal` keep `SIZE_OP_U32`.

- [ ] **Step 4: Add n_global_cache_slots to Chunk and cache_offset to Function**

In `chunk.rs`, add a field to Chunk:
```rust
pub struct Chunk {
    // ...existing fields...
    /// Number of inline cache slots for global accesses in this chunk.
    pub n_global_cache_slots: u16,
}
```

In `chunk.rs`, add a field to Function:
```rust
pub struct Function {
    // ...existing fields...
    /// Offset into the VM's inline cache for this function's cache slots.
    /// Set at VM creation time, not at compile time.
    pub cache_offset: usize,
}
```

Update `Chunk::new()` to initialize `n_global_cache_slots` to 0.
Update `Function` construction to initialize `cache_offset` to 0 (set later at VM creation).

**Serialization:** Add `n_global_cache_slots` to the `.semac` function table section. Bump `FORMAT_VERSION` in `serialize.rs`. `cache_offset` is NOT serialized (it's computed at load time).

- [ ] **Step 5: Update compiler to assign cache slots and emit them**

In `compiler.rs`, add a `next_cache_slot: u16` counter to the `Compiler` struct. Increment it for each `LoadGlobal` and `CallGlobal` emitted:

```rust
// For LoadGlobal:
self.emit.emit_op(Op::LoadGlobal);
self.emit.emit_u32(spur_to_u32(spur));
let slot = self.next_cache_slot;
self.next_cache_slot += 1;
self.emit.emit_u16(slot);

// For CallGlobal:
self.emit.emit_op(Op::CallGlobal);
self.emit.emit_u32(spur_to_u32(spur));
self.emit.emit_u16(argc);
let slot = self.next_cache_slot;
self.next_cache_slot += 1;
self.emit.emit_u16(slot);
```

Set `chunk.n_global_cache_slots = self.next_cache_slot` in `finish()`.

Inner lambdas (compiled by nested Compiler instances) get their own independent `next_cache_slot` counters, since each Function has its own Chunk.

- [ ] **Step 6: Check compilation succeeds**

Run: `cargo check -p sema-vm 2>&1 | head -30`

Expected: Errors in vm.rs, disasm.rs, serialize.rs (they read bytecode with old sizes). Fix in next steps.

### Task 2.3: Update VM dispatch

- [ ] **Step 7: Replace global_cache with per-instruction cache in VM**

In `vm.rs`, replace the global cache:

```rust
// Before:
const GLOBAL_CACHE_SIZE: usize = 256;
// ...
global_cache: [(u32, u64, Value); GLOBAL_CACHE_SIZE],

// After:
/// Per-instruction inline cache for global accesses.
/// Each LoadGlobal/CallGlobal instruction has a dedicated slot.
/// Entry: (env_version, cached_value). Version 0 = empty.
inline_cache: Vec<(u64, Value)>,
```

In `VM::new()`, assign `cache_offset` to each function and compute total cache size:

```rust
fn assign_cache_offsets(
    closure: &Closure,
    functions: &mut [Rc<Function>],
) -> usize {
    let mut offset = closure.func.chunk.n_global_cache_slots as usize;
    for f in functions {
        Rc::make_mut(f).cache_offset = offset;
        offset += f.chunk.n_global_cache_slots as usize;
    }
    offset // total cache slots
}
```

The main closure always starts at offset 0. Each Function gets the next available offset.

Initialize: `inline_cache: vec![(0, Value::nil()); total_slots]`

Each frame needs to know its cache base offset. Add `cache_base: usize` to `CallFrame`. When pushing a frame, set `cache_base = closure.func.cache_offset`.

**`new_with_rc_functions` path (stdlib HOF closures):** This factory creates VMs for closures called from map/filter/etc. It must also assign cache offsets and size the cache. Add the same `assign_cache_offsets` call (or compute from the functions vec).

- [ ] **Step 8: Update LoadGlobal handler**

```rust
op::LOAD_GLOBAL => {
    let bits = read_u32!(code, pc);
    let cache_slot = read_u16!(code, pc) as usize + cache_base;
    let version = self.globals.version.get();
    let entry = &self.inline_cache[cache_slot];
    if entry.0 == version {
        self.stack.push(entry.1.clone());
    } else {
        let spur: Spur = unsafe { std::mem::transmute::<u32, Spur>(bits) };
        match self.globals.get(spur) {
            Some(val) => {
                self.inline_cache[cache_slot] = (version, val.clone());
                self.stack.push(val);
            }
            None => {
                let err = SemaError::Unbound(resolve_spur(spur));
                handle_err!(self, fi, pc, err, pc - 7, 'dispatch);
            }
        }
    }
}
```

Key difference from before: no spur-based hashing, no tag matching — just version check on the dedicated slot. Cache hit is two comparisons (version check) + array index.

- [ ] **Step 9: Update CallGlobal handler**

Same pattern: read `cache_slot` from the new operand position, use dedicated slot for lookup.

```rust
op::CALL_GLOBAL => {
    let bits = read_u32!(code, pc);
    let argc = read_u16!(code, pc) as usize;
    let cache_slot = read_u16!(code, pc) as usize + cache_base;
    // ... rest uses inline_cache[cache_slot] instead of global_cache[slot]
}
```

- [ ] **Step 10: Update `saved_pc` calculations**

All `saved_pc` calculations for error handling in LoadGlobal and CallGlobal need updating:
- LoadGlobal: `saved_pc = pc - SIZE_LOAD_GLOBAL` (was `pc - SIZE_OP_U32`)
- CallGlobal: `saved_pc = pc - SIZE_CALL_GLOBAL` (was `pc - SIZE_CALL_GLOBAL`, but size changed)

### Task 2.4: Update disassembler and serializer

- [ ] **Step 11: Update disasm.rs**

Update `LoadGlobal` disassembly to read the extra `cache_slot` u16:
```rust
Op::LoadGlobal => {
    let spur_bits = read_u32(code, pc + 1);
    let cache_slot = read_u16(code, pc + 5);
    // ... format with cache_slot
    pc += 7;
}
```

Update `CallGlobal` similarly (read cache_slot after argc):
```rust
Op::CallGlobal => {
    let spur_bits = read_u32(code, pc + 1);
    let argc = read_u16(code, pc + 5);
    let cache_slot = read_u16(code, pc + 7);
    pc += 9;
}
```

- [ ] **Step 12: Update serialize.rs**

Update PC advancement in serialization/deserialization for the new instruction sizes.

**Critical:** The existing code groups `LoadGlobal | StoreGlobal | DefineGlobal => pc + 5`. This must be split:
```rust
Op::LoadGlobal => pc + 7,                              // op + u32 spur + u16 cache_slot
Op::StoreGlobal | Op::DefineGlobal => pc + 5,          // op + u32 spur (unchanged)
Op::CallGlobal => pc + 9,                              // op + u32 spur + u16 argc + u16 cache_slot
```

Also serialize `n_global_cache_slots` in the function table section and bump `FORMAT_VERSION`.

- [ ] **Step 13: Update all other bytecode walkers**

Search for any code that walks bytecode by opcode. **Every instance where `LoadGlobal` is grouped with `StoreGlobal | DefineGlobal` in a match arm must be split.** Update LoadGlobal/CallGlobal PC advancement.

Key locations (all need arm splits):
- `compiler.rs`: `patch_closure_func_ids` — currently `Op::LoadGlobal | Op::StoreGlobal | Op::DefineGlobal => pc += 1 + 4`. Split to `Op::LoadGlobal => pc += 1 + 4 + 2` and `Op::StoreGlobal | Op::DefineGlobal => pc += 1 + 4`
- `compiler.rs`: `extract_ops` test helper — currently `Op::LoadGlobal | Op::StoreGlobal | Op::DefineGlobal => pc += 4`. Split to `Op::LoadGlobal => pc += 6` and `Op::StoreGlobal | Op::DefineGlobal => pc += 4`. Also update `Op::CallGlobal => pc += 6` to `pc += 8`.
- `disasm.rs`: currently groups all three globals in one arm at `pc += 5`. Split.
- Any validation code in vm.rs

### Task 2.5: Run tests and commit

- [ ] **Step 14: Run the full test suite**

Run: `make test`

Expected: All tests pass.

- [ ] **Step 15: Run serialize roundtrip tests specifically**

Run: `cargo test -p sema --test serialize_roundtrip_test`

Expected: All pass (serialization handles new instruction sizes).

- [ ] **Step 16: Commit**

```bash
git add crates/sema-vm/src/
git commit -m "perf(vm): per-instruction inline cache for global lookups

Replace 256-slot direct-mapped global cache with per-instruction inline
caches. Each LoadGlobal/CallGlobal instruction gets a dedicated cache
slot, eliminating hash collisions. Cache hit = version check + array
index (O(1), zero hashing overhead).

LoadGlobal: 5 → 7 bytes (added u16 cache_slot operand)
CallGlobal: 7 → 9 bytes (added u16 cache_slot operand)"
```

---

## Chunk 3: Task 3 — Lua-Style Open Upvalues (#7)

### Overview

Replace the current eager-capture upvalue model (where StoreLocal dual-writes to both stack and upvalue cell) with Lua-style open upvalues that point directly at stack slots. This eliminates:
1. The `has_open_upvalues` branch on every LoadLocal/StoreLocal (10 opcodes)
2. The dual-write to upvalue cells on every StoreLocal to captured variables
3. Code duplication across specialized store opcodes

**How it works:**
- **Open upvalue**: holds an absolute stack index. Reads/writes go through the stack directly.
- **Closed upvalue**: holds a `Value`. Created when the owning frame exits.
- `MakeClosure`: creates Open upvalues pointing at parent's stack slots.
- `StoreLocal`: just writes the stack. No upvalue check needed.
- `LoadLocal`: just reads the stack. No upvalue check needed.
- `Return`/`TailCall`/exception unwind: closes all open upvalues for the exiting frame by copying the stack value into the cell.

### Files

- **Modify:** `crates/sema-vm/src/vm.rs` — UpvalueCell (defined here, not chunk.rs), make_closure, LoadLocal, StoreLocal, LoadUpvalue, StoreUpvalue, Return, TailCall, exception handling, debug_locals, debug_upvalues

### Task 3.1: Change UpvalueCell to Open/Closed model

- [ ] **Step 1: Write tests for the new upvalue behavior**

Add tests in `crates/sema-vm/src/vm.rs` (test section) that exercise upvalue mutation patterns:

```rust
#[test]
fn test_upvalue_mutation_after_close() {
    // Inner closure captures x, mutates it, then outer returns.
    // The closed upvalue should reflect the final value.
    let globals = make_test_env();
    let ctx = EvalContext::new();
    let result = eval_str(
        "(define (make-counter)
           (define count 0)
           (lambda ()
             (set! count (+ count 1))
             count))
         (define c (make-counter))
         (c) (c) (c)",
        &globals,
        &ctx,
    ).unwrap();
    assert_eq!(result, Value::int(3));
}

#[test]
fn test_upvalue_shared_between_closures() {
    let globals = make_test_env();
    let ctx = EvalContext::new();
    let result = eval_str(
        "(define (make-pair)
           (define x 0)
           (list
             (lambda () (set! x (+ x 1)) x)
             (lambda () x)))
         (define p (make-pair))
         ((first p))
         ((first p))
         ((first (rest p)))",
        &globals,
        &ctx,
    ).unwrap();
    assert_eq!(result, Value::int(2));
}
```

- [ ] **Step 2: Run tests to verify they pass with current implementation**

Run: `cargo test -p sema-vm -- test_upvalue_mutation_after_close test_upvalue_shared_between_closures`

Expected: PASS (current implementation handles these correctly)

- [ ] **Step 3: Update UpvalueCell to Open/Closed enum**

```rust
/// State of a captured variable.
#[derive(Debug, Clone)]
pub enum UpvalueState {
    /// Points to a stack slot (variable is still live in its declaring frame).
    Open(usize),
    /// Owns the value (frame has exited, value was copied from stack).
    Closed(Value),
}

#[derive(Debug)]
pub struct UpvalueCell {
    pub state: RefCell<UpvalueState>,
}

impl UpvalueCell {
    pub fn open(stack_index: usize) -> Self {
        UpvalueCell {
            state: RefCell::new(UpvalueState::Open(stack_index)),
        }
    }

    pub fn closed(value: Value) -> Self {
        UpvalueCell {
            state: RefCell::new(UpvalueState::Closed(value)),
        }
    }
}
```

### Task 3.2: Update make_closure

- [ ] **Step 4: Update make_closure to create Open upvalues**

When capturing a parent local, create an Open upvalue pointing at the stack slot:

```rust
// Before (eager capture):
let val = self.stack[base + slot].clone();
let cell = Rc::new(UpvalueCell::new(val));

// After (open upvalue):
let stack_index = base + slot;
let cell = Rc::new(UpvalueCell::open(stack_index));
```

For deduplication: if the same local is captured by multiple closures, they must share the same UpvalueCell. The `open_upvalues` on CallFrame already handles this — check if a cell exists before creating a new one.

When capturing a parent upvalue (UpvalueDesc::ParentUpvalue), just clone the Rc as before — the cell is already either Open or Closed.

### Task 3.3: Simplify LoadLocal and StoreLocal

- [ ] **Step 5: Remove upvalue checks from all LoadLocal variants**

Remove the `has_open_upvalues` check from LoadLocal, LoadLocal0-3:

```rust
// Before:
let val = if has_open_upvalues {
    if let Some(ref open) = self.frames[fi].open_upvalues {
        if let Some(Some(cell)) = open.get(slot) {
            cell.value.borrow().clone()
        } else {
            self.stack[base + slot].clone()
        }
    } else { unreachable!() }
} else {
    self.stack[base + slot].clone()
};

// After:
let val = self.stack[base + slot].clone();
```

- [ ] **Step 6: Remove dual-write from all StoreLocal variants**

Remove the upvalue sync from StoreLocal, StoreLocal0-3:

```rust
// Before:
let val = unsafe { pop_unchecked(&mut self.stack) };
self.stack[base + slot] = val.clone();
if has_open_upvalues {
    if let Some(ref open) = self.frames[fi].open_upvalues {
        if let Some(Some(cell)) = open.get(slot) {
            *cell.value.borrow_mut() = val;
        }
    }
}

// After:
let val = unsafe { pop_unchecked(&mut self.stack) };
self.stack[base + slot] = val;
```

Note: no `.clone()` needed since we're just moving the value to the stack.

- [ ] **Step 7: Remove `has_open_upvalues` from the dispatch loop**

Remove the `has_open_upvalues` variable from the outer dispatch loop. It's no longer needed. Remove it from the frame-reload code too.

### Task 3.4: Update LoadUpvalue and StoreUpvalue

- [ ] **Step 8: Update LoadUpvalue to handle Open/Closed**

```rust
op::LOAD_UPVALUE => {
    let idx = read_u16!(code, pc) as usize;
    let cell = &self.frames[fi].closure.upvalues[idx];
    let val = match *cell.state.borrow() {
        UpvalueState::Open(stack_idx) => self.stack[stack_idx].clone(),
        UpvalueState::Closed(ref v) => v.clone(),
    };
    self.stack.push(val);
}
```

- [ ] **Step 9: Update StoreUpvalue to handle Open/Closed**

```rust
op::STORE_UPVALUE => {
    let idx = read_u16!(code, pc) as usize;
    let val = unsafe { pop_unchecked(&mut self.stack) };
    let cell = &self.frames[fi].closure.upvalues[idx];
    let state = cell.state.borrow();
    match *state {
        UpvalueState::Open(stack_idx) => {
            drop(state);
            self.stack[stack_idx] = val;
        }
        UpvalueState::Closed(_) => {
            drop(state);
            *cell.state.borrow_mut() = UpvalueState::Closed(val);
        }
    }
}
```

Note: We borrow shared first to read the state, drop it, then either write to the stack (Open) or borrow mutably to update the cell (Closed). This avoids holding a mutable borrow across the stack write.

### Task 3.5: Close upvalues on frame exit

- [ ] **Step 10: Add close_upvalues as a free function**

Use a free function (not a method) to avoid borrow-checker conflicts. In `tail_call_vm_closure`, calling `self.close_upvalues(frame)` where `frame` borrows from `self.frames` while also borrowing `self.stack` would be rejected. A free function taking the stack and the open_upvalues directly avoids this:

```rust
/// Close all open upvalues for a frame.
/// Copies stack values into the upvalue cells so they survive stack truncation.
fn close_open_upvalues(
    stack: &[Value],
    open_upvalues: &Option<Vec<Option<Rc<UpvalueCell>>>>,
) {
    if let Some(ref open) = open_upvalues {
        for cell_opt in open {
            if let Some(cell) = cell_opt {
                let mut state = cell.state.borrow_mut();
                if let UpvalueState::Open(idx) = *state {
                    *state = UpvalueState::Closed(stack[idx].clone());
                }
            }
        }
    }
}
```

- [ ] **Step 11: Call close_open_upvalues in Return handler**

```rust
op::RETURN => {
    let result = if !self.stack.is_empty() {
        unsafe { pop_unchecked(&mut self.stack) }
    } else {
        Value::nil()
    };
    let frame = self.frames.pop().unwrap();
    close_open_upvalues(&self.stack, &frame.open_upvalues);  // ← Close before truncation!
    self.stack.truncate(frame.base);
    if self.frames.is_empty() {
        return Ok(crate::debug::VmExecResult::Finished(result));
    }
    self.stack.push(result);
    continue 'dispatch;
}
```

- [ ] **Step 12: Close upvalues in tail_call_vm_closure**

When TailCall reuses the current frame, the old frame's upvalues must be closed **before** `copy_args_to_locals` overwrites the stack slots:

```rust
fn tail_call_vm_closure(&mut self, closure: Rc<Closure>, argc: usize) -> Result<(), SemaError> {
    // Close upvalues BEFORE any stack mutation
    {
        let frame = self.frames.last().unwrap();
        close_open_upvalues(&self.stack, &frame.open_upvalues);
    }
    // Now safe to borrow mutably for frame reuse
    let frame = self.frames.last_mut().unwrap();
    frame.open_upvalues = None;
    // ... existing frame-reuse logic (copy_args_to_locals, etc.) ...
}
```

- [ ] **Step 13: Close upvalues during exception unwinding**

In `handle_exception`, when popping frames to find a handler, close upvalues for each popped frame:

```rust
// In the frame-popping loop:
let frame = self.frames.pop().unwrap();
close_open_upvalues(&self.stack, &frame.open_upvalues);
self.stack.truncate(frame.base);
```

- [ ] **Step 13b: Update debug_locals and debug_upvalues**

`debug_locals()` reads `cell.value.borrow()` — the `.value` field no longer exists. Update to match on `UpvalueState`:
```rust
// Before: cell.value.borrow().clone()
// After:
match *cell.state.borrow() {
    UpvalueState::Open(idx) => self.stack[idx].clone(),
    UpvalueState::Closed(ref v) => v.clone(),
}
```

Same change for `debug_upvalues()` which also reads `uv.value.borrow()`.

### Task 3.6: Run tests and commit

- [ ] **Step 14: Run upvalue-specific tests**

Run: `cargo test -p sema-vm -- upvalue`

Expected: All pass.

- [ ] **Step 15: Run the full test suite**

Run: `make test`

Expected: All tests pass.

- [ ] **Step 16: Run dual-eval tests (tree-walker and VM must agree)**

Run: `cargo test -p sema --test dual_eval_test`

Expected: All pass. Both backends produce identical results for closure/upvalue patterns.

- [ ] **Step 17: Commit**

```bash
git add crates/sema-vm/src/vm.rs
git commit -m "perf(vm): switch to Lua-style open upvalues

Open upvalues point directly at stack slots while the declaring frame
is alive. Closed on frame exit (Return, TailCall, exception unwind).

- Removes has_open_upvalues branch from all 10 LoadLocal/StoreLocal opcodes
- Removes dual-write (stack + cell) from all 5 StoreLocal variants
- Upvalue overhead moves to LoadUpvalue/StoreUpvalue (Open/Closed check)
  and Return (close scan) — both less frequent than local access"
```

---

## Task 4: Cleanup and Documentation

- [ ] **Step 1: Close item #6 in vm-improvements.md**

Update the status from "Open" to "Won't fix" with a note:

```markdown
| 6 | Document TCO limitations          | 1 hr      | Low      | None   | Won't fix — native tail calls don't cause frame growth |
```

- [ ] **Step 2: Mark items #7, #8, #9 as Done**

- [ ] **Step 3: Update vm-status.md if test counts changed**

- [ ] **Step 4: Update CHANGELOG.md unreleased section**

Add entries for all three improvements.

- [ ] **Step 5: Commit docs**

```bash
git add docs/ CHANGELOG.md
git commit -m "docs: update vm-improvements status and changelog for #7, #8, #9"
```
