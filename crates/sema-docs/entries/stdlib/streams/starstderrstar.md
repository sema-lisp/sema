---
name: "*stderr*"
module: "streams"
section: "Standard Streams"
returns: stream
syntax: "*stderr*"
see_also: ["*stdout*", "*stdin*", "stream/write-string", "println-error"]
---

The standard error stream. A writable, non-readable stream of type `"stderr"`. Use with `stream/write`, `stream/write-string`, etc.

```sema
(stream/type *stderr*)       ; => "stderr"
(stream/writable? *stderr*)  ; => #t
```
