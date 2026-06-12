---
name: "read-line"
module: "file-io"
section: "Console I/O"
returns: "string | nil"
---

Read one line from standard input, with the trailing newline removed. Returns `nil` at end of input. Alias: `io/read-line`.

```sema
(read-line)   ; => "user typed text"
```
