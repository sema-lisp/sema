---
name: "term/enable-focus-events"
module: "terminal"
section: "Screen Control"
syntax: "(term/enable-focus-events)"
returns: "nil"
see_also: ["term/disable-focus-events", "term/with-focus-events", "io/read-key"]
---

Enable focus reporting (`CSI ?1004h`). The terminal then sends an event when the window gains or loses focus, which `io/read-key` decodes as `{:kind :focus :focused #t|#f}` — useful to pause a spinner or repaint when the user tabs away. Pair with `term/disable-focus-events`, or use the `term/with-focus-events` guard. Support is inconsistent (e.g. macOS Terminal.app); treat it as a nice-to-have. Takes no arguments.

It writes the escape sequence to stdout and flushes it, then returns `nil`. It raises an arity error when called with arguments, and an I/O error if stdout cannot be written. A terminal that does not support the sequence ignores it.

```sema
(io/with-raw-mode
  (term/enable-focus-events)
  (io/read-key)          ; {:kind :focus :focused #f} when the window loses focus
  (term/disable-focus-events))
```
