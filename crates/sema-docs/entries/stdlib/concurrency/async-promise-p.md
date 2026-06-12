---
name: "async/promise?"
module: "concurrency"
section: "Promises"
params: [{ name: value }]
returns: "bool"
---

`#t` if `value` is an async promise (e.g. one returned by `async/spawn`, `async/resolved`, or `async/rejected`), `#f` otherwise.

```sema
(async/promise? (async/resolved 42))  ; => #t
(async/promise? 42)                    ; => #f
```
