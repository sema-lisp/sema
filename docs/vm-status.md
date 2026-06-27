# Bytecode VM Status

> Last updated: 2026-06-18 (tree-walker retired; VM is the sole evaluator)

## Current State

The bytecode VM (`sema-vm` crate) is the **sole** execution backend. The tree-walking interpreter was retired and its source deleted; the VM is now the only evaluator. The `--tw` and `--vm` CLI flags were removed in 1.18.0 (there is only one evaluator). Async/concurrency features are VM-only by design.

- **sema-vm unit tests:** 524 passing
- **Evaluator tests:** 840+ test cases across 11 test files (formerly dual-eval; now VM-only)
- **Total project tests:** 4,300+ passing, 0 failures (4 ignored — see *Known Limitations* below)

## Architecture

```
Source → Reader → Macro Expand → Lower (Expr<Spur>) → Optimize → Resolve (Expr<VarRef>) → Compile (bytecode) → VM Execute
                  ↑ VM-native                                                                            ↑
                  └── defmacro expanded here (expand_for_vm_in                                           |
                      in sema-eval) before compilation                            VM closures: same-VM CallFrame push
                                                                                   NativeFn fallback for stdlib HOF interop
```

**Same-VM execution:** VM closures carry an opaque `payload: Option<Rc<dyn Any>>` on `NativeFn`. The payload holds a `VmClosurePayload` (closure + function table). When `call_value` encounters a payload, it downcasts and pushes a `CallFrame` on the **same VM** — no fresh `VM::new()`. This eliminates native stack growth. True TCO is implemented via `tail_call_vm_closure`, which reuses the current frame, enabling 100K+ depth tail recursion.

**NativeFn fallback:** Closures passed to stdlib higher-order functions (map, filter, etc.) still go through the NativeFn wrapper interface, which creates a short-lived VM. This ensures interop with `sema-stdlib` which depends on `sema-core`, not `sema-vm`.

