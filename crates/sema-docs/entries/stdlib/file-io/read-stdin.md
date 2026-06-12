---
name: "read-stdin"
module: "file-io"
section: "Console I/O"
returns: string
---

Read all of standard input to end-of-file and return it as a single string. Alias: `io/read-stdin`.

```sema
(read-stdin)   ; => entire piped stdin contents as a string
```
