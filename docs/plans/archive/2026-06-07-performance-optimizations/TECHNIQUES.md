# OxCaml Performance Techniques — Synthesized Catalog

> Data-mined from the OxCaml (Jane Street OCaml fork) documentation on 2026-06-07.
> Raw source docs are mirrored under [`references/`](./references/). This file is the
> distilled, implementation-oriented catalog. For Sema-specific applicability, see
> [`sema-specific-adoptions.md`](./sema-specific-adoptions.md).

Each technique lists: **what** it is, the **mechanism** (how it works under the hood),
the **cost model** (why it is faster), the **soundness invariant** that makes it safe,
and the **source doc(s)**.

---

## Scope / allocation techniques

### 1. Stack / local allocation with regions

- **What:** Allocate short-lived values on a stack-like area instead of the GC heap.
- **Mechanism:** Values carry a *locality mode* — `local` (may not escape its region; stack-eligible) or `global` (may escape; heap). A function body, loop body, lazy expression, and module binding each open a *region*. Locals use a separate stack (layout-compatible with the minor heap), not the native call stack. Region entry records a stack pointer; region exit resets it, reclaiming everything at once. `stack_ expr` forces the next allocation onto the stack (or errors if it would escape). `exclave_ expr` ends the current region early and evaluates in the caller's region, letting a function return a value allocated in the caller's frame.
- **Cost model:** Allocation = pointer bump. Deallocation = one stack-pointer reset (not per-object). Cannot trigger GC, so it is safe in latency-critical zero-alloc paths. Reuses hot cache lines. Works for tuples, records, variants, closures, boxed numbers, strings, transient option/list nodes.
- **Soundness:** Type checker forbids a `local` value from escaping its region; `global` values may not reference `local` values; local closures capturing locals become local; mutable field/array/ref contents must be `global_` (otherwise an old mutable container could capture a younger stack value); tail calls close the caller region first.
- **Source:** `stack-allocation-intro.md`, `stack-allocation-reference.md`, `modes-intro.md`

### 2. Local parameters, local returns, and locality inference

- **What:** Function *types* express whether arguments/returns are local, letting APIs accept stack-allocated values without letting them escape.
- **Mechanism:** `x @ local` promises the function won't capture/store/return `x` beyond what its return locality allows. Arrows can be `'a -> 'b`, `'a @ local -> 'b`, `'a -> 'b @ local`, `'a @ local -> 'b @ local`. Locality is inferred within a compilation unit; exported functions need `.mli` annotations. HOFs should mark callback params `@ local` so stack-allocated closures can be passed. `[@local_opt]` gives limited mode-polymorphism on selected primitives.
- **Cost model:** Lets stack allocation survive abstraction boundaries; avoids forcing helpers to heap-allocate just because they return a value; enables stack-allocated closures in iterator-style APIs.
- **Soundness:** A function with a local arg is checked not to let it escape; local-returning functions are checked so the result lives in the caller/outer region; cross-unit inference is conservative.
- **Source:** `stack-allocation-intro.md`, `stack-allocation-reference.md`, `modes-intro.md`

---

## Representation / unboxing techniques

### 3. Kind & layout system

- **What:** A type-level classification of runtime representation and modal behavior.
- **Mechanism:** Each type has a *kind* = layout + modal bounds + with-bounds + non-modal bounds. Layouts describe representation/calling-convention: `value`, `immediate`, `immediate64`, `float64`, `float32`, `bits32`, `bits64`, `word`, `vec128`, unboxed products (`float64 & bits32`), `value_or_null`, `any`. Subkinding allows a more precise kind where a less precise one is expected. With-bounds record that a container's modal behavior depends on element types. Kinds also track *mode-crossing* (when a type can ignore an axis like locality/contention/uniqueness).
- **Cost model:** Lets the compiler know exact representation + calling convention → unboxed registers, flat arrays, mixed blocks, and representation-generic APIs without forcing boxing.
- **Soundness:** Code may only manipulate values with a concrete/representable layout; layout annotations are checked; subkinding/normalization keep representation constraints consistent across modules.
- **Source:** `kinds-intro.md`, `unboxed-types-intro.md`, `modes-intro.md`

### 4. Unboxed numeric scalars

