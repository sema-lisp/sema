---
name: "term/enable-bracketed-paste"
module: "terminal"
section: "Screen Control"
syntax: "(term/enable-bracketed-paste)"
returns: "nil"
see_also: ["term/disable-bracketed-paste", "term/with-bracketed-paste", "io/read-key"]
---

Enable bracketed paste mode (`CSI ?2004h`). The terminal then wraps pasted text in markers, so `io/read-key` returns a whole paste as `{:kind :paste :text "…"}` instead of interpreting its newlines and control bytes as live keystrokes (a real injection vector otherwise). Pair with `term/disable-bracketed-paste`, or use the `term/with-bracketed-paste` guard to disable automatically on exit. Takes no arguments.

It writes the escape sequence to stdout and flushes it, then returns `nil`. It raises an arity error when called with arguments, and an I/O error if stdout cannot be written. A terminal that does not support the sequence ignores it.

```sema
(io/with-raw-mode
  (term/enable-bracketed-paste)
  (io/read-key)          ; {:kind :paste :text "..."} on a paste
  (term/disable-bracketed-paste))
```
