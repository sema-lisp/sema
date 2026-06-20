# Sema-Specific Adoptions of OxCaml Techniques

> Companion to [`TECHNIQUES.md`](./TECHNIQUES.md). For each OxCaml technique, this judges
> applicability to **Sema** (Lisp-in-Rust, NaN-boxed `Value(u64)`, `Rc`-backed heap types,
> tree-walker + stack bytecode VM, single-threaded) and gives a concrete recommendation.
>
> Classification legend: **HIGH-VALUE & TRACTABLE** / **MEDIUM** / **LOW** / **NOT-APPLICABLE**.

## TL;DR

Do **not** copy OxCaml's type/mode/kind system. The transfer that pays off is narrower:
use OxCaml's *ideas* internally to cut allocation in the VM and stdlib —
escape analysis, zero-allocation linting, homogeneous unboxed arrays,
unique-`Rc` mutation fast paths, and allocation-free numeric kernels.

Recommended first wave:

1. Bytecode-level **zero-allocation checker / lint** (measurement first).
2. Extend **unique-`Rc` copy-on-write fast paths** beyond maps → vectors, bytevectors, typed arrays.
3. Expand **unboxed homogeneous numeric arrays** + allocation-free array kernels.
4. Targeted **escape / allocation-sinking analysis** before any general arena.
5. Treat full modes/kinds/parallelism as future research, not near-term work.

## Sema baseline (ground all advice here)

- `Value` is a NaN-boxed `u64`. Immediates: `nil`, bools, 45-bit small ints (sign-extended),
  chars, interned symbols/keywords (`Spur`). Heap types are `Rc<T>` pointers in the box:
  string, list/vector (`Rc<Vec<Value>>`), map (`BTreeMap`), hashmap, lambda, macro, record,
  bytevector, `f64-array` (`TAG_F64_ARRAY`), `i64-array` (`TAG_I64_ARRAY`), promise, channel, …
- Default execution is `sema-vm` (stack bytecode VM): variable-length opcodes, threaded dispatch
  + superinstructions + inline call cache already landed; `CallFrame` Vec with `Rc<Closure>` clone
  per call; upvalue vecs.
- Single-threaded: `Rc` not `Arc`, `hashbrown` for `Env`, `BTreeMap` for user maps,
  `Env = Rc<RefCell<HashMap>>`.
- Known hot costs: `Value::clone`/`Drop` bump/drop an `Rc` on every push/load/pop; arithmetic
  historically decodes through `view()` → `ValueView`; call frames heap-allocate; transient
  `Rc<Vec>`/`Rc<String>` allocation pressure.
- **The existing [VM perf roadmap](../2026-02-17-vm-performance-roadmap.md)** targets
  dispatch/arithmetic/call-frames to close a ~7.8x gap vs Janet. The NEW value here is the
  *allocation-reduction*, *unboxing*, and *static-analysis* angle OxCaml emphasizes.
  (Note: that roadmap's `try_as_small_int` fast path is **not yet landed** as of writing.)

Verified existing infrastructure this plan builds on:
`Value::with_hashmap_mut_if_unique`, `with_map_mut_if_unique`, `into_hashmap_rc`,
`as_f64_array_rc` (all in `crates/sema-core/src/value.rs`); stdlib modules
`typed_array.rs`, `bytevector.rs`, `arithmetic.rs`, `math.rs`.

---

## Adoption catalog

### 1. Stack/local allocation + regions — HIGH-VALUE & TRACTABLE (scope narrowly)

The *idea* transfers; OxCaml's implementation does not (Sema has no static type system to prove
user-level lifetimes). But the VM can identify many non-escaping temporaries.

1. Start with **allocation sinking**, not new local `Value` tags: don't build a temporary
   list/vector when it's immediately consumed by a known intrinsic (e.g. apply args that stay a
   stack slice instead of an allocated list).
2. Add a CoreExpr/bytecode escape analysis for obvious cases: allocation consumed by
   `car`/`cdr`/`length`/`nth`/`get`/typed-array kernels; not stored to env/global/upvalue/
   map/vector/record; not passed to an unknown native/user call; not returned.
