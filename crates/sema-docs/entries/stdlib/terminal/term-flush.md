---
name: "term/flush"
module: "terminal"
section: "Screen Control"
syntax: "(term/flush)"
returns: "nil"
see_also: ["term/write-at", "term/move-to", "display"]
---

Flush buffered stdout. The other `term/*` control functions self-flush; use this when you batch styled `io/print` writes and want to present a frame all at once. Takes no arguments.

It returns `nil`. It raises an arity error when called with arguments, and an I/O error if stdout cannot be flushed. Calling it when nothing is buffered is harmless.

```sema
(io/print (term/green "ok"))
(io/print " ")
(io/print (term/red "fail"))
(term/flush)
```
