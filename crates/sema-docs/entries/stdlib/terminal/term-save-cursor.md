---
name: "term/save-cursor"
module: "terminal"
section: "Screen Control"
syntax: "(term/save-cursor)"
returns: "nil"
see_also: ["term/restore-cursor", "term/move-to", "term/cursor-home"]
---

Save the current cursor position, to be restored later with `term/restore-cursor`. Takes no arguments.

It writes the escape sequence to stdout and flushes it, then returns `nil`. It raises an arity error when called with arguments, and an I/O error if stdout cannot be written. A terminal that does not support the sequence ignores it.

```sema
(term/save-cursor)
(term/write-at 1 1 "header")
(term/restore-cursor)
```
