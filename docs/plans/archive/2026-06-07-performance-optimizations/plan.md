# Performance Optimizations — Implementation Spike Plan

> 📦 **ARCHIVED (2026-06-20) — never started; superseded by a simpler perf pass.**
> This spike (allocation reduction, unique-Rc fast paths, typed-array kernels,
> escape analysis, frame-local arenas) was never begun. The perf work that
> actually shipped in 1.19.x was a separate, simpler initiative — PGO, fat LTO,
> and inline string opcodes (see CHANGELOG 1.19.2) — none of which this plan
> proposed. The 25-file `references/` OxCaml documentation mirror was **deleted**
> on archival (reconstructable from oxcaml.org); only these 3 original Sema docs
> are kept. Revisit if allocation-bound workloads become a real bottleneck.

> **Status:** Research / filed for a future spike. Not yet started.
> **Date filed:** 2026-06-07
> **Inputs:** [`TECHNIQUES.md`](./TECHNIQUES.md) (OxCaml techniques),
> [`sema-specific-adoptions.md`](./sema-specific-adoptions.md) (per-technique fit for Sema).
> **Relationship to prior work:** complements the existing
> [VM perf roadmap](../2026-02-17-vm-performance-roadmap.md) (dispatch/arithmetic/call-frames).
> This plan adds the **allocation-reduction + unboxing + static-analysis** angle.

## Goal

Reduce allocation pressure and per-op overhead in the Sema bytecode VM and stdlib by adapting
OxCaml's allocation-oriented techniques, **measuring first** so optimizations are evidence-driven.

## Guiding principles

- Measure before optimizing; land the zero-alloc lint + counters before any unsafe arena work.
- Conservative analyses (false positives acceptable) beat unsound cleverness.
- Preserve dynamic-Lisp semantics and persistent value behavior; mutate in place only when proven unique.
- Keep a heap/scalar fallback for every fast path.
- Stay single-threaded; do not introduce `Arc`.

## Phased plan

### Phase A — Measurement & guardrails (HIGH, ~M)

Build the instrumentation that justifies everything else.

- [ ] Bytecode **allocation classification** of opcodes: no-alloc / definitely-alloc / may-alloc
      (technique #13). Per-function summaries: `NoAllocStrict`/`NoAllocNormalReturn`/`MayAlloc`/`Unknown`.
- [ ] CLI/dev lint: `--vm-report-alloc`, `--vm-require-zero-alloc FUNCTION`; print witnesses
      (function, bytecode offset, opcode, source span).
- [ ] VM **counters/probes** (technique #16): alloc-op counts, call counts, slow-path arithmetic
      counts, sampled `Rc` clone/drop. Disabled by default (feature flag), zero-cost when off.
- [ ] Capture baseline allocation reports for the benchmark suite + stdlib hot paths.
- **Files:** `sema-vm/src/{chunk,compiler,disasm,optimize,vm}.rs`, `crates/sema/src/main.rs`,
      `sema-core/src/value.rs`.

### Phase B — Unique-`Rc` mutation fast paths (HIGH, ~S/M)

Extend the proven map pattern to all `Rc<Vec<…>>`-backed types (technique #12).

- [ ] Add `with_vector_mut_if_unique`, `with_bytevector_mut_if_unique`,
      `with_f64_array_mut_if_unique`, `with_i64_array_mut_if_unique` (check `strong_count == 1`
      *without* first constructing a temporary `Rc`).
- [ ] Add consuming `into_vector_rc` / `into_f64_array_rc` / `into_i64_array_rc` (like `into_hashmap_rc`).
- [ ] Update stdlib update/mutation sites to use the helpers before cloning; fix `typed_array.rs`'s
      `as_f64_array_rc()`-then-`make_mut` (the accessor inflates the refcount and hides uniqueness).
- [ ] Tests for shared vs unique cases.
- **Files:** `sema-core/src/value.rs`, `sema-stdlib/src/{map,list,typed_array,bytevector}.rs`.

### Phase C — Typed-array kernels (HIGH for numerics, ~M)

Allocation-free, callback-free numeric kernels (techniques #7, #11).

- [ ] Rust-native kernels: `f64-array/{map2,axpy,scale,add,sub,dot}`, `i64-array/{add,bit-ops}`.
- [ ] Keep flexible callback APIs but route hot paths through fixed kernels.
- [ ] Benchmark scalar Rust loops first (LLVM auto-vec); add `std::arch` SIMD behind a feature flag
      only if needed, with portable fallback + runtime CPU detection.
- [ ] Index validation (non-negative, no `as usize` wrap).
- **Files:** `sema-stdlib/src/{typed_array,bytevector}.rs`, `sema-core/src/value.rs`.

### Phase D — Internal value-class & escape analysis (MEDIUM/HIGH, ~L)

The compiler-internal analogue of kinds + locality (techniques #2, #3, #1 step 1–2).

- [ ] Conservative **value-class** propagation over CoreExpr/bytecode → emit specialized opcodes
      where safe (respecting stdlib-name shadowing + global-redefinition invalidation).
- [ ] **Allocation sinking** for obvious non-escaping temporaries (e.g. apply-args list that stays a
      stack slice) — escape barriers: unknown calls, `eval`, macro expansion, global/upvalue stores, returns.
- [ ] Validate impact against Phase A reports.
- **Files:** `sema-vm/src/{core_expr,optimize,compiler,vm}.rs`.

### Phase E — Frame-local / arena values (ADVANCED, ~L/XL)

Only if Phase A–D evidence shows transient collection/string allocations still dominate.

- [ ] VM frame arena; internal `TAG_LOCAL_LIST`/`TAG_LOCAL_VECTOR` (or side table) with no-op `Drop`.
- [ ] Verifier-enforced non-escape; heap fallback on any uncertainty.
- **Files:** `sema-core/src/value.rs`, `sema-vm/src/{vm,compiler}.rs`.

## Explicitly out of scope (see adoptions doc §"What NOT to do yet")

User-visible mode/kind system; mixed blocks; `or_null`; `Arc`/parallelism modes; scalar
`int8`/`int16`/`float32` `Value` tags; unboxed user records/tuples.

## Success criteria

- Allocation reports show a measurable drop in `MakeList`/`MakeVector`/string allocations on the
  benchmark suite.
- Unique-`Rc` fast paths eliminate clone-then-mutate on common stdlib update calls (verified by counters).
- Typed-array kernels show a clear speedup over the callback-per-element path on numeric benchmarks.
- No regressions in dual-eval tests; semantics preserved (persistent unless provably unique).

## Validation (per AGENTS.md)

- `cargo build`, `make lint` (fmt-check + clippy -D warnings), `cargo test`.
- New stdlib/value behavior → `dual_eval_tests!` in `crates/sema/tests/dual_eval_test.rs`
  (pure, no I/O); typed-array/uniqueness edge cases get explicit shared-vs-unique tests.
- Benchmark with `hyperfine` against the existing tak/numeric suite; record before/after.
