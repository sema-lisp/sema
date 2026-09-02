---
name: "*stdin*"
module: "streams"
section: "Standard Streams"
returns: stream
syntax: "*stdin*"
see_also: ["*stdout*", "*stderr*", "stream/read-line", "read-line"]
---

The standard input stream. A readable, non-writable stream of type `"stdin"`. Use with `stream/read`, `stream/read-line`, etc.

```sema
(stream/type *stdin*)       ; => "stdin"
(stream/readable? *stdin*)  ; => #t
```
