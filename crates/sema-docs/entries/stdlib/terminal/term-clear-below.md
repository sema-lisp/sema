---
name: "term/clear-below"
module: "terminal"
section: "Screen Control"
syntax: "(term/clear-below)"
returns: "nil"
see_also: ["term/clear", "term/clear-line", "term/move-to"]
---

Clear from the cursor to the end of the screen. Takes no arguments.

It writes the escape sequence to stdout and flushes it, then returns `nil`. It raises an arity error when called with arguments, and an I/O error if stdout cannot be written. A terminal that does not support the sequence ignores it.

```sema
(term/move-to 10 1)
(term/clear-below)
```
