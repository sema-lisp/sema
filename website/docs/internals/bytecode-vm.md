# Bytecode VM

::: tip The Evaluator
Sema compiles to bytecode and runs on the VM. Every entry point — the CLI, the REPL, the embedding API, `eval`, `import`/`load`, macros, and async/await — compiles to bytecode and runs on the VM.
:::

## Overview

Sema's evaluator is a bytecode VM. The VM compiles Sema source code into stack-based bytecode for fast execution. On compute-heavy workloads it is fast — 500 iterations of the TAK benchmark `(tak 18 12 6)` run in roughly 1.9 s (≈3.7 ms/iteration) in a plain release build on a modern laptop. The PGO-optimized release binaries shipped via cargo-dist / Homebrew (v1.19.2+) run this benchmark ~30% faster again; see [Performance Internals](./performance.md).

Macro expansion and dynamic `eval` are handled by `sema-eval`, which expands macros to a `Value` AST and then feeds them through the same compile-to-bytecode pipeline.

## Compilation Pipeline

```
Source text
  → Reader       (tokenize + parse → Value AST)
  → Macro expand  (sema-eval expands macros)
  → Lower         (Value AST → CoreExpr IR)
  → Optimize      (constant folding + simplification on CoreExpr)
  → Resolve        (CoreExpr → ResolvedExpr with slot/upvalue/global analysis)
  → Compile        (ResolvedExpr → bytecode Chunks)
  → VM execution   (dispatch loop)
```

### Phase 1: Lowering (Value → CoreExpr)

The lowering pass converts the `Value` AST into `CoreExpr`, a desugared intermediate representation. All ~40 special forms are lowered to ~35 CoreExpr variants. Several forms desugar into simpler ones:

| Source Form | Lowers To                                 |
| ----------- | ----------------------------------------- |
| `cond`      | Nested `If`                               |
| `when`      | `If` with nil else                        |
| `unless`    | `If` with swapped branches                |
| `case`      | `Let` + nested `If` with `Or` comparisons |
| `defun`     | `Define` + `Lambda`                       |
| Named `let` | `Letrec` + `Lambda`                       |

**Tail position analysis** happens during lowering. The `Call` node carries a `tail: bool` flag, set based on position:

- **Tail**: last expression in `lambda` body, `begin`, `let`/`let*`/`letrec` body, `if` branches, `cond` clauses, `and`/`or` last operand
- **Not tail**: `try` body (handler must be reachable), `do` loop body, non-last expressions

### Phase 2: Variable Resolution (CoreExpr → ResolvedExpr)

The resolver walks the CoreExpr tree and classifies every variable reference as one of:

| Resolution          | Opcode                         | Description                              |
| ------------------- | ------------------------------ | ---------------------------------------- |
| `Local { slot }`    | `LoadLocal` / `StoreLocal`     | Variable in the current function's frame |
| `Upvalue { index }` | `LoadUpvalue` / `StoreUpvalue` | Captured from an enclosing function      |
| `Global { spur }`   | `LoadGlobal` / `StoreGlobal`   | Module-level binding                     |

This is a key optimization: instead of hash-based environment chain lookup (O(scope depth) per access), variables are accessed by direct slot index (O(1)).

#### Upvalue Capture

Closures use the Lua/Steel upvalue model. When a lambda references a variable from an enclosing function:

1. The resolver marks the outer local as **captured**
2. An `UpvalueDesc` entry is added to the inner lambda: `ParentLocal(slot)` if capturing from the immediate parent, `ParentUpvalue(index)` if capturing through an intermediate function

```sema
(lambda (x)           ; x = Local slot 0
  (lambda ()          ; captures x: UpvalueDesc::ParentLocal(0)
    (lambda ()        ; captures through chain: UpvalueDesc::ParentUpvalue(0)
      x)))            ; resolves to Upvalue { index: 0 }
```

### Phase 3: Bytecode Compilation (ResolvedExpr → Chunk)

