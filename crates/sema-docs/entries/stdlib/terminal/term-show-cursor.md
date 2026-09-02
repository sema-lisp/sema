---
name: "term/show-cursor"
module: "terminal"
section: "Screen Control"
syntax: "(term/show-cursor)"
returns: "nil"
see_also: ["term/hide-cursor", "term/with-alt-screen"]
---

Show the text cursor again after `term/hide-cursor`. Takes no arguments.

It writes the escape sequence to stdout and flushes it, then returns `nil`. It raises an arity error when called with arguments, and an I/O error if stdout cannot be written. A terminal that does not support the sequence ignores it.

```sema
(term/hide-cursor)
(draw-frame)
(term/show-cursor)
```
