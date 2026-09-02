---
name: "read-stdin"
module: "file-io"
section: "Console I/O"
returns: string
params: []
see_also: ["read-line", "io/eof?", "io/read-stdin"]
syntax: "(read-stdin)"
---

Read all of standard input to end-of-file and return it as a single string. Alias: `io/read-stdin`.

```sema
(read-stdin)   ; => entire piped stdin contents as a string
```
