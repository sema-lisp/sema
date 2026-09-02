---
name: "term/leave-alt-screen"
module: "terminal"
section: "Screen Control"
syntax: "(term/leave-alt-screen)"
returns: "nil"
see_also: ["term/enter-alt-screen", "term/with-alt-screen"]
---

Leave the alternate screen buffer and restore the primary screen and its scrollback. The inverse of `term/enter-alt-screen`. Takes no arguments.

It writes the escape sequence to stdout and flushes it, then returns `nil`. It raises an arity error when called with arguments, and an I/O error if stdout cannot be written. A terminal that does not support the sequence ignores it.

```sema
(term/enter-alt-screen)
(term/clear)
(term/write-at 1 1 "hello")
(term/leave-alt-screen)
```
