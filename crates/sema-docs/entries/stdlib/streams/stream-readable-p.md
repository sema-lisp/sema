---
name: "stream/readable?"
module: "streams"
section: "Introspection"
aliases: ["stream/writable?"]
---

Check the direction of a stream.

```sema
(stream/readable? (stream/from-string "x"))   ;; => #t
(stream/writable? (stream/from-string "x"))   ;; => #f
(stream/writable? (stream/byte-buffer))       ;; => #t
```
