---
name: "async/pending?"
module: "concurrency"
section: "Promises"
params: [{ name: promise, type: promise }]
returns: "bool"
---

`#t` if `promise` is still pending (has not yet resolved, rejected, or been cancelled), `#f` otherwise. Errors if the argument is not a promise.

```sema
(async/pending? (async (do-work)))  ; => #t (before the scheduler runs)
```
