# Bytecode VM Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build a complete bytecode VM for Sema — from CoreExpr IR through compilation to VM execution — so that all 712 tests pass through the compiled path with measurable speedup on 1BRC.

**Architecture:** A new `sema-vm` crate implements the compiler pipeline (Value AST → CoreExpr → Chunk bytecode) and VM runtime (dispatch loop, call frames, TCO, closures with upvalues, exception handling). The existing tree-walker (`sema-eval`) is preserved as the macro expansion engine and `eval` fallback. The two runtimes share `EvalContext` and the global `Env`. A `--vm` CLI flag and `Interpreter::eval_compiled()` method let us run both paths side-by-side for correctness verification.

**Tech Stack:** Rust 2021, sema-core types (Value, Env, Spur, EvalContext, SemaError), lasso (interning), hashbrown (HashMap). No new external dependencies.

**Dep flow:** `sema-core ← sema-reader ← sema-vm ← sema-eval` (sema-vm depends on core+reader; sema-eval depends on sema-vm for the compiled path).

**Test commands:**

- `cargo test -p sema-vm` — unit tests for compiler + VM
- `cargo test -p sema --test integration_test` — all 545 integration tests
- `cargo test` — all 712 tests
- `make lint` — `cargo fmt --check` + `cargo clippy -- -D warnings`
- Benchmark: `cargo run --release -- benchmarks/1brc/1brc.sema -- benchmarks/data/bench-1m.txt`

---

## Task 1: Create `sema-vm` crate with Op enum and Chunk/Function structs

**Files:**

- Create: `crates/sema-vm/Cargo.toml`
- Create: `crates/sema-vm/src/lib.rs`
- Create: `crates/sema-vm/src/opcodes.rs`
- Create: `crates/sema-vm/src/chunk.rs`
- Modify: `Cargo.toml` (workspace members)

**Context:** This is the foundational crate. It must sit between sema-reader and sema-eval in the dependency graph. It depends on sema-core (for Value, Spur, SemaError) and sema-reader (for parsing in compile-from-string helpers). It does NOT depend on sema-eval (that would be circular).

**Step 1: Create `crates/sema-vm/Cargo.toml`**

```toml
[package]
name = "sema-vm"
version = "1.2.2"
edition = "2021"

[dependencies]
sema-core.workspace = true
sema-reader.workspace = true
hashbrown.workspace = true
lasso.workspace = true
```

**Step 2: Create `crates/sema-vm/src/opcodes.rs`**

Define the `Op` enum with `#[repr(u8)]`. Start with the minimum set from the investigation doc:

```rust
/// Bytecode opcodes for the Sema VM.
///
/// Stack-based: operands are pushed/popped from the value stack.
/// Variable-length encoding: opcode (1 byte) + operands (u16/u32/i32).
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    // Constants & stack
    Const,       // u16 const_index → push constants[i]
    Nil,         // push nil
    True,        // push #t
    False,       // push #f
    Pop,         // discard TOS
    Dup,         // duplicate TOS

    // Locals (slot-addressed within call frame)
    LoadLocal,   // u16 slot → push locals[slot]
    StoreLocal,  // u16 slot → locals[slot] = pop

    // Upvalues (captured variables from enclosing scopes)
    LoadUpvalue,  // u16 index → push upvalues[i].get()
    StoreUpvalue, // u16 index → upvalues[i].set(pop)

    // Globals (module-level bindings, keyed by Spur)
    LoadGlobal,  // u32 spur → push globals[spur]
    StoreGlobal, // u32 spur → globals[spur] = pop
    DefineGlobal, // u32 spur → globals[spur] = pop (define, not set!)

    // Control flow
    Jump,         // i32 relative offset
    JumpIfFalse,  // i32 relative offset (pop condition)
    JumpIfTrue,   // i32 relative offset (pop condition)

    // Function calls
    Call,         // u16 argc → call TOS-argc with argc args
    TailCall,     // u16 argc → tail call (reuse frame)
    Return,       // return TOS

    // Closures
    MakeClosure,  // u16 func_id, u16 n_upvalues, then n * (u8 is_local, u16 idx)

    // Native function calls (fast path)
    CallNative,   // u16 native_id, u16 argc

    // Data constructors
    MakeList,     // u16 n → pop n values, push list
    MakeVector,   // u16 n → pop n values, push vector
    MakeMap,      // u16 n_pairs → pop 2n values, push map
    MakeHashMap,  // u16 n_pairs → pop 2n values, push hashmap

    // Exception handling
    Throw,        // pop value, throw as exception

    // Generic arithmetic & comparison
    Add,
    Sub,
    Mul,
    Div,
    Negate,
    Not,
    Eq,
    Lt,
    Gt,
    Le,
    Ge,

    // Specialized int arithmetic (fast paths)
    AddInt,
    SubInt,
    MulInt,
    LtInt,
    EqInt,
}
```

**Step 3: Create `crates/sema-vm/src/chunk.rs`**

```rust
use sema_core::{Span, Spur, Value};

/// A compiled code object (bytecode + metadata).
#[derive(Debug, Clone)]
pub struct Chunk {
    pub code: Vec<u8>,
    pub consts: Vec<Value>,
    pub spans: Vec<(u32, Span)>,   // sparse PC → source span mapping (sorted by PC)
    pub max_stack: u16,
    pub n_locals: u16,
    pub exception_table: Vec<ExceptionEntry>,
}

impl Chunk {
    pub fn new() -> Self {
        Chunk {
            code: Vec::new(),
            consts: Vec::new(),
            spans: Vec::new(),
            max_stack: 0,
            n_locals: 0,
            exception_table: Vec::new(),
        }
    }
}

impl Default for Chunk {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct ExceptionEntry {
    pub try_start: u32,
    pub try_end: u32,
    pub handler_pc: u32,
    pub stack_depth: u16,
    pub catch_slot: u16,  // local slot to bind the caught error value
}

/// A compiled function (template for closures).
#[derive(Debug, Clone)]
pub struct Function {
    pub name: Option<Spur>,
    pub chunk: Chunk,
    pub upvalue_descs: Vec<UpvalueDesc>,
    pub arity: u16,
    pub has_rest: bool,
    pub local_names: Vec<(u16, Spur)>,  // slot → name for debug/reify
}

#[derive(Debug, Clone, Copy)]
pub enum UpvalueDesc {
    Local(u16),   // capture from parent's local slot
    Upvalue(u16), // capture from parent's upvalue index
}
```

