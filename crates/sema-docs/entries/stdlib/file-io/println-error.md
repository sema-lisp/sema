---
name: "println-error"
module: "file-io"
section: "Console I/O"
returns: nil
---

Write its arguments to standard error, separated by spaces, followed by a newline. Alias: `io/println-error`.

```sema
(println-error "error:" 42)   ; writes "error: 42\n" to stderr
```
