---
name: "stream/available?"
module: "streams"
section: "Introspection"
params: [{ name: stream, type: stream }]
returns: "bool"
see_also: ["stream/readable?", "stream/read", "stream/read-line"]
---

Returns `#t` if data is ready to read without blocking.

```sema
(stream/available? (stream/from-string "x"))  ;; => #t
(stream/available? (stream/from-string ""))   ;; => #f
```
