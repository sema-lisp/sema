---
name: "term/restore-cursor"
module: "terminal"
section: "Screen Control"
syntax: "(term/restore-cursor)"
returns: "nil"
see_also: ["term/save-cursor", "term/move-to"]
---

Restore the cursor to the position saved by `term/save-cursor`. Takes no arguments.

It writes the escape sequence to stdout and flushes it, then returns `nil`. It raises an arity error when called with arguments, and an I/O error if stdout cannot be written. A terminal that does not support the sequence ignores it.

```sema
(term/save-cursor)
(term/write-at 1 1 "header")
(term/restore-cursor)
```
