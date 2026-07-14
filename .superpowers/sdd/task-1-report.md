# Task 02 / Internal Task 1 report

## Outcome

Added the `sema-core::runtime` identity and relationship vocabulary only. The
slice includes checked process-global and runtime-local allocation, scoped
root/promise/channel identities, wait-local completion kinds, and independent
origin/cancellation/lifetime relationships. It adds no executor, native,
context, condition, scheduler, or compatibility behavior.

## RED evidence

The public integration test was added before the runtime module. Running:

```text
cargo test -p sema-core --test runtime_types_test ids -- --nocapture
```

failed with `E0432`/`E0433`: `could not find runtime in sema_core`. This was the
expected missing-feature failure. The test covered the wished-for public ID and
relationship API before implementation; allocator terminal cases were then
kept behind crate-private unit-test seams.

## Files

- `crates/sema-core/src/lib.rs`
- `crates/sema-core/src/runtime/mod.rs`
- `crates/sema-core/src/runtime/ids.rs`
- `crates/sema-core/src/runtime/cancel.rs`
- `crates/sema-core/tests/runtime_types_test.rs`

## GREEN evidence

- `cargo test -p sema-core runtime::ids -- --nocapture` — 6 passed.
- `cargo test -p sema-core --test runtime_types_test ids -- --nocapture` — 2 passed.
- `cargo test -p sema-core --test runtime_types_test relationships -- --nocapture` — 1 passed.
- `cargo test -p sema-core` — 265 unit, 9 property, 3 runtime integration,
  and 1 doc test passed; 1 doc test ignored.
- `cargo clippy -p sema-core --all-targets -- -D warnings` — passed.
- `scripts/check-unified-runtime-legacy.sh --check` — passed.
- `git diff --check` — passed.
- Changed Rust files were formatted with `rustfmt` (workspace-wide fmt check
  also reports unrelated pre-existing formatting drift outside this slice).

## Concerns

None. Allocation exhaustion is intentionally testable only through private unit
test seams; the process-global allocator itself is not mutated to exhaustion by
tests.