**Step 4: Create `crates/sema-vm/src/lib.rs`**

```rust
#![allow(clippy::mutable_key_type)]
pub mod chunk;
pub mod opcodes;

pub use chunk::{Chunk, ExceptionEntry, Function, UpvalueDesc};
pub use opcodes::Op;
```

**Step 5: Add to workspace `Cargo.toml`**

Add `"crates/sema-vm"` to the `[workspace] members` list and add `sema-vm = { path = "crates/sema-vm" }` to `[workspace.dependencies]`.

**Step 6: Run `cargo build -p sema-vm` and `make lint`**

Expected: compiles clean, no warnings.

**Step 7: Commit**

```
feat(vm): add sema-vm crate with Op enum, Chunk, and Function structs
```

---

## Task 2: Bytecode emitter helpers and disassembler

**Files:**

- Create: `crates/sema-vm/src/emit.rs`
- Create: `crates/sema-vm/src/disasm.rs`
- Modify: `crates/sema-vm/src/lib.rs`

**Context:** The emitter provides builder methods for appending opcodes with their operands to a Chunk. The disassembler produces human-readable output from a Chunk — essential for debugging the compiler. Both are needed before writing the compiler.

**Step 1: Write tests for the emitter and disassembler**

Add tests to each module. The emitter test: create a Chunk, emit `Const 0` + `Const 1` + `AddInt` + `Return`, verify the byte sequence. The disassembler test: disassemble the same chunk and verify the string output matches expected format.

**Step 2: Implement `emit.rs`**

Helper struct `Emitter` wrapping a `Chunk`:

- `emit_op(&mut self, op: Op)` — push opcode byte
- `emit_u16(&mut self, val: u16)` — push 2 LE bytes
- `emit_u32(&mut self, val: u32)` — push 4 LE bytes
- `emit_i32(&mut self, val: i32)` — push 4 LE bytes (signed)
- `add_const(&mut self, val: Value) -> u16` — add to constant pool, return index
- `emit_const(&mut self, val: Value)` — emit `Op::Const` + constant index
- `emit_span(&mut self, span: Span)` — record PC → span mapping
- `current_pc(&self) -> u32` — current code length
- `patch_jump(&mut self, offset: u32)` — backpatch a jump's i32 operand
- `emit_jump(&mut self, op: Op) -> u32` — emit jump with placeholder, return patch point

**Step 3: Implement `disasm.rs`**

Function `disassemble(chunk: &Chunk, name: Option<&str>) -> String`. Walks the bytecode, decoding each opcode and its operands, producing output like:

```
== <main> ==
0000  CONST 0          ; 1
0003  CONST 1          ; 2
0006  ADD_INT
0007  RETURN
```

For each `Const` opcode, show the constant value from the pool as a comment. For jump opcodes, show the target PC.

**Step 4: Verify**

Run `cargo test -p sema-vm` and `make lint`. All tests pass, lint clean.

**Step 5: Commit**

```
feat(vm): add bytecode emitter and disassembler
```

---

## Task 3: CoreExpr IR and the lowering pass (Value AST → CoreExpr)

**Files:**

- Create: `crates/sema-vm/src/core_expr.rs`
- Create: `crates/sema-vm/src/lower.rs`
- Modify: `crates/sema-vm/src/lib.rs`

**Context:** CoreExpr is the desugared intermediate representation. The lowering pass converts Value AST (from the reader) into CoreExpr by handling all ~35 special forms. At this stage, variables are still referenced by name (Spur) — resolution into local/upvalue/global happens in the next task. Macros must be expanded before lowering, which means the lowerer needs access to the tree-walker — but since sema-vm can't depend on sema-eval, macro expansion is deferred: the lowerer takes already-expanded AST. (Macro expansion will be wired in Task 9 when sema-eval integrates with sema-vm.)

**Step 1: Define `CoreExpr` enum in `core_expr.rs`**

```rust
use sema_core::{Spur, Value};

/// Desugared core language — no macros, no syntactic sugar.
/// Variables are still referenced by name (Spur) at this stage.
/// The variable resolver (Task 4) replaces VarRef names with slot indices.
#[derive(Debug, Clone)]
pub enum CoreExpr {
    Const(Value),
    Var(Spur),
    If {
        test: Box<CoreExpr>,
        then: Box<CoreExpr>,
        else_: Box<CoreExpr>,
    },
    Begin(Vec<CoreExpr>),
    Set(Spur, Box<CoreExpr>),
    Lambda(LambdaDef),
    Call {
        func: Box<CoreExpr>,
        args: Vec<CoreExpr>,
        tail: bool,
    },
    Define(Spur, Box<CoreExpr>),
    Let {
        bindings: Vec<(Spur, CoreExpr)>,
        body: Vec<CoreExpr>,
    },
    LetStar {
        bindings: Vec<(Spur, CoreExpr)>,
        body: Vec<CoreExpr>,
    },
    Letrec {
        bindings: Vec<(Spur, CoreExpr)>,
        body: Vec<CoreExpr>,
    },
    NamedLet {
        name: Spur,
        bindings: Vec<(Spur, CoreExpr)>,
        body: Vec<CoreExpr>,
    },
    Do(DoLoop),
    Try {
        body: Vec<CoreExpr>,
        catch_var: Spur,
        handler: Vec<CoreExpr>,
    },
    Throw(Box<CoreExpr>),
    And(Vec<CoreExpr>),
    Or(Vec<CoreExpr>),
    Quote(Value),
    MakeList(Vec<CoreExpr>),
    MakeVector(Vec<CoreExpr>),
    MakeMap(Vec<(CoreExpr, CoreExpr)>),
    DefineRecordType {
        type_name: Spur,
        ctor_name: Spur,
        pred_name: Spur,
        field_names: Vec<Spur>,
        field_specs: Vec<(Spur, Spur)>, // (field, accessor)
    },
    Module {
        name: Spur,
        exports: Vec<Spur>,
        body: Vec<CoreExpr>,
    },
    Import {
        path: Box<CoreExpr>,
        selective: Vec<Spur>,
    },
    Load(Box<CoreExpr>),
    Eval(Box<CoreExpr>),
}

#[derive(Debug, Clone)]
pub struct LambdaDef {
    pub name: Option<Spur>,
    pub params: Vec<Spur>,
    pub rest: Option<Spur>,
    pub body: Vec<CoreExpr>,
}

#[derive(Debug, Clone)]
pub struct DoLoop {
    pub vars: Vec<DoVar>,
    pub test: Box<CoreExpr>,
    pub result: Vec<CoreExpr>,
    pub body: Vec<CoreExpr>,
}

#[derive(Debug, Clone)]
pub struct DoVar {
    pub name: Spur,
    pub init: CoreExpr,
    pub step: Option<CoreExpr>,
}
```

