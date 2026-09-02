---
name: "term/disable-mouse"
module: "terminal"
section: "Screen Control"
syntax: "(term/disable-mouse)"
returns: "nil"
see_also: ["term/enable-mouse", "term/with-mouse"]
---

Disable mouse event reporting previously turned on with `term/enable-mouse`. Takes no arguments.

It writes the escape sequence to stdout and flushes it, then returns `nil`. It raises an arity error when called with arguments, and an I/O error if stdout cannot be written. A terminal that does not support the sequence ignores it.

```sema
(io/with-raw-mode
  (term/enable-mouse)
  (io/read-key)
  (term/disable-mouse))
```
