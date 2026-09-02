---
name: "io/eof?"
module: "file-io"
section: "Console I/O"
params: []
returns: "bool"
see_also: ["io/read-line", "io/read-stdin", "read-line"]
syntax: "(io/eof?)"
---

Return `#t` after any stdin read (`io/read-line`, `io/read-stdin`, `io/read-key`) has signalled EOF. Non-breaking alternative to checking `io/read-line` for `nil`.

```sema
(define line (io/read-line))
(when (io/eof?)
  (println "stdin closed"))
```
