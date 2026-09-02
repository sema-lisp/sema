---
name: "term/clear-line"
module: "terminal"
section: "Screen Control"
syntax: "(term/clear-line)"
returns: "nil"
see_also: ["term/clear", "term/clear-below", "term/move-to"]
---

Clear the line the cursor is on, without moving the cursor. Useful for redrawing a status line or spinner in place. Takes no arguments.

It writes the escape sequence to stdout and flushes it, then returns `nil`. It raises an arity error when called with arguments, and an I/O error if stdout cannot be written. A terminal that does not support the sequence ignores it.

```sema
(term/move-to 5 1)
(term/clear-line)
(io/print "status: ok")
```
