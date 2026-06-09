# Architecture Decisions and Future Plans

> **ARCHIVED 2026-06-09.** Legacy pre-`docs/adr.md` document — kept for history only; do not trust as current.
> Unique content was rehomed to `docs/adr.md` #60-63 (NaN-boxing, mini-eval removal, sandbox, package system).
> Known-wrong claims as of archival: the **Named-Let** section is false (named-let IS desugared to letrec+lambda; `compile_named_let`/`NamedLet` were removed — see adr.md #52); the **eval reify** section describes a design that was never built (VM `eval` sees globals only, no locals view — see `docs/limitations.md`); the LSP and "Future: lock file" sections long since shipped.

This document records key architectural decisions in Sema and planned future work.

## Naming Conventions

The stdlib uses four naming conventions, reflecting Sema's evolution from Scheme roots toward its own identity:

### `module/function` (slash-namespaced) — preferred

The modern, preferred style for new functions. Groups functions by module for discoverability.

```scheme
(string/trim "  hello  ")
(list/map inc '(1 2 3))
(map/keys my-map)
(file/read "data.txt")
```

All new stdlib additions should use this convention.

### `function-name` (dash-separated) — legacy Scheme/R7RS compatibility

Kept as aliases for compatibility with Scheme code and familiarity for users coming from R7RS.

```scheme
(string-append "a" "b")
(string-length "hello")
(string-ref "hello" 0)
```

These are not deprecated but the slash-namespaced variants should be preferred in new code and documentation.

### `type->type` (arrow) — type conversions

Standard Lisp/Scheme convention for conversion functions.

```scheme
(string->number "42")
(char->integer #\A)
(number->string 3.14)
```

### `predicate?` (question mark) — predicates

Standard Scheme convention for boolean-returning functions.

```scheme
(string? x)
(null? lst)
(even? 4)
(string/contains? "hello" "ell")
```

### No-namespace builtins

A few ubiquitous primitives are kept without a namespace prefix for Scheme familiarity and brevity.

```scheme
(substring "hello" 1 3)
(format "~a is ~a" "Sema" "great")
(str 42)
```

## Rc Cycles and Memory Management

- Sema uses `Rc` (not `Arc`) throughout, since the interpreter is single-threaded. This avoids atomic reference counting overhead.
- Recursive `define` creates Rc cycles by design: a lambda captures its environment, which contains a binding to the lambda itself. This is necessary for recursive closures to work.
- No cycle collector exists. Recursive closures leak memory — the Rc cycle is never broken.
- This is acceptable for the current use case: scripting and short-lived interpreter sessions where the OS reclaims memory on exit.
- **Future work:**
  - Investigate weak self-references for named lambdas (the lambda's own binding in its captured env could be a `Weak<RefCell<...>>`)
  - Alternatively, a simple mark-and-sweep pass over the global environment on interpreter drop could break known cycles

## Sandbox / Permission System

- The `--sandbox` CLI flag restricts dangerous native functions at runtime via a capability bitset (`Caps` type in `sema-core`).
- Eight capability groups: `fs-read`, `fs-write`, `shell`, `network`, `env-read`, `env-write`, `process`, `llm`.
- Sandboxed functions remain registered (discoverable, tab-completable) but return a `PermissionDenied` error when invoked.
- Implementation: `register_fn_gated()` (in `sema-stdlib` and `sema-llm`) wraps closures with a `Sandbox::check()` guard at registration time. When the sandbox is unrestricted (default), zero overhead — functions are registered directly.
- The WASM playground (`sema.run`) uses compile-time feature flags (`#[cfg(not(target_arch = "wasm32"))]`) to shim out dangerous APIs entirely — this is complementary to the runtime sandbox.
- Embedders can use `InterpreterBuilder::with_sandbox(Sandbox::deny(...))` for fine-grained control.
- Presets: `--sandbox=strict` (deny shell, fs-write, network, env-write, process, llm) and `--sandbox=all` (deny everything).
- **Not a process sandbox** — this is an in-language permission check. It prevents stdlib natives from doing I/O but does not provide OS-level isolation.

## Evaluator Callback Architecture (Mini-Eval Removal)

- The 620-line mini-evaluator (`sema_eval_value` + `call_function`) that previously lived in `sema-stdlib/src/list.rs` has been **deleted**.
- It existed because `sema-stdlib` cannot depend on `sema-eval` (circular dependency). It was a hand-optimized fast-path evaluator that provided ~4× speedup on hot loops by skipping the trampoline, call stack, and span tracking.
- It was replaced with a **callback architecture**: `sema-core` provides thread-local `eval_callback` and `call_callback` functions, registered by `sema-eval` during interpreter initialization. All stdlib functions now call through the real evaluator.
- **Trade-off:** 1BRC benchmark regressed from ~960ms to ~3050ms (3.2×) on 1M rows. This is acceptable for correctness — the mini-eval diverged from the real evaluator (no `try/catch`, `do`, macros, modules) and was a maintenance blocker for the bytecode VM transition.
- **Fast-path optimizations** recovered ~14% of the regression (3050ms → ~2630ms on 1M rows):
  1. Thread-local shared `EvalContext` (`with_stdlib_ctx`) eliminates per-call allocation of 6 RefCells in `call_function` and IO streaming functions.
  2. Inline `NativeFn` dispatch in `call_function` skips the `call_callback` thread-local indirection for native function calls.
  3. Self-evaluating fast path in `eval_value` short-circuits Int, Float, String, Bool, Nil, Symbol, Keyword, and other self-evaluating forms before depth/step tracking.
  4. Deferred cloning in `eval_value_inner` avoids `Value::clone()` and `Env::clone()` on the first trampoline iteration (the common non-TCO case).
- **Remaining gap** (~2630ms vs ~960ms original mini-eval) is dominated by the tree-walker's fundamental per-expression overhead: Env chain lookups, Rc refcounting, call-stack management, and trampoline dispatch. This cannot be closed without a bytecode VM.

## Bytecode VM Design Decisions

Key architectural decisions for the bytecode VM:

### 1. `eval` Semantics: Reify (read-only)

**Decision:** `(eval expr)` sees lexical locals from the calling VM frame, but as a **read-only view**. `set!` inside `eval` can only target globals — attempting to mutate a reified local is an error.

**What "reify" means:** When compiled code calls `eval`, the VM must bridge two worlds. The VM stores locals in numbered stack slots (e.g., `x` is slot 0, `y` is slot 1). The tree-walker evaluator that runs `eval`'d expressions expects an `Env` hash map with named bindings. "Reify" means the VM walks its current frame and upvalue cells, builds a temporary `Env` with `{x → slot[0], y → slot[1], ...}`, and passes it to the tree-walker.

**Why read-only:** If `eval` could mutate locals via `set!`, the temporary Env would need to be a _live view_ into VM slots — requiring `Env` to become an enum over hash-map storage vs. frame-pointer storage. This is architecturally invasive and adds runtime branching to every `Env::get`/`set` call. The read-only model avoids this entirely: the reified Env is a plain hash map snapshot. `eval` can read locals and mutate globals, which covers all practical use cases.

**Compiler requirement:** The compiler must preserve a name→slot mapping table (`Vec<(Spur, u16)>`) in each function's debug metadata. Without this, reify can't know what names to assign to slot values.

**What reify includes:** Locals in the current frame + captured upvalues from enclosing scopes + module globals. The full lexical environment is visible.

**Performance:** O(n) per `eval` call where n = number of locals + upvalues. Acceptable since `eval` is inherently dynamic and rare in hot paths.

### 2. Macro Phase: Keep Runtime Semantics

**Decision:** Macros remain runtime-expanded, exactly as today. When the VM encounters a Macro value, it delegates to the tree-walker for expansion, then compiles and executes the result.

**Implication:** The tree-walker (`sema-eval`) is NOT deleted — it becomes the macro expansion engine. Both runtimes must coexist and interoperate. The compilation pipeline is: source → reader → macro expand (tree-walker) → lower to CoreExpr → resolve variables → compile to bytecode → VM execution.

**Performance risk:** Macros used in hot loops trigger expand→compile per iteration. Mitigation: per-callsite expansion cache keyed by macro identity + structural arg hash. Not required for v1 but should be designed for.

### 3. GC: Keep Rc for v1

**Decision:** Accept `Rc` cycle limitations for the initial bytecode VM. Document known cycle sources (recursive `define`, self-referencing closures via upvalue cells). Plan tracing mark-sweep GC for v2, using the VM stack as roots.

**Known cycle sources:**

- Named lambdas bind themselves in their captured env: `Lambda → env → bindings → Lambda`
- In the VM, self-reference becomes: `Closure → upvalues → UpvalueCell → Value::Closure → ...`
- These are bounded leaks (closure + its captured environment), not growing leaks.

### 4. VM Closure Execution: NativeFn with Payload (implemented)

**Decision:** VM closures are wrapped as NativeFn values but carry an opaque `payload: Option<Rc<dyn Any>>` containing a `VmClosurePayload` (the compiled closure + function table). Inside the VM, `call_value` detects the payload, downcasts to `VmClosurePayload`, and pushes a `CallFrame` on the **same VM** — no Rust recursion and no native stack growth.

**Why not a separate VmClosure type:** The originally planned approach was to add a dedicated VmClosure value type. Instead, the NativeFn payload field was used, which avoids adding a new type tag and keeps the Value type compact.

**NativeFn fallback:** The NativeFn wrapper function is kept as a fallback for closures that cross the VM/tree-walker boundary (e.g., closures passed to stdlib HOFs like `map`, `filter`, `fold`). In that case the NativeFn callback fires, which spins up a fresh VM.

**TCO:** True tail-call optimization is implemented via `tail_call_vm_closure`: the current frame's stack space is reused for the tail call, enabling 100K+ depth tail recursion without stack growth.

### 5. Named-Let: Dedicated Compiler Path (implemented)

**Decision:** Named-let (`(let loop ((n init)) body)`) is compiled via a dedicated `compile_named_let` in the VM compiler. The `NamedLet` CoreExpr variant flows through lowering, resolution, and into the compiler where it is handled directly — binding the loop name to a synthetic lambda, then calling it with initial values.

**History:** An earlier plan to desugar named-let into letrec in `lower.rs` was considered but not implemented. The dedicated compiler path was kept because it correctly handles func_id patching and upvalue support.

## NaN-Boxing Value Representation

**Decision:** Replace the 24-byte `enum Value` with an 8-byte NaN-boxed `struct Value(u64)`. All values are encoded in 8 bytes using IEEE 754 quiet NaN payload space.

**Encoding scheme:**

- **Floats:** Stored directly as `f64` bits. Canonical quiet NaN (`0x7FF8...`) used for NaN float values to avoid collision with boxed values.
- **Boxed values:** sign=1, exponent=all 1s, quiet bit=1. Bits 50-45 = TAG (6 bits, up to 64 types), bits 44-0 = PAYLOAD (45 bits).
- **Small integers:** 45-bit two's complement in the payload, range ±17.5 trillion. No heap allocation.
- **Symbols/keywords:** `Spur` (interned string key, 32 bits) stored directly in the payload. No heap allocation.
- **Chars:** Unicode codepoint (32 bits) stored directly in the payload.
- **Booleans/nil:** Tag-only, zero payload. Constants `Value::NIL`, `Value::TRUE`, `Value::FALSE`.
- **Heap types** (String, List, Vector, Map, Lambda, etc.): Rc pointer stored in the 45-bit payload (pointer >> 3, using 8-byte alignment guarantee). 23 heap-allocated types supported.

**API change:** Value is no longer an enum — pattern matching uses `val.view()` → `ValueView` enum, or direct accessors (`as_int()`, `as_str()`, `as_list()`, `is_nil()`, etc.). Constructors are lowercase functions: `Value::int(n)`, `Value::string(s)`, `Value::list(v)`, etc.

**Benchmark results (Apple M-series, release mode):**

| Benchmark | Old (TW) | NaN-box (TW) | Δ TW | Old (VM) | NaN-box (VM) | Δ VM     |
| --------- | -------- | ------------ | ---- | -------- | ------------ | -------- |
| tak       | 19.3s    | 21.1s        | −9%  | 9.09s    | 8.04s        | **+12%** |
| nqueens   | 18.7s    | 20.8s        | −11% | <1ms     | <1ms         | —        |
| deriv     | 2.97s    | 3.44s        | −16% | 1.99s    | 1.84s        | **+8%**  |

**Analysis:**

- **VM mode sees 8-12% speedup** — the bytecode VM benefits from smaller Value size in its stack, constant pool, and register operations. Better cache locality.
- **Tree-walker sees 9-16% regression** — the tree-walker's hot path now has additional cost from `view()` (refcount bump) and accessor overhead that the direct enum `match` didn't have. The tree-walker matches on Value types hundreds of millions of times in these benchmarks.
- **Memory reduced ~5-10%** across all benchmarks (RSS), reflecting the 3× smaller Value type.
- **Binary size unchanged** (~9.2MB).

**Why keep it despite tree-walker regression:** The VM is the future execution path. Tree-walker regression is acceptable because (a) the VM is already 2× faster than the TW, (b) the TW will eventually only be used for macro expansion and REPL, (c) the VM gains compound with data-heavy workloads (lists, vectors, maps have 3× less per-element overhead).

**Migration scope:** ~1,800 compile errors across 34 files in 8 crates. Purely mechanical: constructor renames, pattern match → accessor/view() conversion.

**Safety fix during migration:** `as_bytevector()` and `as_record()` had dangling pointer UB — they used `borrow_rc()` which created a stack-local `ManuallyDrop<Rc<T>>` and returned a reference into it. Fixed to use `borrow_ref()` directly, returning `&[u8]` and `&Record`.

## Package System

- `sema pkg` CLI for managing dependencies — `init`, `add`, `install`, `update`, `remove`, `list`.
- Two package sources: **git repos** (fully working) and a **package registry** (CLI implemented, central registry not yet deployed).
- Git packages install via `sema pkg add github.com/user/repo@ref` to `~/.sema/packages/`.
- Registry commands (`search`, `info`, `publish`, `yank`, `login`) require a running registry instance — either self-hosted or the central registry once it launches.
- Self-hostable registry server lives in `pkg/` — single Rust binary with SQLite, REST API, and web UI.
- Package manifest: `sema.toml` with `[package]` metadata and `[deps]` section (short names for registry, URL paths for git).
- Default entrypoint: `package.sema`. Custom entrypoint via `entrypoint` field in `sema.toml`.
- **Future:** lock file (`sema.lock`) for reproducible builds, recording exact commit SHAs and registry checksums.

## LSP Server

- Editor support currently consists of syntax highlighting only, with grammars for VS Code, Zed, Vim, Emacs, and Helix (see `editors/` directory). A standalone `tree-sitter-sema` grammar is also published.
- **Future: `sema-lsp` crate** using the `tower-lsp` crate
- Features to implement, in priority order:
  1. **Diagnostics** — surface parse errors from `sema-reader` in real-time
  2. **Go-to-definition** — resolve symbols through the module system
  3. **Completion** — stdlib function names, imported symbols, local bindings
  4. **Hover docs** — show function signatures and docstrings
- Can reuse `sema-reader` for parsing and `sema-eval` for limited type inference.
- The existing editor highlighting grammars in `editors/` provide a foundation to build on.