**Design decisions:**

- `tail` flag on `Call` is set during lowering based on tail position analysis (last expr in body of lambda, begin, let, cond, when, unless; NOT inside try body)
- `And`/`Or` remain distinct (short-circuit semantics need special compilation)
- `Quote` wraps a raw `Value` — no further processing needed
- `Define` is kept separate from `Set` (define creates, set! mutates)
- `DefineRecordType`, `Module`, `Import`, `Load`, `Eval` are kept as IR nodes because they have runtime semantics that the VM must handle
- LLM-specific special forms (`prompt`, `message`, `deftool`, `defagent`) are lowered to `Call` — they become native function calls in the compiled path. The compiler detects them and lowers `(prompt ...)` to a call to a native `__prompt` function, etc.

**Step 2: Implement `lower.rs`**

The lowering function: `pub fn lower(expr: &Value) -> Result<CoreExpr, SemaError>`.

Walks the Value AST and converts each special form to its CoreExpr equivalent. Pattern:

```rust
pub fn lower(expr: &Value) -> Result<CoreExpr, SemaError> {
    lower_expr(expr, false)
}

fn lower_expr(expr: &Value, tail: bool) -> Result<CoreExpr, SemaError> {
    match expr {
        Value::Nil | Value::Bool(_) | Value::Int(_) | Value::Float(_)
        | Value::String(_) | Value::Char(_) | Value::Keyword(_)
        | Value::Bytevector(_) => Ok(CoreExpr::Const(expr.clone())),

        Value::Symbol(spur) => Ok(CoreExpr::Var(*spur)),

        Value::Vector(items) => {
            let exprs = items.iter().map(|v| lower_expr(v, false)).collect::<Result<_, _>>()?;
            Ok(CoreExpr::MakeVector(exprs))
        }

        Value::Map(map) => {
            let pairs = map.iter()
                .map(|(k, v)| Ok((lower_expr(k, false)?, lower_expr(v, false)?)))
                .collect::<Result<_, SemaError>>()?;
            Ok(CoreExpr::MakeMap(pairs))
        }

        Value::HashMap(_) => Ok(CoreExpr::Const(expr.clone())),

        Value::List(items) => {
            if items.is_empty() {
                return Ok(CoreExpr::Const(Value::Nil));
            }
            lower_list(items, tail)
        }

        // Self-evaluating: NativeFn, Lambda, etc.
        other => Ok(CoreExpr::Const(other.clone())),
    }
}
```

The `lower_list` function handles special forms by checking the head symbol:

- `quote` → `CoreExpr::Quote`
- `if` → `CoreExpr::If` (tail propagated to then/else)
- `cond` → lower to nested `If`
- `define` → `CoreExpr::Define` (with function shorthand lowering)
- `defun` → lower to `define` + `lambda`
- `set!` → `CoreExpr::Set`
- `lambda`/`fn` → `CoreExpr::Lambda`
- `let` → `CoreExpr::Let` or `CoreExpr::NamedLet` (tail on last body)
- `let*` → `CoreExpr::LetStar`
- `letrec` → `CoreExpr::Letrec`
- `begin` → `CoreExpr::Begin` (tail on last expr)
- `do` → `CoreExpr::Do`
- `and`/`or` → `CoreExpr::And`/`CoreExpr::Or`
- `when` → lower to `If` with nil else
- `unless` → lower to `If` with swapped branches
- `case` → lower to nested `If` with `or` comparisons
- `defmacro` → `CoreExpr::Call` to a runtime native (macros are expanded before lowering)
- `quasiquote` → expand inline during lowering (recursive walk producing `Const`/`MakeList`/`Call` to `append`)
- `try`/`throw` → `CoreExpr::Try`/`CoreExpr::Throw`
- `prompt`/`message`/`deftool`/`defagent` → `CoreExpr::Call` to native builtins
- `module`/`import`/`load` → corresponding CoreExpr nodes
- `eval` → `CoreExpr::Eval`
- `macroexpand` → `CoreExpr::Call` to native
- `with-budget` → `CoreExpr::Call` to native
- `delay` → lower to `Lambda` (zero-arg closure wrapping the body)
- `force` → `CoreExpr::Call` to native `force`
- `define-record-type` → `CoreExpr::DefineRecordType`
- Anything else → function `Call`

**Tail position rules** (critical for TCO correctness):

- `begin`: only last expr is tail
- `if`: both then and else branches are tail
- `let`/`let*`/`letrec`/`named-let`: only last body expr is tail
- `cond`: last expr of each clause is tail
- `and`/`or`: only last operand is tail
- `when`/`unless`: only last body expr is tail
- `do`: result exprs — only last is tail; body is NOT tail
- `try`: body is NOT tail (handler must be reachable); handler last expr IS tail
- `lambda` body: last expr is tail (new tail context)

**Step 3: Write unit tests**

