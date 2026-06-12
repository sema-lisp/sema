---
name: "*stdout*"
module: "streams"
section: "Standard Streams"
returns: stream
---

The standard output stream. A writable, non-readable stream of type `"stdout"`. Use with `stream/write`, `stream/write-string`, etc.

```sema
(stream/type *stdout*)       ; => "stdout"
(stream/writable? *stdout*)  ; => #t
```