**Open upvalues (Lua-style):** Upvalue cells hold a stack index (`Open { frame_base, slot }`) instead of an eagerly-copied value. LoadLocal/StoreLocal are unconditional stack access — no dual-write to upvalue cells needed. Cells are closed (value copied from stack into cell) at frame exit (Return, TailCall, exception unwind) and before non-VM calls (to protect against the NativeFn fallback creating a fresh VM that can't resolve Open cells). After closing, entries in `open_upvalues` are cleared to prevent stale cell reuse when slots are reused across scopes.

**Dependency flow:** `sema-core ← sema-reader ← sema-vm ← sema-eval`. The VM crate cannot depend on sema-eval.

**NaN-boxed values:** All values are 8-byte NaN-boxed `u64`. Small ints (±17.5 trillion), symbols, keywords, chars, bools, and nil are unboxed immediates — no heap allocation. The VM benefits from smaller stack slots and better cache locality (8–12% speedup over the pre-NaN-boxing VM).

**Optimizer:** Constant folding pass runs between lowering and resolution — folds arithmetic, comparisons, boolean simplification, if/and/or with constant operands, and dead constant elimination.

## Opcodes

64 opcodes across 8 categories:

- **Stack/constants:** Const, Nil, True, False, Pop, Dup
- **Variables:** LoadLocal(0-3), StoreLocal(0-3), LoadUpvalue, StoreUpvalue, LoadGlobal, StoreGlobal, DefineGlobal
- **Control flow:** Jump, JumpIfFalse, JumpIfTrue, Call, TailCall, Return, Throw
- **Functions:** MakeClosure, CallNative, CallGlobal
- **Arithmetic (generic):** Add, Sub, Mul, Div, Negate, Not, Eq, Lt, Gt, Le, Ge
- **Arithmetic (int fast-path):** AddInt, SubInt, MulInt, LtInt, EqInt — operate directly on NaN-boxed bits, no Clone/Drop
- **Data constructors:** MakeList, MakeVector, MakeMap, MakeHashMap
- **Intrinsic stdlib ops:** Car, Cdr, Cons, IsNull, IsPair, IsList, IsNumber, IsString, IsSymbol, Length, Append, Get, ContainsQ

**Per-instruction inline cache:** `LoadGlobal` (7 bytes: op + u32 spur + u16 cache_slot) and `CallGlobal` (9 bytes: op + u32 spur + u16 argc + u16 cache_slot) each get a dedicated cache slot in a side array. On hit (matching spur + env version), global access is a single array index — no HashMap lookup. Cache entries store `(spur_bits, version, value)` to guard against cross-VM closure slot collisions. Bytecode format version 2.

## Known Limitations

One structural bug found during the May 2026 audit remains documented as planned multi-session work, not a blocker:

- **`.semac` bytecode loading is unsafe from untrusted sources** (audit finding C11). `validate_bytecode` does not abstract-interpret the instruction stream for stack balance; the VM's `pop_unchecked` (90+ call sites) assumes stack-balanced bytecode, so a hand-crafted `.semac` with a leading `Pop` triggers UB in release builds. Treat `.semac` files as trusted-source-only until the stack-depth verifier in `adr.md` #56 lands. See `limitations.md` #32.

The May 2026 audit's other structural finding, **VM `set!` through stdlib HOF callbacks loses the mutation** (audit finding C1), is now **FIXED** (2026-06-18). `(let ((c 0)) (map (fn (x) (set! c (+ c x))) (list 1 2 3)) c)` now returns `6` on the VM. The fix routes HOF callbacks back into the running VM via a thread-local `CURRENT_VM` plus nested-frame execution (`run_nested_closure`), rather than the open-upvalue-runtime approach once sketched in `adr.md` #55. Two minor follow-ups remain deferred: `(type (fn (x) x))` still reports `:native-fn` on the VM (TW-2), and ~~VM caught-error maps are still missing `:stack-trace` (TW-1)~~ **FIXED** (2026-06-27) — caught errors now include `:stack-trace`. See `docs/deferred.md`.

## Resolved Bugs

All 10 original VM bugs from the early bring-up are fixed:

| Bug                                                     | Problem                                                                                                            | Fix                                                                                             |
| ------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------ | ----------------------------------------------------------------------------------------------- |
| **1. Self-ref injection corrupting locals**             | `make_closure` wrote a NativeFn self-reference into a local slot for all named functions, not just named-let loops | Desugar named-let to `letrec` + `lambda` in `lower.rs`, eliminating self-ref injection entirely |
| **2. Missing arity checking**                           | NativeFn wrapper silently filled missing args with Nil and ignored extras                                          | Added strict arity validation in both the NativeFn fallback wrapper and `call_vm_closure`       |
| **3. compile_named_let missing func_id patch/upvalues** | Named-let didn't patch child func_ids or support upvalues                                                          | Same named-let desugaring as Bug 1 — `compile_named_let` and `NamedLet` variants fully removed  |
| **4. Fresh VM per closure → stack overflow**            | Each closure call created `VM::new()` + `vm.run()`, exhausting the Rust stack after ~200-500 calls                 | Same-VM execution via opaque payload in NativeFn (see Architecture above)                       |
| **5. Recursive inner define**                           | `(define (f) (define (g) (g)) (g))` failed — resolver resolved lambda body before defining local slot              | Fixed in resolver: allocate local slot before resolving RHS                                     |
| **6. delay/force not capturing lexical vars**           | `(define (f a) (delay a))` failed — delay passed raw AST, tree-walker couldn't see VM locals                       | Fixed: delay now lowers to zero-arg lambda thunk that captures lexical environment              |
| **7. `__vm-import` selective import**                   | Selective names list pushed as single element instead of spreading individual symbols                              | Fixed: spread symbols individually in the reconstructed import form                             |
| **8. `and` optimizer returning `#f` for falsy values**  | `fold_and` in optimizer replaced constant falsy values (nil) with `#f` instead of preserving the original value    | Fixed: return the actual falsy constant, matching tree-walker semantics                         |
| **9. Inner define forward references**                  | `(define (a) (b)) (define (b) 42)` inside function bodies failed — resolver didn't pre-scan for define names       | Fixed: `resolve_body()` pre-registers all inner define names before resolving expressions       |
| **10. Stale upvalue cell reuse on slot reuse**           | Named-lets reusing the same slot after a native call got a Closed cell containing the old closure's value           | Fixed: `close_open_upvalues` clears entries after closing, preventing stale cell reuse          |

## Performance

> **Note (Jun 2026):** the numbers below are **pre-PGO** and from older runs. v1.19.2 shipped fat LTO (3–9%) and PGO (~25–29% on 1BRC, −11% to −40% on compute) in the release binaries — see [Performance Roadmap](performance-roadmap.md) §10/§13. Re-measure before relying on these.

- **1BRC (10M rows, VM):** ~15.9s — dominated by Rc/drop (~35%), VM dispatch (16.5%), HashMap::clone (5.8%)
- **Compute benchmarks (VM):** TAK 8.04s, deriv 1.84s (post-NaN-boxing)
- **VM vs (retired) tree-walker:** the VM was ~1.7–2× faster on compute-heavy workloads, which motivated retiring the tree-walker
- **Janet comparison:** ~1.7× behind Janet on 1BRC (both are embeddable bytecode VMs, no JIT)
- See [Performance Roadmap](performance-roadmap.md) for detailed analysis and optimization plan

## Deferred Work

- **Tracing GC:** Replace Rc-based reference counting — estimated ~1.3× speedup
- **Direct threading:** Computed goto dispatch — estimated 15–30% on tight loops
- **Macro expansion caching:** Cache expanded macros to avoid redundant VM-native macro expansion
- **Register-based VM:** Would reduce push/pop traffic but requires full rewrite