Test each special form lowering:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use sema_reader::read_one;

    fn lower_str(input: &str) -> CoreExpr {
        let val = read_one(input).unwrap();
        lower(&val).unwrap()
    }

    #[test]
    fn test_lower_literal() {
        match lower_str("42") {
            CoreExpr::Const(Value::Int(42)) => {}
            other => panic!("expected Const(42), got {other:?}"),
        }
    }

    #[test]
    fn test_lower_if() {
        match lower_str("(if #t 1 2)") {
            CoreExpr::If { .. } => {}
            other => panic!("expected If, got {other:?}"),
        }
    }

    #[test]
    fn test_lower_lambda() {
        match lower_str("(lambda (x) x)") {
            CoreExpr::Lambda(_) => {}
            other => panic!("expected Lambda, got {other:?}"),
        }
    }

    #[test]
    fn test_lower_define() {
        match lower_str("(define x 42)") {
            CoreExpr::Define(_, _) => {}
            other => panic!("expected Define, got {other:?}"),
        }
    }

    #[test]
    fn test_lower_cond_to_if() {
        // (cond (#t 1)) should lower to If
        match lower_str("(cond (#t 1))") {
            CoreExpr::If { .. } => {}
            other => panic!("expected If from cond, got {other:?}"),
        }
    }

    #[test]
    fn test_lower_when_to_if() {
        match lower_str("(when #t 42)") {
            CoreExpr::If { .. } => {}
            other => panic!("expected If from when, got {other:?}"),
        }
    }

    #[test]
    fn test_tail_position_begin() {
        // In (begin 1 (f x)), the call (f x) should have tail=true
        let expr = lower_str("(lambda () (begin 1 (f x)))");
        // Verify the last call in begin has tail=true
        match expr {
            CoreExpr::Lambda(def) => {
                assert_eq!(def.body.len(), 1); // begin is the body
                match &def.body[0] {
                    CoreExpr::Begin(exprs) => {
                        match exprs.last().unwrap() {
                            CoreExpr::Call { tail, .. } => assert!(*tail),
                            _ => panic!("expected Call"),
                        }
                    }
                    _ => panic!("expected Begin"),
                }
            }
            _ => panic!("expected Lambda"),
        }
    }
}
```

**Step 4: Run `cargo test -p sema-vm` and `make lint`**

**Step 5: Commit**

```
feat(vm): add CoreExpr IR and lowering pass for all special forms
```

---

## Task 4: Variable resolver (local/upvalue/global analysis)

**Files:**

- Create: `crates/sema-vm/src/resolve.rs`
- Modify: `crates/sema-vm/src/core_expr.rs` (add ResolvedExpr or annotate CoreExpr)
- Modify: `crates/sema-vm/src/lib.rs`

**Context:** This is the critical compilation phase (as Steel's implementation shows). The resolver walks the CoreExpr tree and determines, for each variable reference, whether it's a local (slot index), upvalue (captured from enclosing scope), or global (module-level binding). It also detects which locals are captured and mutated (requiring boxing for `set!` through upvalues). The output is an annotated CoreExpr where every `Var(Spur)` is replaced with `Var(VarRef)`.

**Step 1: Define `VarRef` and `ResolvedExpr`**

Add to `core_expr.rs`:

```rust
#[derive(Debug, Clone)]
pub enum VarResolution {
    Local { slot: u16 },
    Upvalue { index: u16 },
    Global { spur: Spur },
}

#[derive(Debug, Clone)]
pub struct VarRef {
    pub name: Spur,
    pub resolution: VarResolution,
}
```

Create a `ResolvedExpr` enum that mirrors `CoreExpr` but uses `VarRef` instead of raw `Spur` for variable references. Alternatively, modify `CoreExpr::Var` to hold either a raw `Spur` or a `VarRef` — use a two-pass approach where the resolver transforms the tree in-place.

**Recommended approach:** Define `ResolvedExpr` as a separate enum. This makes the pipeline type-safe: `Value → CoreExpr → ResolvedExpr → Chunk`. The compiler (Task 5) takes `ResolvedExpr` as input, not `CoreExpr`.

**Step 2: Implement `resolve.rs`**

The resolver maintains a stack of `Scope` structs:

```rust
struct Scope {
    locals: Vec<Local>,
    upvalues: Vec<UpvalueDesc>,
    kind: ScopeKind,
}

struct Local {
    name: Spur,
    slot: u16,
    is_captured: bool,
    is_mutated: bool,
}

enum ScopeKind {
    Function, // lambda boundary — upvalue capture point
    Block,    // let/begin — no capture boundary
}
```

The algorithm:

1. **Resolve pass:** Walk the CoreExpr. For each `Var(spur)`:
   - Search current scope's locals → `VarResolution::Local { slot }`
   - Search enclosing scopes up to a function boundary → add `UpvalueDesc` entries → `VarResolution::Upvalue { index }`
   - Not found → `VarResolution::Global { spur }`
2. **For `Set(spur, expr)`:** Same resolution, plus mark the target as `is_mutated = true`
3. **For `Lambda`:** Push a new `Function` scope. After resolving the body, collect `upvalues` vec and `n_locals` count.
4. **For `Let`/`LetStar`/`Letrec`:** Push a `Block` scope with the bound variables as locals.
5. **For `Define`:** If inside a function scope, create a local. If at top-level, treat as global.

**Important:** Locals that are both `is_captured` and `is_mutated` must be boxed (wrapped in `UpvalueCell` at runtime). The resolver marks these; the compiler emits boxing instructions.

**Step 3: Write unit tests**

```rust
#[test]
fn test_resolve_local() {
    // (lambda (x) x) → x resolves to Local { slot: 0 }
    let core = lower_str("(lambda (x) x)");
    let resolved = resolve(&core).unwrap();
    // Check that x in body is Local slot 0
}

#[test]
fn test_resolve_global() {
    // (+ 1 2) → + resolves to Global
    let core = lower_str("(+ 1 2)");
    let resolved = resolve(&core).unwrap();
    // Check that + is Global
}

#[test]
fn test_resolve_upvalue() {
    // (lambda (x) (lambda () x)) → inner x is Upvalue
    let core = lower_str("(lambda (x) (lambda () x))");
    let resolved = resolve(&core).unwrap();
}

