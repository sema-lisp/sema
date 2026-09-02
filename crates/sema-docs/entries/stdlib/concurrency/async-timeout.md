---
name: "async/timeout"
module: "concurrency"
section: "Promises"
params: [{ name: ms, type: number }, { name: promise, type: promise }]
returns: "any"
see_also: ["async/with-timeout", "async/cancel", "async/sleep"]
---

```sema
(async/timeout ms promise) → value
```

Wait for `promise` to resolve, but raise an error if it takes longer than `ms` milliseconds. The underlying task is **not** automatically cancelled; pair with `async/cancel` if you need to free its resources.

```sema
(async/timeout 100 (async (do-slow-work)))
;; raises: async/timeout: operation timed out
```

A `ms = 0` (or very short) timeout still lets synchronously-ready work finish — it only fires once the virtual clock reaches the deadline with the task still pending. Durations are capped at `86_400_000` ms (1 day).
