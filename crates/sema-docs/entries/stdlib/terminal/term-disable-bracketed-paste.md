---
name: "term/disable-bracketed-paste"
module: "terminal"
section: "Screen Control"
syntax: "(term/disable-bracketed-paste)"
returns: "nil"
see_also: ["term/enable-bracketed-paste", "term/with-bracketed-paste"]
---

Disable bracketed paste mode (`CSI ?2004l`), undoing `term/enable-bracketed-paste`. Call on exit so paste markers don't leak into the shell afterward. Takes no arguments.

It writes the escape sequence to stdout and flushes it, then returns `nil`. It raises an arity error when called with arguments, and an I/O error if stdout cannot be written. A terminal that does not support the sequence ignores it.

```sema
(io/with-raw-mode
  (term/enable-bracketed-paste)
  (io/read-key)
  (term/disable-bracketed-paste))
```
