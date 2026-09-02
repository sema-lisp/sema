---
name: "term/disable-focus-events"
module: "terminal"
section: "Screen Control"
syntax: "(term/disable-focus-events)"
returns: "nil"
see_also: ["term/enable-focus-events", "term/with-focus-events"]
---

Disable focus reporting (`CSI ?1004l`), undoing `term/enable-focus-events`. Call on exit. Takes no arguments.

It writes the escape sequence to stdout and flushes it, then returns `nil`. It raises an arity error when called with arguments, and an I/O error if stdout cannot be written. A terminal that does not support the sequence ignores it.

```sema
(io/with-raw-mode
  (term/enable-focus-events)
  (io/read-key)
  (term/disable-focus-events))
```
