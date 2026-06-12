---
name: "async/forced?"
module: "concurrency"
section: "Promises"
params: [{ name: promise, type: promise }]
returns: "bool"
---

`#t` if a lazy promise (created with `delay`) has already been forced and cached its value, `#f` if it has not yet been evaluated. Canonical slash-namespaced alias of `promise-forced?`. Errors if the argument is not a promise.

```sema
(define p (delay (+ 1 2)))
(async/forced? p)   ; => #f
(force p)
(async/forced? p)   ; => #t
```
