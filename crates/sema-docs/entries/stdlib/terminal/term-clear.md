---
name: "term/clear"
module: "terminal"
section: "Screen Control"
syntax: "(term/clear)"
returns: "nil"
see_also: ["term/clear-line", "term/clear-below", "term/cursor-home"]
---

Clear the entire screen and move the cursor to the top-left (home) position. Takes no arguments.

It writes the escape sequence to stdout and flushes it, then returns `nil`. It raises an arity error when called with arguments, and an I/O error if stdout cannot be written. A terminal that does not support the sequence ignores it.

```sema
(term/clear)
(term/write-at 1 1 "fresh screen")
```
