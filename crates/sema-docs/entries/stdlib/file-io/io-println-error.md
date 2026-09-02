---
name: "io/println-error"
module: "file-io"
section: "Console I/O"
syntax: "(io/println-error arg ...)"
returns: "nil"
see_also: ["io/print-error", "println-error", "println"]
---

Print to stderr with a trailing newline.

```sema
(io/println-error "error: file not found")
```
