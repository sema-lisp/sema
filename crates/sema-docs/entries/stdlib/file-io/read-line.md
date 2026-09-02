---
name: "read-line"
module: "file-io"
section: "Console I/O"
returns: "string | nil"
params: []
see_also: ["read-stdin", "io/eof?", "io/read-line"]
syntax: "(read-line)"
---

Read one line from standard input, with the trailing newline removed. Returns `nil` at end of input. Alias: `io/read-line`.

```sema
(read-line)   ; => "user typed text"
```
