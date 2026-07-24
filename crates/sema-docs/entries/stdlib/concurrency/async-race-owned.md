---
name: "async/race-owned"
module: "concurrency"
section: "Promises"
---

```sema
(async/race-owned thunks) → value
```

Run a list (or vector) of zero-argument **thunks** concurrently and return the value of the **first one to settle** — then **cancel every loser**. This is the *owned* (structured-concurrency) counterpart to `async/race`: where `async/race` observes already-spawned promises and leaves the losers running, `async/race-owned` owns the tasks it starts and tears the losers down as soon as the winner settles. If the first settlement is an error, that error is re-raised (and the others are still cancelled). An empty input list is an argument error.

```sema
;; The fast thunk wins; the slow one is cancelled, not left running.
(async/race-owned (list (fn () (async/sleep 100) :slow)
                        (fn () (async/sleep 10)  :fast)))  ; => :fast
```

Use `async/race-owned` when the losing work has side effects or holds resources you want stopped promptly; use `async/race` when you deliberately want the other promises to keep running.
