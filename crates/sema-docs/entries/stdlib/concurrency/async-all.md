---
name: "async/all"
module: "concurrency"
section: "Promises"
params: [{ name: promises, type: "list | vector" }]
returns: "list"
see_also: ["async/race", "async/spawn", "async/spawn-all", "async/await"]
---

```sema
(async/all promises) → list
```

Await every promise and return a list of their values, in the **same order** as the input (not completion order). Takes a list or vector of promises. The tasks themselves run concurrently — `async/all` only collects them.

If any promise rejects, `async/all` re-raises that error. So it is all-or-nothing: a single failure aborts the whole batch (use per-task `try`/error handling inside each thunk if you want partial results).

To spawn *and* await in one step, prefer `async/map` (apply a function to each item) or `async/spawn-all` (run a list of thunks) — `async/all` is for promises you already hold.

```sema
(let ((p1 (async 10))
      (p2 (async 20))
      (p3 (async 30)))
  (async/all (list p1 p2 p3)))  ; => (10 20 30)
```
