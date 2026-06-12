---
name: "stream?"
module: "streams"
section: "Introspection"
---

Type predicate — returns `#t` if the value is a stream.

```sema
(stream? (stream/byte-buffer))    ;; => #t
(stream? 42)                      ;; => #f
```
