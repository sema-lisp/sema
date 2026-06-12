---
name: "async/rejected?"
module: "concurrency"
section: "Promises"
params: [{ name: promise, type: promise }]
returns: "bool"
---

`#t` exactly when `promise` is in the `Rejected` state. This excludes `Cancelled` (its own peer state), so the terminal-state predicates partition cleanly: a promise is at most one of `async/resolved?` / `async/rejected?` / `async/cancelled?`. Errors if the argument is not a promise.

```sema
(async/rejected? (async/rejected "boom"))  ; => #t
```
