---
name: "display"
module: "file-io"
section: "Console I/O"
syntax: "(display value ...)"
returns: "nil"
see_also: ["print", "println", "newline", "io/flush"]
---

Print a value in human-readable form (no quotes around strings) without a trailing newline.

`display` is the "for humans" printer; `print` is the "for machines" printer (read-syntax — outer strings are quoted). `println` is `display` plus a newline. Use `display` to build up output on one line, then `(newline)` or `(io/flush)` when done.

```sema
(display "hello")   ; outputs: hello   (no quotes)
(print "hello")     ; outputs: "hello" (quoted, re-readable)
(display 42)        ; outputs: 42
```