#[test]
fn test_resolve_captured_mutated() {
    // (lambda () (let ((n 0)) (lambda () (set! n (+ n 1)) n)))
    // n is captured AND mutated → needs boxing
    let core = lower_str("(lambda () (let ((n 0)) (lambda () (set! n (+ n 1)) n)))");
    let resolved = resolve(&core).unwrap();
}
```

**Step 4: Run `cargo test -p sema-vm` and `make lint`**

**Step 5: Commit**

```
feat(vm): add variable resolver (local/upvalue/global analysis)
```

---

## Task 5: Bytecode compiler (ResolvedExpr → Chunk)

**Files:**

- Create: `crates/sema-vm/src/compiler.rs`
- Modify: `crates/sema-vm/src/lib.rs`

**Context:** The compiler walks a `ResolvedExpr` tree and emits bytecode into a `Chunk` using the `Emitter`. Each `Lambda` produces a separate `Function`. The compiler maintains a `FunctionStore` for all compiled functions (referenced by index).

**Step 1: Define the compiler struct**

```rust
pub struct Compiler {
    functions: Vec<Function>,
    emitter: Emitter,
    current_scope: CompilerScope,
}

struct CompilerScope {
    n_locals: u16,
    stack_depth: u16,
    loop_exits: Vec<u32>, // for do loop break points
}
```

**Step 2: Implement compilation for each ResolvedExpr variant**

Key compilation patterns:

- `Const(v)` → `emit_const(v)`
- `Var(ref_)` → `LoadLocal`/`LoadUpvalue`/`LoadGlobal` based on resolution
- `If { test, then, else_ }` → compile test, `JumpIfFalse` to else, compile then, `Jump` over else, compile else
- `Begin(exprs)` → compile each; last inherits tail context
- `Set(ref_, expr)` → compile expr, `StoreLocal`/`StoreUpvalue`/`StoreGlobal`
- `Define(spur, expr)` → compile expr, `DefineGlobal spur` (or `StoreLocal` if in function)
- `Lambda(def)` → create a new `Function`, compile its body, emit `MakeClosure`
- `Call { func, args, tail }` → compile func, compile args, emit `Call argc` or `TailCall argc`
- `Let { bindings, body }` → compile each init, `StoreLocal` for each binding, compile body
- `And(exprs)` → compile first, `JumpIfFalse` to short-circuit, compile rest... last inherits tail
- `Or(exprs)` → compile first, `JumpIfTrue` to short-circuit, compile rest...
- `Do(loop)` → compile inits, loop top: compile test, `JumpIfFalse` to body, compile result + `Return`/jump, compile body, compile steps, `Jump` back to top
- `Try { body, catch_var, handler }` → add exception table entry, compile body, jump over handler, compile handler
- `Throw(expr)` → compile expr, `Throw`
- `MakeList(exprs)` → compile each, `MakeList n`
- `MakeVector(exprs)` → compile each, `MakeVector n`
- `MakeMap(pairs)` → compile each key+value, `MakeMap n`
- `Quote(v)` → `emit_const(v)`
- `Import`/`Load`/`Eval`/`Module`/`DefineRecordType` → emit as native calls with special handling

**Step 3: Write unit tests**

```rust
#[test]
fn test_compile_add() {
    // (+ 1 2) → compile and disassemble, verify opcodes
    let chunk = compile_str("(+ 1 2)").unwrap();
    let dis = disassemble(&chunk, Some("test"));
    assert!(dis.contains("CONST"));
    assert!(dis.contains("RETURN"));
}

#[test]
fn test_compile_if() {
    let chunk = compile_str("(if #t 1 2)").unwrap();
    let dis = disassemble(&chunk, Some("test"));
    assert!(dis.contains("JUMP_IF_FALSE"));
}

#[test]
fn test_compile_lambda() {
    let (funcs, chunk) = compile_str_full("(lambda (x) (+ x 1))").unwrap();
    assert!(funcs.len() >= 1); // at least the inner lambda
}
```

**Step 4: Run `cargo test -p sema-vm` and `make lint`**

**Step 5: Commit**

```
feat(vm): add bytecode compiler (ResolvedExpr → Chunk)
```

---

## Task 6: VM dispatch loop with call frames and stack

**Files:**

- Create: `crates/sema-vm/src/vm.rs`
- Modify: `crates/sema-vm/src/lib.rs`

**Context:** The VM executes bytecode by decoding and dispatching opcodes in a loop. It maintains a value stack, a call frame stack, and the global environment.

**Step 1: Define VM structs**

```rust
use std::cell::RefCell;
use std::rc::Rc;

use sema_core::{Env, EvalContext, SemaError, Spur, Value};

use crate::{Chunk, Function, Op, UpvalueDesc};

/// A mutable cell for variables that are both captured and mutated.
#[derive(Debug)]
pub struct UpvalueCell {
    pub value: RefCell<Value>,
}

/// A runtime closure (function template + captured upvalues).
#[derive(Debug, Clone)]
pub struct Closure {
    pub func: Rc<Function>,
    pub upvalues: Vec<Rc<UpvalueCell>>,
}

struct CallFrame {
    closure: Rc<Closure>,
    pc: usize,
    base: usize, // stack base for this frame's locals
}

pub struct VM {
    stack: Vec<Value>,
    frames: Vec<CallFrame>,
    globals: Rc<Env>,
    functions: Vec<Rc<Function>>,
}
```

**Step 2: Implement the dispatch loop**

```rust
impl VM {
    pub fn execute(
        &mut self,
        closure: Rc<Closure>,
        ctx: &EvalContext,
    ) -> Result<Value, SemaError> {
        self.push_frame(closure);
        self.run(ctx)
    }

