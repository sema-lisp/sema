---
name: "async/resolved?"
module: "concurrency"
section: "Promises"
params: [{ name: promise, type: promise }]
returns: "bool"
---

`#t` if `promise` has settled into the `Resolved` state (it completed with a value), `#f` if it is still pending, rejected, or cancelled. Errors if the argument is not a promise.

```sema
(async/resolved? (async/resolved 42))  ; => #t
```