3. *Only then* consider a VM-frame arena for non-escaping collections (new internal
   `TAG_LOCAL_LIST`/`TAG_LOCAL_VECTOR` or a side table; `Drop` must be a no-op for local tags;
   a verifier must guarantee non-escape).

**Files:** `sema-vm/src/lower.rs`, `optimize.rs`, `compiler.rs`, `vm.rs`; later `sema-core/src/value.rs`.
**Effort:** alloc sinking for a few intrinsics M (proto 1–3h, harden 1–2d); general arena L/XL.
**Guardrails:** unknown native calls, `eval`, macro expansion, global stores, upvalue capture,
and returns all count as escaping; heap fallback for every uncertain case.

### 2. Locality / mode inference — MEDIUM/HIGH (internal-only)

No user `@local` syntax. Implement an internal `EscapeClass`/`AllocClass`
(`Immediate` / `NoAlloc` / `FrameLocal` / `HeapRequired` / `Unknown`) feeding the zero-alloc lint,
allocation sinking, and `MakeList`/`MakeVector` decisions. Diagnostics first; optimize once trusted.
**Files:** `sema-vm/src/core_expr.rs`, `optimize.rs`, `compiler.rs`, `vm.rs`. **Effort:** M/L (~1–2d).

### 3. Kind / layout system — MEDIUM (internal value-class lattice)

Not a full kind system — one dynamic `Value`. Add internal compiler facts
(`Nil`/`Bool`/`SmallInt`/`Float`/`Number`/`Symbol`/`String`/`List`/`Vector`/`F64Array`/`I64Array`/`Unknown`)
to emit specialized bytecode (int opcodes already exist; add float opcodes only where proven;
prefer typed-array kernels over scalar unboxed locals first).
**Files:** `sema-vm/src/optimize.rs`, `compiler.rs`, `opcodes.rs`, `vm.rs`. **Effort:** M.
**Guardrails:** never trust inferred classes across unknown calls or mutable global redefinition
without invalidation; respect stdlib-name shadowing like `optimize.rs` constant folding already does.

### 4. Unboxed numeric scalars — MEDIUM

Sema already has unboxed 45-bit small ints and direct `f64` via NaN-boxing. Missing piece is the
repeated dynamic decode/encode in hot loops. Land the roadmap fast paths (`try_as_small_int`,
`try_as_f64_exact`, `is_immediate`, `cheap_clone`), add `AddFloat`/`MulFloat`/`LtFloat` *only after
measuring*, and prefer `f64-array` kernels over boxed scalar loops.
**Files:** `sema-core/src/value.rs`, `sema-vm/src/{opcodes,vm,compiler}.rs`,
`sema-stdlib/src/{arithmetic,math}.rs`. **Effort:** S/M (accessors+opcodes); L (unboxed locals).
**Guardrails:** keep dynamic mixed int/float arithmetic; don't bloat the finite tag space.

### 5. Unboxed products (tuples/records) — LOW

Records are dynamic `Rc<Record>{type_tag, fields}`. Don't implement general unboxed products.
If multiple-return allocation is ever measured as a bottleneck, add a VM multiple-return
convention (count + stack slots) instead of boxing a tuple/list. **Effort:** L.

### 6. Mixed blocks — NOT-APPLICABLE

Mixed blocks exist to tell OCaml's *tracing GC* which fields are pointers. Sema uses `Rc<T>` +
NaN-boxed `Value`; no GC scans object fields. Use homogeneous typed arrays for unboxed numeric
storage instead. **No action.**

### 7. Arrays of unboxed elements — HIGH-VALUE & TRACTABLE

Already partially present (`TAG_F64_ARRAY`, `TAG_I64_ARRAY`, `as_f64_array_rc`, `typed_array.rs`).
Best transfer for Sema:

1. Expand/harden typed-array APIs: `f64-array/{map2,axpy,scale,add,sub,dot}`,
   `i64-array/{add,bit-and/or/xor}`.
2. Add smaller typed arrays only if workloads justify (`f32-array`, `i32-array`; `u8` already via
   bytevector).
3. Add fixed Rust-native kernels instead of per-element callback calls in hot code (the current
   `f64-array/map` boxes each element through a Sema call).