- **What:** Primitive numbers represented directly, not as boxed heap objects.
- **Mechanism:** `float# : float64`, `float32# : float32`, `int32# : bits32`, `int64# : bits64`, `nativeint# : word`, plus SIMD vectors. Unboxed numbers live in locals, params, returns, and some record fields; args/returns use machine registers per layout. Libraries (`Float_u`, `Int32_u`, …) expose ops.
- **Cost model:** No heap allocation for boxed numbers, no pointer indirection, no GC scanning; operands/results stay in CPU registers; far less allocation in numeric loops.
- **Soundness:** Layout system stops unboxed values being used where `value` representation is required; generic boxed-uniform operations are restricted.
- **Source:** `unboxed-types-intro.md`, `small-numbers.md`, `kinds-intro.md`

### 5. Unboxed products (tuples & records)

- **What:** Product values represented as their fields directly, with no allocated block.
- **Mechanism:** Unboxed tuples `#(a * b * c)`, unboxed records `#{ field : ty }`. Across function boundaries the fields pass separately in registers/stack slots. Nested unboxed products flatten into the enclosing structure.
- **Cost model:** Eliminates tuple/record wrapper allocation, avoids field pointer-chasing, improves register allocation — ideal for returning several numeric values from a tight loop.
- **Soundness:** Layout system tracks the unboxed product layout; mutable fields in unboxed records are currently disallowed.
- **Source:** `unboxed-types-intro.md`, `kinds-intro.md`

### 6. Mixed blocks (boxed + unboxed fields in one block)

- **What:** A block format storing both GC-scannable values and unboxed fields together.
- **Mechanism:** The header records how many leading fields are scannable; the compiler reorders so all value fields precede non-value fields; the GC scans only the value prefix. Relative order within each group is preserved. All-float records get a flat-float representation.
- **Cost model:** Stores numeric fields inline (no per-field boxing), fewer allocations, better spatial locality, GC skips non-pointer data.
- **Soundness:** Compiler + runtime agree on layout; header bounds the GC scan; polymorphic equality/hash/marshal are unsupported on such structures; unsafe/C code must assert the layout version.
- **Source:** `unboxed-types-intro.md`

### 7. Arrays of unboxed elements

- **What:** Arrays whose elements have non-`value` layout, stored flat.
- **Mechanism:** Element layout may be `any`; elements packed by width (`float64`/`bits64` = 64b, `float32`/`bits32` = 32b, `vec128` = 128b). Array primitives are `[@layout_poly]` and specialize to the element layout at the call site. Non-float unboxed arrays use custom blocks for compare/hash.
- **Cost model:** No per-element boxing; less memory bandwidth & cache footprint; enables tight numeric loops and SIMD load/store; 32-bit values pack densely.
- **Soundness:** Layout-poly limited to compiler primitives; element layouts checked statically; maybe-null float-like elements restricted (breaks float-array separability).
- **Source:** `unboxed-types-intro.md`, `small-numbers.md`, `simd-intro.md`

### 8. `or_null`: non-allocating option

- **What:** An option-like type represented as either a value/immediate or a null word — no `Some` allocation.
- **Mechanism:** `type ('a : value) or_null : value_or_null` with constructors `Null` and `This v`. Ordinary OCaml values are never the word `0` (pointers non-null, immediates tagged), so `0` safely means null. GC is taught not to traverse null. `value_or_null` is a layout just above `value`.
- **Cost model:** Avoids allocating `Some x` and the extra indirection through the option block — big win for `find`/`head`/lookup APIs on hot paths.
- **Soundness:** The argument must be non-null layout `value`; `or_null or_null` is forbidden (would double-book the word `0`); arrays need non-null elements.
- **Source:** `unboxed-types-or-null.md`, `unboxed-types-intro.md`

### 9. Immutable arrays (`iarray`)

- **What:** Array-like containers that cannot be mutated.
- **Mechanism:** Syntax `[: ... :]`, type `t iarray`, no mutating ops. Because contents never change they may be stack-allocated, and the type is covariant (`sub iarray <: super iarray` when `sub <: super`).
- **Cost model:** Immutable sharing avoids defensive copies; safe covariance with no runtime check; no writes → free sharing across parallel tasks; contents stack-allocatable; avoids contention restrictions of mutable arrays.
- **Soundness:** No mutation API exists, which is exactly what makes covariance + stack allocation of contents safe.
- **Source:** `immutable-arrays.md`, `tutorial-parallelism-part1.md`

