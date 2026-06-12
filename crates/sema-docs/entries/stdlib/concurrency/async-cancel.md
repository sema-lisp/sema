---
name: "async/cancel"
module: "concurrency"
section: "Promises"
---

```sema
(async/cancel promise) → bool
```

Request cancellation of a spawned task. Returns `#t` if the call actually transitioned the promise into the `Cancelled` state, `#f` if there was nothing to cancel — the promise was already terminal (resolved, rejected, previously cancelled) or was never spawned in the first place (e.g. created via `async/resolved`).

Cancellation is best-effort and never errors. The next time the task hits a yield point, it transitions to `Cancelled`; subsequent `(await p)` raises `"async/await: task was cancelled"` (distinct from a normal rejection).

```sema
(async/cancel (async/resolved 1))                ;; => #f  (never spawned)
(let ((p (async 42))) (await p) (async/cancel p)) ;; => #f  (already resolved)
(let ((p (async (async/sleep 100)))) (async/cancel p)) ;; => #t
```
