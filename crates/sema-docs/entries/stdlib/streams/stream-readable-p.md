---
name: "stream/readable?"
module: "streams"
section: "Introspection"
aliases: ["stream/writable?"]
params: [{ name: stream, type: stream }]
returns: "bool"
see_also: ["stream/available?", "stream/type", "stream?"]
---

Check the direction of a stream.

```sema
(stream/readable? (stream/from-string "x"))   ;; => #t
(stream/writable? (stream/from-string "x"))   ;; => #f
(stream/writable? (stream/byte-buffer))       ;; => #t
```
