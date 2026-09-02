---
name: "term/cursor-home"
module: "terminal"
section: "Screen Control"
syntax: "(term/cursor-home)"
returns: "nil"
see_also: ["term/move-to", "term/clear", "term/save-cursor"]
---

Move the cursor to the top-left corner (row 1, column 1) without clearing. Takes no arguments.

It writes the escape sequence to stdout and flushes it, then returns `nil`. It raises an arity error when called with arguments, and an I/O error if stdout cannot be written. A terminal that does not support the sequence ignores it.

```sema
(term/cursor-home)
(io/print "top-left")
```
