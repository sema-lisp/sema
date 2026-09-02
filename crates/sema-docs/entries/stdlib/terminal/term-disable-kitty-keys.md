---
name: "term/disable-kitty-keys!"
module: "terminal"
section: "Screen Control"
syntax: "(term/disable-kitty-keys!)"
returns: "nil"
see_also: ["term/enable-kitty-keys!", "term/with-kitty-keys"]
---

Pop the kitty keyboard protocol flags pushed by `term/enable-kitty-keys!`, restoring the terminal's previous keyboard mode. Call before `io/tty-restore!` on exit. Takes no arguments.

It writes the escape sequence to stdout and flushes it, then returns `nil`. It raises an arity error when called with arguments, and an I/O error if stdout cannot be written. A terminal that does not support the sequence ignores it.

```sema
(io/with-raw-mode
  (term/enable-kitty-keys!)
  (io/read-key)
  (term/disable-kitty-keys!))
```
