---
name: "async/cancel"
module: "concurrency"
section: "Promises"
params: [{ name: promise, type: promise }]
returns: "bool"
see_also: ["async/cancelled?", "async/with-timeout", "async/race-owned"]
---

```sema
(async/cancel promise) → bool
```

Request cancellation of a spawned task. Returns `#t` when this call records the first cancellation request for a still-pending spawned task, `#f` if there was nothing to cancel — the promise was already terminal (resolved, rejected, previously cancelled) or was never spawned in the first place (e.g. created via `async/resolved`).

Cancellation is a request, not a synchronous transition. A task that has not started yet settles as `Cancelled` immediately; a task already parked on a wait (a timer, a channel operation, an offloaded blocking call) settles when the runtime tears its wait down, which happens after this call returns. So `(async/cancelled? p)` read immediately after a successful `(async/cancel p)` can still be `#f` for a parked task. Await the promise (under `try` — awaiting a cancelled promise raises the `:cancelled` condition) to synchronize; after the promise settles, `async/cancelled?` is deterministic.

Cancellation is best-effort and never errors. Subsequent `(await p)` raises the `:cancelled` condition (distinct from a normal rejection).

```sema
(async/cancel (async/resolved 1))                ;; => #f  (never spawned)
(let ((p (async 42))) (await p) (async/cancel p)) ;; => #f  (already resolved)
(let ((p (async (async/sleep 100)))) (async/cancel p)) ;; => #t

;; Parked task: the request is recorded (#t) but the promise settles later —
;; synchronize with await before reading async/cancelled?.
(let ((ch (channel/new 1)))
  (let ((p (async (channel/recv ch))))
    (async/sleep 1)                       ;; p is now parked on the recv
    (list (async/cancel p)                ;; => #t  (request recorded)
          (async/cancelled? p)            ;; => #f  (not yet settled)
          (try (async/await p) (catch e (:type e)))  ;; => :cancelled
          (async/cancelled? p))))         ;; => #t  (settled)
```
