---
name: "io/read-stdin"
module: "file-io"
section: "Console I/O"
params: []
returns: "string"
see_also: ["read-stdin", "io/read-line", "io/eof?"]
syntax: "(io/read-stdin)"
---

Read all of stdin as a string (until EOF).

```sema
(define input (io/read-stdin))
```
