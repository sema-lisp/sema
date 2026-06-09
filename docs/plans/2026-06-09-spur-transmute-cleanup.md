# Spur Transmute Cleanup

**Date:** 2026-06-09
**Status:** Pending
**Extracted from:** `docs/done/plans/2026-02-17-critical-fixes.md` item 12 (the only item left open when that doc was closed)

## Problem

`transmute::<u32, Spur>` (and the reverse) is used to pack interned-string keys into NaN-boxed `Value` payloads. `lasso::Spur` is documented as a newtype over `NonZeroU32`, but its layout is not a stable public guarantee — if lasso changes the internal representation, these transmutes break silently (or become UB).

Current call sites (2026-06-09):

- `crates/sema-core/src/value.rs:681, 692` — Spur → u32 when boxing symbols/keywords
- `crates/sema-core/src/value.rs:985, 993` — u32 → Spur when unboxing
- `crates/sema-core/src/value.rs:1262, 1279` — u32 → Spur in `as_symbol`/`as_keyword`-style accessors
- `crates/sema-vm/src/vm.rs:878, 896, 907, 1387` — u32 → Spur in global-lookup / inline-cache hot paths

## Approach

Replace transmutes with lasso's safe conversion API:

- Spur → u32: `spur.into_inner().get()` (`Key::into_usize` / `NonZeroU32` accessor)
- u32 → Spur: `Spur::try_from_usize(bits as usize - 1)` or `Key::try_from_usize` — note lasso's key/usize conversions are offset-by-one (`into_usize` returns `get() - 1`); verify round-trip with a test rather than assuming.

Centralize in two `#[inline(always)]` helpers in `sema-core` (e.g. `spur_to_bits` / `bits_to_spur`) so there is exactly one place encoding the assumption, used by both sema-core and sema-vm.

## Constraints

- These sit on VM hot paths (LoadGlobal/inline cache). Verify with `make bench-vm` that the safe conversion compiles to the same code (it should — `NonZeroU32` get/new_unchecked is free; `try_from_usize` adds a branch that the optimizer should fold with the existing payload checks). If a measurable regression appears, keep a single `unsafe` helper with a `const` layout assertion + round-trip debug_assert instead of raw transmutes at every site.
- A compile-time guard (e.g. `const _: () = assert!(size_of::<Spur>() == 4);` plus a unit test round-tripping a freshly interned Spur through the helpers) should land regardless of which variant wins.

## Done When

- Zero `transmute::<u32, Spur>` / `transmute(spur)` outside the (at most one) centralized helper
- Round-trip unit test in sema-core
- `make bench-vm` shows no regression on global-heavy benchmarks (higher-order-fold, deriv)
