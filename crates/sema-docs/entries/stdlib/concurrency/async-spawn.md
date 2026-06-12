---
name: "async/spawn"
module: "concurrency"
section: "Promises"
---

```sema
(async/spawn thunk) → async-promise
```

Spawn a zero-argument function as an async task. Returns a promise that resolves when the task completes.

```sema
(define p (async/spawn (fn () (+ 1 2))))
(async/await p)  ; => 3
```

Usually called via the `async` special form:

```sema
(define p (async (+ 1 2)))
(await p)  ; => 3
```