### 10. Small numbers (`float32`, `int8`, `int16`, `char#`)

- **What:** Smaller scalar numeric representations and their unboxed forms.
- **Mechanism:** `float32`/`float32#`, `int8`/`int8#`, `int16`/`int16#`, `char#`. Boxed `float32` is a custom block; `float32#` passes in FP registers; `float32# array` packs 32-bit floats; `char#` shares `int8#`'s layout.
- **Cost model:** Smaller footprint → better cache density and memory bandwidth; avoids needless widening to 64-bit; better SIMD packing.
- **Soundness:** Layout distinguishes boxed/unboxed forms; ops routed through the right-representation libraries; some pattern-match contexts restricted where support is incomplete.
- **Source:** `small-numbers.md`, `unboxed-types-intro.md`

### 11. SIMD vector types & intrinsics

- **What:** Explicit SIMD vector values + ops mapped to CPU vector instructions.
- **Mechanism:** Boxed/unboxed vector types (`int8x16(#)`, `int32x4(#)`, `float32x4(#)`, `float64x2(#)`, … plus 256-bit). Unboxed vectors pass in XMM/YMM registers and store flat. Intrinsics via `ocaml_simd_sse` / `ocaml_simd_avx`; const-required operands supplied by `ppx_simd`. Loads/stores from strings, bytes, bigstrings, unboxed arrays.
- **Cost model:** Multiple scalar ops per instruction; less loop overhead; high throughput for numeric arrays, byte processing, text scanning, hashing-like workloads.
- **Soundness:** Vector types are opaque; only recognized intrinsics operate on them; layout tracks boxed/unboxed; arch support explicit via SSE/AVX libraries.
- **Source:** `simd-intro.md`, `unboxed-types-intro.md`

---

## Aliasing / mutation techniques

### 12. Uniqueness & linearity for destructive update

- **What:** A mode system tracking whether a value has a single reference, enabling safe in-place mutation.
- **Mechanism:** *Uniqueness* axis: `unique` (one reference) vs `aliased`. *Linearity/affinity* axis: `once` (callable ≤ once) vs `many`. A `t @ unique` parameter consumes the sole reference; closures capturing unique values become `once`. Modes are deep (a unique container implies unique children unless a field uses `@@ aliased`). Branch-sensitive: a value can be unique in one branch, aliased in another. Future *overwriting* reuses memory in place (e.g. `List.map` on a unique input).
- **Cost model:** Avoids copy-on-write cloning when a container is provably unshared; in-place mutation behind pure-looking APIs; two-phase build (mutate while unique → publish as shared immutable); replaces allocate-new-result with mutate-existing.
- **Soundness:** Compiler prevents two unique consumes on the same path; unique capture forces `once` closures; modalities mark non-unique fields explicitly; destructive ops gated on a uniqueness proof.
- **Source:** `uniqueness-intro.md`, `uniqueness-reference.md`, `modes-intro.md`, `parallelism-capsules.md`

---

## Static analysis / specialization techniques

### 13. Static `zero_alloc` checker

- **What:** A compile-time proof that an annotated function performs no heap allocation on checked paths — *including its callees*.
- **Mechanism:** `[@zero_alloc]` annotation. The checker runs late (after optimization, inlining, specialization, static allocation, unboxing). It computes per-function allocation behavior and stores **interprocedural summaries in `.cmx`** so callers can use callee summaries across compilation units. Indirect calls and unannotated externals are conservatively "allocating" (unless `[@@noalloc]` or assumed). Abstract domain tracks whether paths allocate before normal return, only on exceptional paths, are assumed safe, get stuck, or are unknown. Relaxed mode (default) ignores allocation on exception-with-backtrace paths; `strict` rejects allocation on all paths. `assume`/`assume error`/`opt`/`assume_unless_opt` tune behavior.
- **Cost model:** Zero runtime overhead; prevents allocation regressions in hot paths; precise diagnostics naming the allocation site / allocating call / indirect call / probe / external.
- **Soundness:** Conservative — if the proof fails, the build fails for annotated functions; unknown/indirect calls assumed allocating; signature annotations enforce the contract across modules.
- **Source:** `zero-alloc-checker.md`

### 14. Templates / monomorphization over modes, kinds, modalities

