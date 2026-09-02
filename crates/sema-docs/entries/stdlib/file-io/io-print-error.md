---
name: "io/print-error"
module: "file-io"
section: "Console I/O"
syntax: "(io/print-error value ...)"
returns: "nil"
see_also: ["io/println-error", "print-error", "print"]
---

Print to stderr without a trailing newline.

```sema
(io/print-error "warning: something happened")
```
