---
name: "print-error"
module: "file-io"
section: "Console I/O"
returns: nil
syntax: "(print-error value ...)"
see_also: ["println-error", "print", "io/print-error"]
---

Write its arguments to standard error, separated by spaces, with no trailing newline, then flush. Alias: `io/print-error`.

```sema
(print-error "warning:" "disk full")   ; writes "warning: disk full" to stderr
```
