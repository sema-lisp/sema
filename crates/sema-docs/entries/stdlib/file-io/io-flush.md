---
name: "io/flush"
module: "file-io"
section: "Console I/O"
params: []
returns: "nil"
see_also: ["display", "print", "io/read-line"]
syntax: "(io/flush)"
---

Flush stdout. Useful when writing a prompt without a trailing newline before reading input.

```sema
(display "name> ")
(io/flush)
(define name (io/read-line))
```
