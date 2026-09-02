---
name: "term/enter-alt-screen"
module: "terminal"
section: "Screen Control"
syntax: "(term/enter-alt-screen)"
returns: "nil"
see_also: ["term/leave-alt-screen", "term/with-alt-screen", "term/hide-cursor"]
---

Switch to the terminal's alternate screen buffer. Use at app start so the TUI gets a clean canvas; pair with `term/leave-alt-screen` on exit to restore the user's scrollback exactly as it was. Takes no arguments.

It writes the escape sequence to stdout and flushes it, then returns `nil`. It raises an arity error when called with arguments, and an I/O error if stdout cannot be written. A terminal that does not support the sequence ignores it.

```sema
(term/enter-alt-screen)
(term/clear)
(term/write-at 1 1 "hello")
(term/leave-alt-screen)
```
