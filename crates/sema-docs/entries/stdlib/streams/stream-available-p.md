---
name: "stream/available?"
module: "streams"
section: "Introspection"
---

Returns `#t` if data is ready to read without blocking.

```sema
(stream/available? (stream/from-string "x"))  ;; => #t
(stream/available? (stream/from-string ""))   ;; => #f
```
