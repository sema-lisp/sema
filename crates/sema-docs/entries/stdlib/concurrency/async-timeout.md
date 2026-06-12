---
name: "async/timeout"
module: "concurrency"
section: "Promises"
---

```sema
(async/timeout ms promise) → value
```

Wait for `promise` to resolve, but raise an error if it takes longer than `ms` milliseconds. The underlying task is **not** automatically cancelled; pair with `async/cancel` if you need to free its resources.

```sema
(async/timeout 100 (async (do-slow-work)))
;; raises: async/timeout: operation timed out
```

`ms = 0` causes an immediate timeout if the promise has not already resolved.
