---
name: "async/spawn"
module: "concurrency"
section: "Promises"
params: [{ name: thunk, type: function }]
returns: "promise"
see_also: ["async/await", "async/all", "async/cancel", "async/run"]
---

```sema
(async/spawn thunk) → async-promise
```

Spawn a zero-argument function as an async task and immediately return a promise. The task does not run yet — it is queued on the scheduler and makes progress when something drives it (an `await`, `async/run`, or another task yielding). The promise resolves with the thunk's value, or rejects if the thunk raises.

```sema
(define p (async/spawn (fn () (+ 1 2))))
(async/await p)  ; => 3
```

Usually written with the `async` special form, which wraps its body in a thunk for you — `(async expr)` is exactly `(async/spawn (fn () expr))`:

```sema
(define p (async (+ 1 2)))
(await p)  ; => 3
```

To fan out over a list of items, reach for the higher-level helpers instead of spawning by hand: `async/map` / `async/spawn-all` (unbounded) or `async/pool-map` (bounded concurrency).