    fn run(&mut self, ctx: &EvalContext) -> Result<Value, SemaError> {
        loop {
            let frame = self.frames.last_mut().unwrap();
            let op = self.read_op(frame);
            match op {
                Op::Const => { /* read u16, push constants[idx] */ }
                Op::Nil => self.push(Value::Nil),
                Op::True => self.push(Value::Bool(true)),
                Op::False => self.push(Value::Bool(false)),
                Op::Pop => { self.pop(); }
                Op::Dup => { let v = self.peek().clone(); self.push(v); }
                Op::LoadLocal => { /* read u16 slot, push stack[base + slot] */ }
                Op::StoreLocal => { /* read u16 slot, stack[base + slot] = pop */ }
                Op::LoadUpvalue => { /* read u16 idx, push upvalues[idx].value.borrow().clone() */ }
                Op::StoreUpvalue => { /* read u16 idx, *upvalues[idx].value.borrow_mut() = pop */ }
                Op::LoadGlobal => { /* read u32 spur, push globals.get(spur) */ }
                Op::StoreGlobal => { /* read u32 spur, globals.set_existing(spur, pop) */ }
                Op::DefineGlobal => { /* read u32 spur, globals.set(spur, pop) */ }
                Op::Jump => { /* read i32 offset, adjust pc */ }
                Op::JumpIfFalse => { /* read i32, pop, if !truthy jump */ }
                Op::JumpIfTrue => { /* read i32, pop, if truthy jump */ }
                Op::Call => { /* call with argc */ }
                Op::TailCall => { /* reuse frame + jump */ }
                Op::Return => {
                    let result = self.pop();
                    self.frames.pop();
                    if self.frames.is_empty() {
                        return Ok(result);
                    }
                    // Restore stack to caller's state
                    self.push(result);
                }
                Op::MakeClosure => { /* create Closure from Function + captured upvalues */ }
                Op::CallNative => { /* lookup native by id, call with args from stack */ }
                Op::MakeList => { /* pop n values, push Value::List */ }
                Op::MakeVector => { /* pop n values, push Value::Vector */ }
                Op::MakeMap => { /* pop 2n values, push Value::Map */ }
                Op::MakeHashMap => { /* pop 2n values, push Value::HashMap */ }
                Op::Throw => { /* pop value, return Err(SemaError::UserException) */ }
                Op::Add => { /* generic add: int+int, float+float, string concat */ }
                Op::Sub | Op::Mul | Op::Div => { /* generic arithmetic */ }
                Op::Negate => { /* negate TOS */ }
                Op::Not => { /* logical not TOS */ }
                Op::Eq | Op::Lt | Op::Gt | Op::Le | Op::Ge => { /* comparisons */ }
                Op::AddInt => { /* fast path: pop 2 ints, push int; fallback to generic Add */ }
                Op::SubInt | Op::MulInt | Op::LtInt | Op::EqInt => { /* int fast paths */ }
            }
        }
    }
}
```

**Step 3: Handle `Call` and `TailCall`**

For `Call`:

1. Pop argc args from stack
2. Pop the function value
3. If `Value::Lambda` → wrap in Closure, push new CallFrame, continue dispatch
4. If `Value::NativeFn` → call directly, push result
5. If `Value::Keyword` → keyword-as-function lookup
6. If `Closure` (from MakeClosure) → push new CallFrame

For `TailCall`:

1. Pop argc args
2. Pop the function value
3. Copy args into current frame's local slots (overwriting)
4. Reset current frame's PC and closure
5. Continue dispatch (no new frame pushed)

**Step 4: Handle exceptions (try/throw)**

The `Throw` opcode returns a `SemaError::UserException`. The dispatch loop catches errors and checks the current frame's exception table. If the PC falls within a `try_start..try_end` range:

1. Restore stack to `stack_depth`
2. Push the caught error value (converted via `error_to_value`)
3. Store to `catch_slot`
4. Jump to `handler_pc`

If no handler found, pop frame and propagate up.

**Step 5: Write unit tests**

Test the VM by compiling small programs and executing them:

```rust
#[test]
fn test_vm_add() {
    let result = eval_compiled("(+ 1 2)").unwrap();
    assert_eq!(result, Value::Int(3));
}

#[test]
fn test_vm_if_true() {
    let result = eval_compiled("(if #t 42 99)").unwrap();
    assert_eq!(result, Value::Int(42));
}

#[test]
fn test_vm_lambda_call() {
    let result = eval_compiled("((lambda (x) (+ x 1)) 41)").unwrap();
    assert_eq!(result, Value::Int(42));
}

#[test]
fn test_vm_closure() {
    let result = eval_compiled("(let ((x 10)) ((lambda () x)))").unwrap();
    assert_eq!(result, Value::Int(10));
}