4. **Fix mutation paths:** `typed_array.rs` uses `as_f64_array_rc()` then `Rc::make_mut` — but the
   accessor increments the refcount, *obscuring uniqueness*. Add
   `with_f64_array_mut_if_unique` / `with_i64_array_mut_if_unique` analogous to the map fast paths.

**Files:** `sema-core/src/value.rs`, `sema-stdlib/src/typed_array.rs`, optionally `bytevector.rs`.
**Effort:** M (unique helpers + a few kernels 1–3h); L (full suite).
**Guardrails:** keep scalar fallbacks; validate indices (non-negative, no `as usize` wrap);
benchmark kernels separately from VM dispatch.

### 8. `or_null` non-allocating option — NOT-APPLICABLE

Sema already has non-allocating absence: `nil` and `#f` are immediate; Rust `Option<Value>` is
stack-only. There is no `Some`-allocation problem. Just prefer `nil`/`#f` for absence on hot paths
and avoid representing optionals as one-element lists. **No action.**

### 9. Immutable arrays — MEDIUM

Sema `list`/`vector` are already `Rc<Vec<Value>>` and behave mostly persistently — close to `iarray`.
Make the immutability invariant explicit; add unique fast paths for vector updates; consider naming
a mutable/transient builder type separately; use immutable vectors/lists as the default for sharing
and compiler constants.
**Files:** `sema-core/src/value.rs`, `sema-stdlib/src/list.rs`, `sema-vm/src/compiler.rs` (literals).
**Effort:** S/M (invariant+fast paths); L (transient builders).
**Guardrails:** never silently mutate a shared `Rc<Vec<Value>>`; check `strong_count == 1`/`get_mut`
before in-place update.

### 10. Small numbers (`float32`/`int8`/`int16`) — MEDIUM for arrays, LOW for scalar tags

Scalar small-int is already better than OCaml's boxed baseline (45-bit immediate). Don't add scalar
`int8`/`int16`/`float32` `Value` variants. Add smaller *typed arrays* if benchmarks need them
(`f32-array`, `i32-array`; `u8` via bytevector). Keep scalar math at int/f64.
**Effort:** M (one array) / L (full family). **Guardrails:** avoid tag explosion.

### 11. SIMD vectorization — MEDIUM

For typed-array/bytevector kernels, not general boxed values. Write clean Rust slice loops first
(LLVM auto-vectorizes `&[f64]`/`&[i64]`/`&[u8]`); add explicit `std::arch` SIMD behind feature flags
+ runtime CPU detection only if scalar kernels fall short. Targets: `f64-array/{dot,add,scale}`,
bytevector equality/search/checksum, text scanning.
**Files:** `sema-stdlib/src/{typed_array,bytevector,string}.rs`. **Effort:** M (auto-vec) / L (explicit).
**Guardrails:** portable scalar fallback; no nightly deps unless accepted; SIMD helps large arrays,
not tiny VM arithmetic.

### 12. Uniqueness / linearity for in-place mutation — HIGH-VALUE & TRACTABLE

Transfers very well: Rust gives the runtime predicate (`Rc::strong_count == 1` / `get_mut`), and Sema
already does this for maps (`with_hashmap_mut_if_unique`, `with_map_mut_if_unique`; used by `assoc`/`dissoc`).

1. Extend unique-mutation helpers to vectors (`Rc<Vec<Value>>`), bytevectors (`Rc<Vec<u8>>`),
   `f64-array` (`Rc<Vec<f64>>`), `i64-array` (`Rc<Vec<i64>>`), and records.
2. Check uniqueness **without first cloning the `Rc`** (avoid `as_*_rc()` before `make_mut` on hot paths).
3. Add consuming APIs (`into_vector_rc`, `into_f64_array_rc`, `into_i64_array_rc`) like `into_hashmap_rc`.
4. Keep persistent semantics: unique → mutate in place; shared → clone then update.

**Files:** `sema-core/src/value.rs`, `sema-stdlib/src/{map,list,typed_array,bytevector}.rs`.
**Effort:** S/M (<1d). **Guardrails:** strong count must be exactly 1; never construct a temporary
`Rc` that changes the count; test shared vs unique cases.

