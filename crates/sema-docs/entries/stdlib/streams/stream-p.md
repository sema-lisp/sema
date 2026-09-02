---
name: "stream?"
module: "streams"
section: "Introspection"
params: [{ name: value, type: any }]
returns: "bool"
see_also: ["stream/type", "stream/readable?"]
---

Type predicate — returns `#t` if the value is a stream.

```sema
(stream? (stream/byte-buffer))    ;; => #t
(stream? 42)                      ;; => #f
```