#[test]
fn test_vm_tail_call() {
    // Should not stack overflow with TCO
    let result = eval_compiled("(begin (define (loop n) (if (= n 0) 0 (loop (- n 1)))) (loop 100000))").unwrap();
    assert_eq!(result, Value::Int(0));
}
```

**Step 6: Run `cargo test -p sema-vm` and `make lint`**

**Step 7: Commit**

```
feat(vm): add VM dispatch loop with call frames, TCO, and exception handling
```

---

## Task 7: Native function registry and `CallNative` dispatch

**Files:**

- Create: `crates/sema-vm/src/natives.rs`
- Modify: `crates/sema-vm/src/vm.rs`
- Modify: `crates/sema-vm/src/compiler.rs`

**Context:** The ~380 native functions registered in the Env need numeric IDs for `CallNative` dispatch. The compiler looks up known native functions by Spur when compiling `Call` nodes — if the callee is a `Global` that resolves to a `NativeFn`, emit `CallNative` instead of `Call` for a fast path.

**Step 1: Define `NativeRegistry`**

```rust
pub struct NativeRegistry {
    fns: Vec<Rc<NativeFn>>,
    name_to_id: hashbrown::HashMap<Spur, u16>,
}
```

Built during interpreter initialization by walking the global Env and extracting all `NativeFn` values.

**Step 2: Compiler integration**

When the compiler encounters `Call { func: Var(ref_), args, .. }` where `ref_.resolution == Global` and the global is a known native:

- Emit `CallNative native_id argc` instead of `LoadGlobal + Call`

This requires passing the `NativeRegistry` to the compiler.

**Step 3: VM dispatch**

```rust
Op::CallNative => {
    let native_id = self.read_u16() as usize;
    let argc = self.read_u16() as usize;
    let args_start = self.stack.len() - argc;
    let result = (self.natives.fns[native_id].func)(ctx, &self.stack[args_start..])?;
    self.stack.truncate(args_start);
    self.push(result);
}
```

**Step 4: Tests**

```rust
#[test]
fn test_native_string_length() {
    let result = eval_compiled("(string-length \"hello\")").unwrap();
    assert_eq!(result, Value::Int(5));
}
```

**Step 5: Run `cargo test -p sema-vm` and `make lint`**

**Step 6: Commit**

```
feat(vm): add native function registry and CallNative fast path
```

---

## Task 8: Upvalue boxing, closures, and `set!` through captures

**Files:**

- Modify: `crates/sema-vm/src/vm.rs`
- Modify: `crates/sema-vm/src/compiler.rs`

**Context:** When a local variable is both captured by a closure AND mutated via `set!`, it must be boxed into an `UpvalueCell`. The compiler emits boxing instructions; the VM manages cell allocation and indirection.

**Step 1: Compiler changes**

For locals marked as captured+mutated by the resolver:

- After initializing the local, emit a `Box` pseudo-op that wraps the value in an `UpvalueCell`
- `LoadLocal` and `StoreLocal` for boxed locals go through `Deref`/`SetCell` indirection

**Approach:** Add `Op::Box` (wrap TOS in UpvalueCell), `Op::Deref` (unwrap UpvalueCell → value), `Op::SetCell` (pop value + pop cell → store value in cell). Add these to the Op enum.

**Step 2: VM changes**

Implement the new opcodes. UpvalueCell is stored as a special value — either add a `Value::Cell(Rc<UpvalueCell>)` variant or use the existing `Rc<RefCell<Value>>` pattern. Using a new value variant is cleaner for the VM but adds to the public type. Alternative: keep cells as an internal VM mechanism using `Rc<UpvalueCell>` stored directly on the stack (requires the stack to hold `enum StackValue { Value(Value), Cell(Rc<UpvalueCell>) }`).

**Recommended:** Use `Rc<UpvalueCell>` stored within the `Value` stack slots — represent cells as `Value::Lambda`-level internal state. Actually, the simplest approach: store `Rc<UpvalueCell>` in the upvalues array of the Closure, and in the local slot store either the raw value (not captured/mutated) or the UpvalueCell wrapper (captured+mutated). The VM knows which slots are boxed from the function's metadata.

**Step 3: Tests**

```rust
#[test]
fn test_closure_mutation() {
    let result = eval_compiled("
        (let ((make-counter (lambda ()
            (let ((n 0))
                (lambda ()
                    (set! n (+ n 1))
                    n)))))
            (let ((c (make-counter)))
                (c) (c) (c)))
    ").unwrap();
    assert_eq!(result, Value::Int(3));
}

#[test]
fn test_shared_mutable_upvalue() {
    let result = eval_compiled("
        (let ((n 0))
            (let ((inc (lambda () (set! n (+ n 1))))
                  (get (lambda () n)))
                (inc) (inc)
                (get)))
    ").unwrap();
    assert_eq!(result, Value::Int(2));
}
```

**Step 4: Run `cargo test -p sema-vm` and `make lint`**

**Step 5: Commit**

```
feat(vm): add upvalue boxing for captured+mutated variables
```

---

## Task 9: Wire VM into Interpreter (dual-path execution)

**Files:**

- Modify: `crates/sema-eval/Cargo.toml` (add sema-vm dependency)
- Modify: `crates/sema-eval/src/eval.rs`
- Modify: `crates/sema-eval/src/lib.rs`
- Modify: `Cargo.toml` (workspace deps)

**Context:** The Interpreter gets a `eval_compiled()` method that runs expressions through the VM pipeline: parse → macro expand (tree-walker) → lower → resolve → compile → VM execute. The tree-walker path is preserved. Both paths share the same `EvalContext` and global `Env`.

**Step 1: Add dependency**

Add `sema-vm.workspace = true` to `crates/sema-eval/Cargo.toml`.

**Step 2: Add `eval_compiled` to Interpreter**

```rust
impl Interpreter {
    pub fn eval_compiled(&self, expr: &Value) -> EvalResult {
        // 1. Lower Value AST to CoreExpr
        let core = sema_vm::lower(expr)?;
        // 2. Resolve variables
        let resolved = sema_vm::resolve(&core)?;
        // 3. Compile to bytecode
        let (functions, chunk) = sema_vm::compile(&resolved, &self.native_registry)?;
        // 4. Execute in VM
        let mut vm = sema_vm::VM::new(
            self.global_env.clone(),
            functions,
            self.native_registry.clone(),
        );
        vm.execute_chunk(&chunk, &self.ctx)
    }

    pub fn eval_str_compiled(&self, input: &str) -> EvalResult {
        let (exprs, spans) = sema_reader::read_many_with_spans(input)?;
        self.ctx.merge_span_table(spans);
        let mut result = Value::Nil;
        for expr in &exprs {
            result = self.eval_compiled(expr)?;
        }
        Ok(result)
    }
}
```

**Step 3: Handle macro expansion**

Before lowering, check if the expression contains macro calls. If so, expand using the tree-walker:

```rust
fn expand_macros(&self, expr: &Value) -> Result<Value, SemaError> {
    // Use the existing tree-walker to detect and expand macros
    // This is a recursive walk: if head is a macro, apply it and recurse
    // Non-macro forms are returned as-is
}
```

**Step 4: Handle `eval`, `import`, `load` at the VM level**

These require calling back into the full pipeline:

- `eval`: the VM calls `Interpreter::eval_compiled` recursively (with reified env)
- `import`/`load`: the VM calls back to load, parse, expand, compile, and execute a file

For now, these can fall back to the tree-walker. This is acceptable for v1 — the VM handles the hot computational paths, while `eval`/`import`/`load` use the tree-walker.

**Step 5: Build a `NativeRegistry` during interpreter init**

After registering stdlib, walk the global env and extract all `NativeFn` values into the registry.

**Step 6: Tests**

Run ALL existing integration tests through the compiled path:

```rust
// In integration_test.rs, add a helper:
fn eval_compiled(input: &str) -> Value {
    let interp = Interpreter::new();
    interp.eval_str_compiled(input).expect(&format!("failed to compile-eval: {input}"))
}

// Duplicate key tests to verify parity
#[test]
fn test_compiled_arithmetic() {
    assert_eq!(eval_compiled("(+ 1 2)"), Value::Int(3));
    assert_eq!(eval_compiled("(- 10 3)"), Value::Int(7));
}
```

**Step 7: Run `cargo test` and `make lint`**

**Step 8: Commit**

```
feat(vm): wire bytecode VM into Interpreter with dual-path execution
```

---

## Task 10: `--vm` CLI flag and compiled integration tests

**Files:**

- Modify: `crates/sema/src/main.rs` (add `--vm` flag)
- Create: `crates/sema/tests/vm_integration_test.rs` (or add `_compiled` variants to existing tests)

**Context:** Add a `--vm` CLI flag that routes all execution through the compiled path. Create a parallel integration test file that runs the existing test suite through `eval_str_compiled` to verify behavioral parity.

**Step 1: Add `--vm` flag to clap CLI**

```rust
#[derive(Parser)]
struct Cli {
    // ... existing fields ...
    #[arg(long, help = "Use bytecode VM instead of tree-walker")]
    vm: bool,
}
```

When `--vm` is set, the REPL and file evaluation use `eval_compiled` instead of `eval`.

**Step 2: Create VM integration tests**

Port the most critical integration tests to verify compiled execution. Start with:

- Arithmetic, comparison, boolean ops
- Define, set!, let/let\*/letrec
- Lambda, closures, recursion
- TCO (tail call optimization)
- Try/catch/throw
- Named let, do loops
- Quasiquote
- Keyword-as-function
- String operations (native calls)
- List operations (map, filter, fold)
- Define-record-type

**Step 3: Run `cargo test` and `make lint`**

**Step 4: Commit**

```
feat(vm): add --vm CLI flag and compiled integration tests
```

---

## Task 11: Profile 1BRC with flamegraph (baseline)

**Files:**

- Create: `benchmarks/profile.sh` (profiling script)

**Context:** Before measuring VM speedup, establish a baseline flamegraph of the tree-walker on 1BRC to confirm where time is spent. This validates (or invalidates) the assumption that env lookups and Rc traffic dominate.

**Step 1: Install profiling tools**

```bash
cargo install flamegraph  # if not already installed
```

**Step 2: Create profiling script**

```bash
#!/bin/bash
# Profile 1BRC with samply or cargo-flamegraph
set -e

echo "Building release-with-debug..."
cargo build --profile release-with-debug

echo "Running 1BRC tree-walker (baseline)..."
# On macOS, use samply or instruments
samply record ./target/release-with-debug/sema benchmarks/1brc/1brc.sema -- benchmarks/data/bench-1m.txt

# Or with cargo flamegraph:
# cargo flamegraph --profile release-with-debug -- benchmarks/1brc/1brc.sema -- benchmarks/data/bench-1m.txt
```

**Step 3: Run and capture baseline**

Expected output: flamegraph showing time distribution across:

- `eval_value` / `eval_value_inner` / `eval_step`
- `Env::get` (hash lookups)
- `Value::clone` / Rc refcount operations
- `try_eval_special` (special form dispatch)
- `call_value` / `apply_lambda`
- Native function calls (string/split, assoc, etc.)

**Step 4: Document findings**

Add results to `benchmarks/1brc/profile-results.md`:

- Top 10 hottest functions with % time
- Key bottleneck confirmed or refuted
- Implications for VM optimization priorities

**Step 5: Commit**

```
chore: add 1BRC profiling script and baseline flamegraph results
```

---

## Task 12: Run 1BRC through VM and measure speedup

**Files:**

- Modify: `benchmarks/1brc/profile-results.md`

**Context:** With the VM wired into the Interpreter, run the 1BRC benchmark through the compiled path and compare against tree-walker baseline.

**Step 1: Run tree-walker baseline (3 runs, take median)**

```bash
time cargo run --release -- benchmarks/1brc/1brc.sema -- benchmarks/data/bench-1m.txt
```

**Step 2: Run VM (3 runs, take median)**

```bash
time cargo run --release -- --vm benchmarks/1brc/1brc.sema -- benchmarks/data/bench-1m.txt
```

**Step 3: Profile VM execution**

Flamegraph the VM path to see where time is now spent. Expected new hotspots:

- VM dispatch loop
- Native function calls (unchanged)
- Stack push/pop and Value::clone

**Step 4: Document comparison**

| Path        | 1BRC 1M rows (median) | Speedup |
| ----------- | --------------------- | ------- |
| Tree-walker | ~Xms                  | 1×      |
| Bytecode VM | ~Yms                  | X/Y×    |

**Step 5: Commit**

```
chore: add 1BRC VM benchmark results
```

---

## Execution Notes

### Dependency ordering

Tasks 1-5 are strictly sequential (each builds on the previous).
Task 6 depends on tasks 1-2 (needs Op + Chunk + emitter).
Task 7 depends on tasks 5-6.
Task 8 depends on tasks 6-7.
Task 9 depends on tasks 5-8.
Task 10 depends on task 9.
Task 11 is independent — can run in parallel with any task.
Task 12 depends on tasks 10-11.

### Testing strategy

- Each task has its own unit tests in `sema-vm`
- Task 10 adds integration tests verifying behavioral parity with tree-walker
- The ultimate correctness check: all 712 existing tests pass through both paths

### Risk mitigation

- If macro expansion causes issues in Task 9, fall back to tree-walker for macro-containing expressions
- If `import`/`load`/`eval` are complex, leave them as tree-walker-delegated for v1
- If 1BRC doesn't show expected speedup, profile to identify the bottleneck (likely native function call overhead for string/split, hashmap operations)

### What's NOT in this plan (deferred to future work)

- Module compilation caching
- WASM playground integration
- Tracing GC
- NaN boxing / tagged values
- Constant folding optimization
- Inline caching
- REPL-specific compilation
