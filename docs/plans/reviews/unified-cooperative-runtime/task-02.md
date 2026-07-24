# Task 02 review: Core runtime data model

Date: 2026-07-14

Reviewed range: `3509bb25..87040f27`, plus the final exact-inventory refresh
recorded with this review.

## Verdict

**APPROVE.** Task 02 defines compile-realistic core runtime contracts without
switching production scheduling or I/O behavior. Independent correctness and
architecture reviews found no unresolved blocker, high, or medium findings
after the fixes below. Oracle's final re-review returned `GO`.

## Resolved findings

| ID | Severity | Finding | Resolution |
| --- | --- | --- | --- |
| `UR-T02-R100` | Medium | Evidence contained trailing whitespace while claiming `git diff --check` passed. | Removed in `87040f27`; current and range checks pass. |
| `UR-T02-R101` | Blocker | The exact runtime inventory became stale after the payload-native constructor shifted `value.rs` lines. | Refreshed the three shifted records, preserving reviewed `C17`/`F15` assignments; inventory and runtime-conformance gates pass. |
| `UR-T02-R201` | High | Runtime-aware natives had no legal way to retain traceable payload state under I2. | `8d3f7abb` added `with_payload_result`: one strong registered payload edge, a `Weak` callback capture, and a function-pointer implementation. |
| `UR-T02-R202` | Medium | Task 03 documented installing private decoder/resource state before bind/split made it available. | `87040f27` specifies issue → bind/split → atomic registration → submit. |
| `UR-T02-R203` / `UR-T02-R303` | Medium | Task 05 named a nonexistent public `ExecutorJob` seam. | `87040f27` specifies private prepared jobs carried only by opaque submission/dispatch wrappers. |
| `UR-T02-R301` | High | Safe abandonment of unadmitted owners could run hostile opaque destructors without containment. | `5f9510e7` added consuming `Option` ownership and contained `Drop` for prepared operations, bindings, submissions, and rejections. |
| `UR-T02-R302` | Medium | Task 03's runtime constructor could not represent permanent runtime-ID exhaustion. | `87040f27` defines `RuntimeCreateError::{IdExhausted, ExecutorAttach}`. |
| `UR-T02-R304` | Medium | Acceptance evidence still described the pre-shift inventory snapshot. | Final evidence and exact map now describe and pass at the acceptance state. |

## Contracts accepted

- IDs are nonzero, non-wrapping, and runtime-scoped where provenance matters.
- Completion authority is private, linear, and split from runtime-local decoder
  and resource state. Admitted paths make one terminal delivery attempt.
- Executor rejection and abandonment preserve the armed/unarmed distinction;
  unwind-mode hostile teardown is contained before or after terminal delivery
  according to ownership state.
- Only send-safe envelopes, jobs, submissions, dispatches, and futures cross
  worker boundaries. Sema values, continuations, decoders, resources, and task
  contexts remain runtime-local.
- Runtime-aware natives receive only `NativeCallContext`; legacy `EvalContext`
  access remains isolated in the compatibility fallback. Traceable callback
  state uses registered payloads with exact GC edge multiplicity.
- Task-context extensions are typed, child inheritance preserves concrete type,
  and handle tracing fails cleanly on an active mutable borrow.
- The raw-ID bridge is explicitly lossy and temporary. Existing callbacks,
  promises, scheduler behavior, and user-facing failure text remain unchanged.

## Verification reviewed

- `cargo test -p sema-core`: 317 unit tests, 23 integration/property tests, and
  1 doc test passed; 1 doc test ignored.
- `cargo test -p sema-lang --test runtime_conformance_test`: 8 passed.
- `cargo fmt --all -- --check`: passed.
- `cargo clippy -p sema-core --all-targets -- -D warnings`: passed.
- `jake docs-check`: passed.
- Legacy scanner: 970 exact matches. Runtime inventory: 1,256 reviewed exact
  matches and no `UNREVIEWED` assignment.
- `git diff --check`: passed.

The seven `vm_async_test` RED characterizations and the ready-spinner fairness
watchdog RED exactly match Task 01 and are not Task 02 regressions.

## Unverified assumptions

- Panic containment is verified under `panic = "unwind"`; abort builds terminate
  by design. Internally double-panicking destructors remain process-fatal.
- Native Windows watchdog behavior still requires the Task 07 CI run.
- WASM host execution was not run; Task 07 owns that adapter.