### 13. Static zero-allocation checker / lint over bytecode — HIGH-VALUE & TRACTABLE

One of the best ideas to adopt; no type system needed — opcodes and known stdlib calls classify
conservatively.

1. Allocation-effect analysis over compiled VM functions.
2. Classify opcodes: definitely-no-alloc (int arithmetic, jumps, bool/nil consts);
   definitely-alloc (`MakeList`/`MakeVector`/`MakeMap`/`MakeHashMap`, string build/concat, closure
   creation, calls without a no-alloc summary); may-alloc (generic `Call`, unknown native).
3. Per-function summaries: `NoAllocStrict` / `NoAllocNormalReturn` / `MayAlloc` / `Unknown`
   (mirrors OxCaml's `.cmx` interprocedural summaries).
4. Dev lint options first (`--vm-report-alloc`, `--vm-require-zero-alloc FUNCTION`), later optional
   `(declare (zero-alloc foo))`.
5. Print allocation witnesses: function, bytecode offset, opcode, source span.

**Files:** `sema-vm/src/{chunk,compiler,disasm,optimize}.rs`, CLI in `crates/sema/src/main.rs`.
**Effort:** M (1–2d). **Guardrails:** conservative is fine (false positives OK for v1); indirect/user
calls allocate unless a summary proves otherwise; separate strict vs normal-path; use it as a planning
tool *before* unsafe local-allocation optimizations.

### 14. Templates / monomorphization — MEDIUM (internal only)

Rust already monomorphizes host code; Sema source is dynamic. No user-visible templates. Add internal
specialization where cheap: direct native-call opcodes for known stdlib fns, specialized arithmetic
opcodes, typed-array kernels, possibly compile-time specialization of small monomorphic-call functions.
**Files:** `sema-vm/src/{optimize,compiler,opcodes,vm}.rs`, `sema-stdlib`. **Effort:** M/L.
**Guardrails:** avoid code-size blow-up; specialize only hot, simple, stable patterns; keep fallback.

### 15. Data-race-freedom modes for parallelism — NOT-APPLICABLE (now)

Sema is single-threaded with `Rc`/`RefCell`/non-`Send` values. If true parallelism is ever pursued:
decide `Arc` vs per-worker isolation first, lean on Rust `Send`/`Sync` as the first safety layer,
prefer actor/message-passing over shared mutable state. Don't prematurely switch to `Arc` (it would
slow the current single-threaded fast path). **No near-term work; XL if pursued.**

### 16. Tracing probes — MEDIUM/HIGH for profiling (not direct speed)

Very applicable as low-overhead VM/stdlib observability that makes allocation work *measurable*.
Add disabled-by-default probes around call entry/exit, `MakeList`/`MakeVector`/`MakeMap`, closure
creation, generic native calls, slow arithmetic fallback, and zero-alloc-checker failures. Prefer a
compile-time feature flag for v1; start with counters (alloc-op counts, call counts, slow-path counts,
sampled `Rc` clone/drop). **Files:** `sema-vm/src/vm.rs`, `sema-core/src/value.rs`, `sema-stdlib`,
`crates/sema/src/main.rs`. **Effort:** S/M (<1d). **Guardrails:** disabled probes must not allocate
or format args.

---

## What NOT to do yet

1. No user-visible mode system — too much complexity for a dynamic Lisp; use internal analyses.
2. No mixed blocks — no OCaml-style tracing GC scanning object fields.
3. No `or_null` — `nil` is already immediate and non-allocating.
4. No `Arc` switch for parallelism — would worsen refcount cost on the single-threaded fast path.
5. No scalar `int8`/`int16`/`float32` tags without benchmarks — use typed arrays for compact numerics.

## Revisit the advanced arena/local-value path only when

- zero-alloc reports show transient `MakeList`/`MakeVector`/string allocations dominate real workloads,
- unique-`Rc` fast paths and typed-array kernels are already implemented,
- benchmarks still show allocation pressure as a top bottleneck,
- the non-escaping patterns are simple enough to verify conservatively.

Until then the simple path wins: static allocation linting, unique-`Rc` mutation, typed-array kernels,
and selective bytecode specialization.