- **What:** A PPX that generates specialized copies of definitions for chosen modes/kinds/modalities.
- **Mechanism:** `ppx_template` expands one declaration into multiple concrete versions (e.g. `id` for `global`, `id__local` for `local`). Ranges over modes (`global`/`local`), kinds (`value`, `value & value`), modalities (`portable`/`nonportable`). Instantiation attributes pick a version; floating attributes apply params to many following items.
- **Cost model:** Avoids runtime representation dispatch for compile-time-known cases; preserves precise local/global or boxed/unboxed behavior without hand-duplicating source; lets a library expose both alloc-friendly and compatibility-friendly variants.
- **Soundness:** All instances are generated eagerly and typechecked (no C++ SFINAE); invalid instances are compile errors; mode/kind constraints still enforced.
- **Source:** `templates-intro.md`

---

## Parallelism / observability techniques

### 15. Data-race-freedom modes for parallelism

- **What:** Mode tracking that permits parallel execution while statically preventing data races.
- **Mechanism:** *Contention* axis: `uncontended` (current domain may mutate) / `shared` (multi-domain read, no write) / `contended` (another domain may write, no unprotected access). *Portability* axis: `portable` (safe to cross domains) / `nonportable`. Portable functions capture only portable values and treat captures as contended. Modes are deep. Types with no mutable state / no functions can mode-cross. Parallel APIs (`fork_join2`) require portable tasks. *Capsules* tie mutable state to keys/passwords/mutexes/locks (unique key = exclusive, mutex = dynamic exclusivity, aliased key = shared read). Parallel array *slices* give disjoint mutable views.
- **Cost model:** Multicore speedups without coarse global locking; immutable/read-only data shared freely; mutable data split into disjoint slices or lock-protected; no runtime race-detection overhead (proven statically).
- **Soundness:** Contended mutable access requires protection; portable closures can't capture uncontended mutable state; unique keys/locality prevent unauthorized capsule access; slices guarantee disjointness; atomics protect shared mutable locations.
- **Source:** `modes-intro.md`, `parallelism-intro.md`, `tutorial-parallelism-part1.md`, `tutorial-parallelism-part2.md`, `parallelism-capsules.md`

### 16. Low-overhead tracing probes

- **What:** Static probe points enabled dynamically for tracing native programs.
- **Mechanism:** `[%probe "name" handler]`. Disabled by default; a disabled probe does not evaluate its handler and costs almost nothing. External tooling enables/disables probes during execution.
- **Cost model:** Production observability without always-on logging overhead; disabled probes avoid allocation and handler execution; ideal for rare paths, allocation witnesses, VM events, latency spikes.
- **Soundness:** Handlers are ordinary typed code; disabled state guarantees no evaluation; the zero-alloc checker treats possibly-enabled handlers as potentially allocating unless proven/assumed otherwise.
- **Source:** `tracing-probes.md`, `zero-alloc-checker.md`

---

## Cross-cutting implementation notes (from the oxcaml source tree)

These come from reading the compiler/runtime, not the user docs — useful when implementing analogues:

- **Local arena allocator** lives in the *domain state* as three hot fields (`local_sp`, `local_top`, `local_limit`) to avoid an indirection on the fast path. Allocation bumps `local_sp` downward; a `NOT_MARKABLE` header color tells the GC to skip local objects; arenas grow ×4; region begin/end is O(1) save/restore of `local_sp`.
- **Sorts** are `Base | Product | Var`; unified like type variables (union-find `Sort.equate`); `Sort.Const.some` pre-allocates `Some` boxes for base sorts to dodge allocation on a hot path.
- **Zero-alloc abstract domain** = `Bot | Safe | Top(witnesses) | Var | Transform | Join`. `transform` is sequential composition with `Safe` as identity and `Bot` absorbing; cross-unit `Var`s are substituted from `.cmx` summaries with a widening fallback for cycles.
- **Flambda2** is double-barrelled CPS in A-normal form (every `Apply` carries return + exception continuations); optimization is a downward type-propagation pass building equations, then an upward rebuild; *loopify* turns tail recursion into a self-loop continuation to avoid closure allocation.
- **`or_null`** = `Variant_with_null` representation, `Null` is integer `0`, `This x` is `x` verbatim, tracked by a `Nullability` jkind axis.
- **SIMD** lowering is a name→instruction selection table (`"caml_sse_float32x4_add" -> addps/vaddps`) that auto-picks AVX (3-operand) vs SSE based on enabled extensions.
