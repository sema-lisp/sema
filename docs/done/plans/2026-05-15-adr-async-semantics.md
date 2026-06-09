# ADR — Async semantics pass (A1 + A4 + D2)

Status: **ACCEPTED & SHIPPED**. All three recommendations adopted as-is. See `CHANGELOG.md` "Unreleased" for the user-facing summary and `website/docs/stdlib/concurrency.md` for the documentation update.

## Context

Three papercuts in the v1.14 async/concurrency layer that are tied tightly enough to want to land together:

- **A1** Scheduler picks ready tasks in *swap-remove* order, producing a LIFO-ish surface that surprises users writing FIFO pipelines.
- **A4** `async/cancel` returns three different shapes (`nil`, error, silent no-op) for what looks like the same operation.
- **D2** Cancellation state is encoded as `PromiseState::Rejected("cancelled")` — a magic string that `async/cancelled?` matches via string compare, aliasing any user that manually rejects with `"cancelled"`.

All three live in `crates/sema-vm/src/scheduler.rs` + `crates/sema-stdlib/src/async_ops.rs` + `crates/sema-core/src/value.rs::PromiseState`. One commit cleanly covers all three.

---

## A1 — Scheduler ready-task pickup order

### Current behavior

```sema
(let ((ch (channel/new 1)))
  (let ((s1 (async (channel/send ch 1)))
        (s2 (async (channel/send ch 2)))
        (s3 (async (channel/send ch 3)))
        (r  (async (list (channel/recv ch) (channel/recv ch) (channel/recv ch)))))
    (await r)))
;; observed: (1 3 2)   — most users would expect (1 2 3)
```

### Root cause (verified by reading the code)

NOT the wake-list ordering as the audit guessed — `wake_blocked_tasks` already iterates `for task in &mut self.tasks` in insertion order. The actual culprit is `crates/sema-vm/src/scheduler.rs:529`:

```rust
let mut task = sched.tasks.swap_remove(idx);
```

`swap_remove` is `O(1)` but it moves the **last** element into position `idx`. So after picking task 0 (s1), task 3 (r) is moved to position 0, then s3 to 1, s2 to 2. Next ready scan finds r at position 0; after r blocks again the scan finds s3 at position 0, etc. The sequence (1 3 2) is exactly what `swap_remove` produces.

### Options

**Option A1-1 (recommended): use `Vec::remove(idx)`.**
- O(n) per task pickup (n = ready tasks), which for typical async workloads of <100 tasks is fine.
- One-line code change. Preserves spawn order strictly.
- Result for the test case: `(1 2 3)`.

**Option A1-2: switch `tasks: Vec<Task>` to `VecDeque<Task>`.**
- Requires reworking every random-access into the queue (cancel by id, blocked-task wake scan, etc.).
- Large diff, no real speedup.
- Reject.

**Option A1-3: leave LIFO, document it as the contract.**
- Users learn the surprise once and adapt.
- But the surprise is subtle (only shows up under contention); no idiom rewards it; and it's strictly worse than FIFO for the common producer-consumer pattern.
- Reject.

### Decision needed
✅ **A1-1: switch to `Vec::remove(idx)`** (recommended)

### Test we'll add
`channel_sender_order_preserved`: assert the snippet above returns `(1 2 3)` on the VM backend.

---

## A4 — `async/cancel` consistent semantics

### Current behavior (verified at HEAD)

| Case | Today |
|---|---|
| Cancel a pending spawned task | sets `PromiseState::Rejected("cancelled")`, returns `nil` |
| `async/cancel` again on the same (now-cancelled) promise | silent no-op, returns `nil` |
| Cancel a promise that already resolved (e.g. `(let ((p (async 42))) (await p) (async/cancel p))`) | silent no-op, returns `nil` |
| Cancel a never-spawned promise (`(async/cancel (async/resolved 1))`) | **errors** with `"async/cancel: cannot cancel a non-spawned promise"` |

Three different behaviors for what is morally one operation: "make this promise cancelled if you can".

### Options

**Option A4-1 (recommended): return a boolean. `#t` when state actually transitioned to Cancelled. `#f` for everything else (resolved, rejected, already-cancelled, never-spawned).**

```sema
(async/cancel p) → #t | #f
```

- Drops the "non-spawned promise" error.
- Callers that ignore the return value still work (just discard `#t`/`#f` like they were discarding `nil`).
- Callers that wanted to know "did I actually cancel something?" can now ask.
- Consistent with `Map.delete()` in JS, `Hash#delete` in Ruby, etc.

**Option A4-2: return `nil` always; raise only on truly malformed input (not-a-promise).**
- Closer to today's behavior but uniform.
- Loses the "did anything happen" signal.
- Acceptable but less useful.

**Option A4-3: keep the error on never-spawned but make double-cancel + cancel-after-await also error.**
- Strictest. Forces the caller to know the state before cancelling.
- High-friction; cancellation is supposed to be best-effort.
- Reject.

### Decision needed
✅ **A4-1: `async/cancel` returns boolean.**

### Test we'll add (and update existing)

```sema
(async/cancel (async/resolved 42))                 ; => #f (was: error)
(let ((p (async 42))) (await p) (async/cancel p))  ; => #f (was: nil)
(let ((p (async (async/sleep 100))))
  (async/cancel p)
  (async/cancel p))                                 ; second call => #f
```

Plus: `(async/cancel p)` on a still-pending spawn → `#t`.

---

## D2 — Structured cancellation, not a magic string

### Current behavior

```rust
// crates/sema-core/src/value.rs
pub enum PromiseState {
    Pending,
    Resolved(Value),
    Rejected(String),
}
```

