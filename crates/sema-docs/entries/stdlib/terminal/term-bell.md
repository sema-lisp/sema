---
name: "term/bell"
module: "terminal"
section: "Screen Control"
syntax: "(term/bell)"
returns: "nil"
see_also: ["term/set-title", "term/flush"]
---

Emit the terminal bell (BEL) — an audible or visible alert depending on terminal settings. Takes no arguments.

It writes the escape sequence to stdout and flushes it, then returns `nil`. It raises an arity error when called with arguments, and an I/O error if stdout cannot be written. A terminal that does not support the sequence ignores it.

```sema
(term/bell)
```
