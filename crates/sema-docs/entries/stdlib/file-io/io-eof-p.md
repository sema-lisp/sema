---
name: "io/eof?"
module: "file-io"
section: "Console I/O"
---

Return `#t` after any stdin read (`io/read-line`, `io/read-stdin`, `io/read-key`) has signalled EOF. Non-breaking alternative to checking `io/read-line` for `nil`.

```sema
(define line (io/read-line))
(when (io/eof?)
  (println "stdin closed"))
```