::: details The instruction format echoes the IBM 704 (1955)
The [704's](http://bitsavers.informatik.uni-stuttgart.de/pdf/ibm/704/24-6661-2_704_Manual_1955.pdf) Type A instruction packed four fields into a single 36-bit word: **prefix** (opcode), **decrement** (constant parameter), **tag** (register selector), and **address** (operand location). Sema's bytecode uses a strikingly similar structure — each instruction is an opcode byte followed by inline operands (constant indices, slot numbers, jump offsets). The 704 also had a `CAS` (Compare Accumulator with Storage) instruction that performed a 3-way branch in a single operation: skip 0, 1, or 2 instructions depending on less-than, equal, or greater-than. This is pattern matching as a hardware primitive — the ancestor of the conditional jump patterns Sema's compiler generates for `cond` and `match`.
:::

The compiler (`compiler.rs`) transforms `ResolvedExpr` into bytecode `Chunk`s. The `Compiler` struct wraps an `Emitter` (bytecode builder) and collects `Function` templates for inner lambdas.

**Compilation strategies:**

- **Constants**: `Nil`, `True`, `False` get dedicated opcodes. All other constants use `Const` + constant pool.
- **Variables**: `LoadLocal`/`StoreLocal` for locals, `LoadUpvalue`/`StoreUpvalue` for captures, `LoadGlobal`/`StoreGlobal`/`DefineGlobal` for globals.
- **Control flow**: `if` uses `JumpIfFalse` + `Jump` for short-circuit. `and`/`or` use `Dup` + conditional jumps to preserve the last truthy/falsy value.
- **Lambdas**: compiled to separate `Function` templates, referenced by `MakeClosure` instruction with upvalue descriptors.
- **`do` loops**: compile to backward `Jump` with `JumpIfTrue` for exit test.
- **`try`/`catch`**: adds entries to the chunk's exception table, no inline opcodes.
- **Named let**: desugared to `letrec` + `lambda` during lowering — the loop body becomes a closure compiled via `MakeClosure`.

**Runtime-delegated forms** — forms that can't be compiled to pure bytecode are compiled as calls to `__vm-*` global functions registered by `sema-eval`:

| Source Form                                               | Delegate                                                     |
| --------------------------------------------------------- | ------------------------------------------------------------ |
| `eval`                                                    | `__vm-eval`                                                  |
| `import`                                                  | `__vm-import`                                                |
| `load`                                                    | `__vm-load`                                                  |
| `defmacro`                                                | `__vm-defmacro-form` (passes entire form as quoted constant) |
| `define-record-type`                                      | `__vm-define-record-type`                                    |
| `delay`                                                   | `__vm-delay` (body wrapped in a zero-arg lambda thunk)       |
| `force`                                                   | `__vm-force`                                                 |
| `prompt`, `message`, `deftool`, `defagent`, `macroexpand` | Corresponding `__vm-*` delegates                             |

**Public API**: `compile(exprs, n_locals, known_natives)` returns `CompileResult { chunk, functions, native_table }`. When `known_natives` is provided, calls to those globals emit `CallNative` for direct dispatch.

### Compiler Optimizations

- **Intrinsic recognition**: Known builtins are compiled to inline opcodes instead of function calls, eliminating global lookup, `Rc` downcast, argument `Vec` allocation, and function pointer dispatch. Arithmetic/comparison: `+`, `-`, `*`, `/`, `<`, `>`, `<=`, `>=`, `=`, `not`. List/predicates: `car`/`first`, `cdr`/`rest`, `cons`, `null?`, `pair?`, `list?`, `number?`, `string?`, `symbol?`, `length`. Collections: `append` (2-arg), `get`, `contains?`, `nth`, `mod`/`modulo`.
- **Peephole: `(if (not X) ...)`**: The pattern `(if (not X) A B)` compiles to `JumpIfTrue` instead of `Not` + `JumpIfFalse`, eliminating one instruction.
- **Fused `CallGlobal`**: Non-tail calls to global functions use a fused `CallGlobal` instruction that combines `LoadGlobal` + `Call` into a single opcode with `(u32 spur, u16 argc, u16 cache_slot)` operands.
- **Specialized `LoadLocal`/`StoreLocal`**: Slots 0–3 have dedicated zero-operand opcodes (`LoadLocal0`..`LoadLocal3`, `StoreLocal0`..`StoreLocal3`), saving 2 bytes per instruction for the most frequently accessed locals.

### Phase 4: VM Execution

The VM (`vm.rs`) is a stack-based dispatch loop.

**Core structs:**

```rust
VM { stack: Vec<Value>, frames: Vec<CallFrame>, globals: Rc<Env>, functions: Rc<Vec<Rc<Function>>>, inline_cache: Vec<(u32, u64, Value)>, native_fns: Vec<Rc<NativeFn>> }
CallFrame { closure: Rc<Closure>, pc: usize, base: usize, open_upvalues: Option<Vec<Option<Rc<UpvalueCell>>>>, cache_base: usize }
```

**Key design points:**

- **Unsafe hot path**: The dispatch loop uses `unsafe` unchecked stack operations (`pop_unchecked`) and raw pointer bytecode reads via `read_u16!`/`read_i32!`/`read_u32!` macros for performance. Opcodes are dispatched by matching the raw byte against `u8` constants (the `op` module), avoiding decode overhead; `std::mem::transmute` is used only to reconstruct `Spur` handles from `u32` operands. Debug builds retain bounds checks via `debug_assert!`.
- **Closure interop**: VM closures are wrapped as `Value::NativeFn` values so code outside the VM can call them. Each NativeFn carries an `Rc<dyn Any>` payload containing `VmClosurePayload` (closure + function table), and the VM uses `raw_tag()` + `downcast_ref` to avoid `Rc` refcount bumps on the hot path. When called from outside the VM (e.g., stdlib higher-order-function callbacks), the NativeFn wrapper creates a fresh VM instance to execute the closure's bytecode; in-VM calls unwrap the payload and run in the same VM.
- **Upvalue cells**: Lua-style open upvalues. `UpvalueCell` holds a `RefCell<UpvalueState>` — `Open { frame_base, slot }` points into the VM stack while the defining frame is alive; `Closed(Value)` owns the value after the frame exits. Locals are read and written directly on the stack (no cell indirection); cells are closed when a frame returns, tail-calls, unwinds — and before any non-VM call (see Current Limitations).
- **Exception handling**: `Throw` opcode triggers handler search via the chunk's exception table. Stack is restored to saved depth, error value pushed, PC jumps to handler.

**Entry points**: `VM::execute()` takes a closure and `EvalContext`. `compile_program()` is the pipeline for normal compilation: `Value AST → lower → optimize → resolve → compile → CompiledProgram`. `compile_program_with_spans()` adds span/source-file support for debug (DAP breakpoints).

### VM Optimizations

- **Two-level dispatch loop**: An outer loop caches frame-local state (code pointer, constants pointer, base offset) into local variables. The inner loop dispatches opcodes without re-fetching frame data. Frame state is only reloaded when control flow changes frames (`Call`, `TailCall`, `Return`, exceptions).
- **NaN-boxed int fast paths**: `AddInt`/`SubInt`/`MulInt`/`LtInt`/`EqInt` operate directly on raw NaN-boxed bits — sign-extending the payload, performing the arithmetic, and re-boxing, without ever constructing a `Value`.
- **Per-instruction global inline cache**: Every `LoadGlobal`/`CallGlobal` instruction carries a `u16` cache-slot operand indexing into a per-VM `Vec<(spur, env_version, Value)>`. A hit (matching spur + env version) skips the `Env` lookup entirely; entries are invalidated by env version mismatch when a global is redefined.
- **Raw pointer bytecode reads**: `read_u16!`, `read_i32!`, and `read_u32!` macros read operands via raw pointer arithmetic on the code buffer, avoiding bounds checks in release builds.
- **Unsafe unchecked stack operations**: `pop_unchecked` skips length checks (the compiler guarantees stack correctness). `debug_assert!` guards catch violations in debug builds.
- **Cold path factoring**: The `handle_err!` macro factors exception handling out of the hot instruction sequence, keeping the fast path compact for better instruction-cache behavior.

## Opcode Set

The VM uses a stack-based instruction set with variable-length encoding. Each opcode is one byte, followed by operands (u16, u32, or i32).

### Constants & Stack

| Opcode  | Operands  | Description             |
| ------- | --------- | ----------------------- |
| `Const` | u16 index | Push `constants[index]` |
| `Nil`   | —         | Push nil                |
| `True`  | —         | Push #t                 |
| `False` | —         | Push #f                 |
| `Pop`   | —         | Discard top of stack    |
| `Dup`   | —         | Duplicate top of stack  |

### Variable Access

| Opcode         | Operands  | Description                  |
| -------------- | --------- | ---------------------------- |
| `LoadLocal`    | u16 slot  | Push `locals[slot]`          |
| `StoreLocal`   | u16 slot  | `locals[slot] = pop`         |
| `LoadUpvalue`  | u16 index | Push `upvalues[index].get()` |
| `StoreUpvalue` | u16 index | `upvalues[index].set(pop)`   |
| `LoadGlobal`   | u32 spur, u16 cache_slot | Push `globals[spur]` (inline-cached) |
| `StoreGlobal`  | u32 spur  | `globals[spur] = pop`        |
| `DefineGlobal` | u32 spur  | Define new global binding    |
| `LoadLocal0`..`LoadLocal3` | — | Push `locals[0..3]` (zero-operand fast path) |
| `StoreLocal0`..`StoreLocal3` | — | `locals[0..3] = pop` (zero-operand fast path) |

### Control Flow

| Opcode        | Operands   | Description                 |
| ------------- | ---------- | --------------------------- |
| `Jump`        | i32 offset | Unconditional relative jump |
| `JumpIfFalse` | i32 offset | Pop, jump if falsy          |
| `JumpIfTrue`  | i32 offset | Pop, jump if truthy         |

### Functions

| Opcode        | Operands                         | Description                           |
| ------------- | -------------------------------- | ------------------------------------- |
| `Call`        | u16 argc                         | Call function with argc args          |
| `TailCall`    | u16 argc                         | Tail call (reuse frame for TCO)       |
| `Return`      | —                                | Return top of stack                   |
| `MakeClosure` | u16 func_id, u16 n_upvalues, ... | Create closure from function template |
| `CallNative`  | u16 native_id, u16 argc          | Direct native function call (no env lookup) |
| `CallGlobal`  | u32 spur, u16 argc, u16 cache_slot | Fused global lookup + call (inline-cached) |

### Data Constructors

| Opcode        | Operands    | Description                    |
| ------------- | ----------- | ------------------------------ |
| `MakeList`    | u16 n       | Pop n values, push list        |
| `MakeVector`  | u16 n       | Pop n values, push vector      |
| `MakeMap`     | u16 n_pairs | Pop 2n values, push sorted map |
| `MakeHashMap` | u16 n_pairs | Pop 2n values, push hash map   |

### Arithmetic & Comparison

| Opcode                       | Description                           |
| ---------------------------- | ------------------------------------- |
| `Add`, `Sub`, `Mul`, `Div`   | Generic arithmetic (int/float/string) |
| `Negate`, `Not`              | Unary operators                       |
| `Eq`, `Lt`, `Gt`, `Le`, `Ge` | Generic comparison                    |
| `AddInt`, `SubInt`, `MulInt` | Specialized int fast paths            |
| `LtInt`, `EqInt`             | Specialized int comparison            |

### Inline Intrinsics

Zero-operand opcodes emitted by intrinsic recognition (bypass `CallGlobal` overhead):

| Opcode                                                       | Description                              |
| ------------------------------------------------------------ | ---------------------------------------- |
| `Car`, `Cdr`, `Cons`                                          | List operations                          |
| `IsNull`, `IsPair`, `IsList`, `IsNumber`, `IsString`, `IsSymbol` | Type predicates                       |
| `Length`, `Append`, `Get`, `ContainsQ`, `Nth`                 | Collection operations                    |
| `Mod`                                                         | Integer modulo fast path                 |

### Exception Handling

| Opcode  | Description                   |
| ------- | ----------------------------- |
| `Throw` | Pop value, raise as exception |

Exception handling uses an **exception table** on the Chunk rather than inline opcodes. Each entry specifies a PC range, handler address, and stack depth to restore.

## Crate Structure

The bytecode VM lives in the `sema-vm` crate, which sits between `sema-reader` and `sema-eval` in the dependency graph:

```
sema-core ← sema-reader ← sema-vm ← sema-eval
```

`sema-vm` depends on `sema-core` (for `Value`, `Spur`, `SemaError`) and `sema-reader` (for parsing in test helpers). It does **not** depend on `sema-eval` — the evaluator will depend on the VM, not the other way around.

### Source Files

| File           | Purpose                                                           |
| -------------- | ----------------------------------------------------------------- |
| `opcodes.rs`   | `Op` enum — 66 bytecode opcodes                                   |
| `chunk.rs`     | `Chunk` (bytecode + constants + spans), `Function`, `UpvalueDesc` |
| `emit.rs`      | `Emitter` — bytecode builder with jump backpatching               |
| `disasm.rs`    | Human-readable bytecode disassembler                              |
| `core_expr.rs` | `CoreExpr` and `ResolvedExpr` IR enums                            |
| `lower.rs`     | Value AST → CoreExpr lowering pass                                |
| `resolve.rs`   | Variable resolution (local/upvalue/global analysis)               |
| `compiler.rs`  | Bytecode compiler (ResolvedExpr → Chunk)                          |
| `vm.rs`        | VM dispatch loop, call frames, closures, exception handling       |
| `optimize.rs`  | Constant folding and simplification on CoreExpr IR                |
| `serialize.rs` | Bytecode serialization/deserialization for `.semac` file format   |
| `scheduler.rs` | Cooperative async task scheduler (VM-per-task, yield signals)     |
| `debug.rs`     | VM debug hooks for DAP (breakpoints, stepping, state queries)     |

## Async Execution (VM-Only)

Async/await and channels are implemented entirely in the VM.

The model is **VM-per-task with cooperative scheduling**:

- Each `async/spawn` creates a **new VM instance** that shares the parent's global `Env` (`Rc<Env>`) and function table (`Rc<Vec<Rc<Function>>>`). Tasks are cheap: no threads, no work stealing — everything stays single-threaded.
- A cooperative scheduler in `scheduler.rs` runs tasks **round-robin**. A task runs until it yields (e.g. `await` on a pending promise, channel operations, `async/sleep`).
- Yielding is signaled via a thread-local **yield signal** (`sema-core/src/async_signal.rs`), not an error variant. The VM checks the signal after every native call (`CallNative`, `CallGlobal`).
- On yield, the VM leaves a `nil` placeholder on the stack and advances the PC past the call. On resume, the scheduler swaps the placeholder for the wake value (`replace_stack_top`), so from bytecode's perspective the call simply returned.

This replaced an earlier replay-based design that re-executed entire task bodies on resume (corrupting side effects). Promises support cancellation (`PromiseState::Cancelled`), and task wake-ups preserve FIFO order.

Yield-aware native functions must work on both closure paths (in-VM and the fresh-VM fallback described under Current Limitations) — see `vm_async_test.rs` for the VM-only test suite.

## Current Limitations

- The compiler emits inline opcodes for common builtins (`+`, `-`, `*`, `/`, `<`, `>`, `<=`, `>=`, `=`, `not`, `car`/`first`, `cdr`/`rest`, `cons`, `null?`, `pair?`, `list?`, `number?`, `string?`, `symbol?`, `length`, `append`, `get`, `contains?`, `nth`, `mod`/`modulo`) via intrinsic recognition. Redefining one of these names in the same program suppresses the intrinsic for that program, but a redefinition from a separate compilation unit (e.g., an earlier REPL entry) does not — the intrinsic still fires.
- `CallNative` optimization requires passing `known_natives` at compile time (done automatically by `eval_str_compiled`); without it, all global calls use `CallGlobal`
- `set!` to a captured local is silently lost when the closure is invoked through a stdlib higher-order function (`map`, `filter`, `for-each`, …) — upvalue cells are closed to snapshots before every non-VM call, so the callback mutates a detached copy. Globals and in-VM calls are unaffected. Use `foldl` with explicit accumulator threading as a workaround.

## Design Decisions

### Why Not Delete CoreExpr After Resolution?

The pipeline uses two IR types: `CoreExpr` (variables as names) and `ResolvedExpr` (variables as slots). This provides type-level safety — the compiler can only receive resolved expressions, preventing accidental use of unresolved variable references.
