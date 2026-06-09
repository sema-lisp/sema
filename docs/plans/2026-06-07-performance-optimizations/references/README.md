# OxCaml Reference Docs (mirrored)

Offline mirror of the OxCaml (Jane Street OCaml fork) documentation pages relevant to
performance optimizations, captured 2026-06-07 from <https://oxcaml.org/documentation/>.
Each file keeps its original source URL in a header comment. Code blocks are OCaml.

These are the raw inputs; the distilled analysis lives in
[`../TECHNIQUES.md`](../TECHNIQUES.md) and [`../sema-specific-adoptions.md`](../sema-specific-adoptions.md).

## Index

### Stack / local allocation
- [`stack-allocation-intro.md`](./stack-allocation-intro.md) — local vs global, regions, `stack_`, `exclave_`
- [`stack-allocation-pitfalls.md`](./stack-allocation-pitfalls.md) — common escape errors
- [`stack-allocation-reference.md`](./stack-allocation-reference.md) — full feature reference (largest, most detailed)

### Unboxed types, kinds, layouts
- [`unboxed-types-intro.md`](./unboxed-types-intro.md) — unboxed scalars/products, mixed blocks, unboxed arrays
- [`unboxed-types-or-null.md`](./unboxed-types-or-null.md) — `or_null` non-allocating option
- [`unboxed-types-block-indices.md`](./unboxed-types-block-indices.md) — block index access
- [`kinds-intro.md`](./kinds-intro.md) — kind = layout + modal/with/non-modal bounds
- [`kinds-non-modal.md`](./kinds-non-modal.md) — non-modal bounds
- [`kinds-types.md`](./kinds-types.md) — kinds of types

### Modes / uniqueness
- [`modes-intro.md`](./modes-intro.md) — the mode lattice (locality, linearity, contention, portability…)
- [`modes-reference.md`](./modes-reference.md) — mode reference
- [`uniqueness-intro.md`](./uniqueness-intro.md) — unique/aliased, once/many, in-place update
- [`uniqueness-pitfalls.md`](./uniqueness-pitfalls.md)
- [`uniqueness-reference.md`](./uniqueness-reference.md)

### Numeric / SIMD
- [`small-numbers.md`](./small-numbers.md) — float32, int8/int16, char#
- [`simd-intro.md`](./simd-intro.md) — SIMD vector types & intrinsics
- [`immutable-arrays.md`](./immutable-arrays.md) — `iarray`, covariance, stack-allocatable contents

### Static analysis / specialization / observability
- [`zero-alloc-checker.md`](./zero-alloc-checker.md) — `[@zero_alloc]` static checker (largest analysis doc)
- [`templates-intro.md`](./templates-intro.md) — `ppx_template` monomorphization
- [`tracing-probes.md`](./tracing-probes.md) — `[%probe ...]` low-overhead tracing

### Parallelism
- [`parallelism-intro.md`](./parallelism-intro.md) — stub overview
- [`parallelism-capsules.md`](./parallelism-capsules.md) — capsules: keys/passwords/mutexes
- [`tutorial-parallelism-part1.md`](./tutorial-parallelism-part1.md) — full tutorial (most substantive)
- [`tutorial-parallelism-part2.md`](./tutorial-parallelism-part2.md)

## Not mirrored (not optimization-focused)

comprehensions, labeled-tuples, include-functor, module-strengthening, polymorphic-parameters,
custom-error-messages, and the modes/kinds *syntax* sub-pages. The OxCaml bibliography also lists
conference talks (links on the source site) covering modal memory management, mixed blocks, the
non-allocating option, the Flambda2 validator, and data-race freedom.