```rust
// crates/sema-stdlib/src/async_ops.rs:220
let is_cancelled = matches!(&*state, PromiseState::Rejected(msg) if msg == "cancelled");
```

A user who does `(async/rejected "cancelled")` will fool `async/cancelled?` into returning `#t`. And `async/rejected?` returns `#t` for cancelled promises (since cancellation IS encoded as a rejection).

### Options

**Option D2-1 (recommended): add `PromiseState::Cancelled` as a peer variant.**

```rust
pub enum PromiseState {
    Pending,
    Resolved(Value),
    Rejected(String),
    Cancelled,
}
```

Then:
- `async/cancel` (when transitioning) sets `PromiseState::Cancelled`.
- `async/cancelled?` matches the variant directly.
- `async/rejected?` returns `#f` for Cancelled (cancellation is its own thing).
- `async/await` of a cancelled promise raises a clear `"async/await: task was cancelled"` error.

**Option D2-2: enrich `Rejected(String)` to `Rejected(RejectionKind)` where `RejectionKind = User(String) | Cancelled`.**
- More extensible (room for `Timeout`, `Interrupted`, etc. later) but more invasive.
- Every callsite that pattern-matches on `Rejected(s)` now needs the inner pattern.
- Display impl needs updating.
- Reject for now — over-engineering for one failure mode. Can do later if we add more system-level rejections.

**Option D2-3: keep the magic string, document it as reserved.**
- Cheap but doesn't fix the problem; users can still alias.
- Reject.

### Decision needed
✅ **D2-1: add `PromiseState::Cancelled` peer variant.**

### Implications by file

| File | Change |
|---|---|
| `crates/sema-core/src/value.rs` | new `PromiseState::Cancelled` variant; `Display` impl prints `<async-promise cancelled>`; `PartialEq` needs update if hand-impl |
| `crates/sema-vm/src/scheduler.rs` | `cancel_task` and the cancelled-before-running paths set `PromiseState::Cancelled` (was: `Rejected("cancelled")`) |
| `crates/sema-stdlib/src/async_ops.rs` | `async/cancelled?` matches the variant; `async/rejected?` excludes Cancelled; `async/await` reports cancellation distinctly; `async/cancel` returns bool (A4) |
| WASM bindings | `crates/sema-wasm/src/lib.rs` debug-state code paths that match `PromiseState`. Search for it. |
| Tests | add `async_cancel_returns_bool`, `async_cancelled_is_distinct_from_rejected`, `await_cancelled_promise_errors` |

### Test we'll add

```sema
(async/cancelled? (async/rejected "cancelled"))     ; => #f  (no longer aliased)
(let ((p (async (async/sleep 100))))
  (async/cancel p)
  (list (async/cancelled? p) (async/rejected? p)))  ; => (#t #f)
```

---

## Combined implementation plan (after the user confirms)

1. **`PromiseState::Cancelled`** — add variant in `sema-core/src/value.rs`. Update `Display`. Compile-fail every match site; fix in scheduler + async_ops. (~30 min)
2. **Scheduler order** — change `swap_remove(idx)` → `remove(idx)` in `scheduler.rs:529`. (~5 min)
3. **`async/cancel` boolean** — rewrite the body in `async_ops.rs:201-211`. Drop the `task_id == 0` error path; return `Value::bool(transitioned)`. The scheduler's `cancel_task` already returns `Result<(), SemaError>` — change to `Result<bool, SemaError>` (true = transitioned, false = no-op) and propagate up via the existing `set_cancel_callback` plumbing.
4. **`async/cancelled?`** — match `PromiseState::Cancelled` directly.
5. **`async/rejected?`** — explicitly exclude Cancelled (return false).
6. **`async/await`** — when state is Cancelled, raise `"async/await: task was cancelled"` (don't surface as a normal rejection).
7. **WASM bindings** — search `crates/sema-wasm/` for `PromiseState::` matches; add Cancelled arm.
8. **Tests** — 5-7 new dual-eval / vm_async tests covering each of A1, A4, D2.
9. **Docs** — `website/docs/stdlib/concurrency.md`:
   - "Scheduling guarantees" sub-section noting FIFO pickup order
   - `async/cancel` doc updated for boolean return
   - `async/cancelled?` doc clarifies it's distinct from `async/rejected?`
   - Add `Cancelled` to the promise-state table if there is one

### Migration notes for CHANGELOG

```
### Changed (v1.15.0)
- (async/cancel p) now returns a boolean (#t if state actually transitioned
  to Cancelled, #f if the promise was already terminal or never spawned).
  Previous behavior: returned nil on success, errored on never-spawned.
- (async/cancelled? p) now matches a dedicated PromiseState::Cancelled
  variant instead of string-comparing the rejection message. Manually
  rejecting with (async/rejected "cancelled") no longer fools the predicate.
- (async/rejected? p) now returns #f for cancelled promises (cancellation
  is a distinct outcome).
- Awaiting a cancelled promise raises "async/await: task was cancelled"
  instead of "async/await: task rejected: cancelled".
- Scheduler ready-task pickup is now strictly FIFO (was: swap-remove,
  which produced LIFO-feeling order under contention).
```

### Estimated effort

~2 hours for the whole pass including tests and docs. Single commit.

---

## Summary of decisions to make

You need to ✅ or ❌ each:

- **A1-1** Use `Vec::remove(idx)` for FIFO ready-task pickup. (recommended ✅)
- **A4-1** `async/cancel` returns a boolean. (recommended ✅)
- **D2-1** Add `PromiseState::Cancelled` as a peer variant. (recommended ✅)

If all three are ✅, the implementation pass is well-scoped and I can do it in one go. If you want to alter any choice (e.g. A4-2 nil-always, or D2-2 RejectionKind enum), say so and I'll revise before coding.
