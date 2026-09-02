---
name: "term/query-cursor-position"
module: "terminal"
section: "Screen Control"
syntax: "(term/query-cursor-position)"
returns: "nil"
see_also: ["term/cursor-position", "io/read-key", "term/move-to"]
---

Request a cursor-position report (DSR, `CSI 6n`) and arm the reply decoder. The reply arrives through `io/read-key` as `{:kind :cpr :row R :col C}` (1-based). Arming matters: a `CSI…R` is otherwise byte-identical to modified-F3 (`CSI 1;<mod>R`), so `io/read-key` only reports `:cpr` when a query is outstanding. For a synchronous result, use `term/cursor-position`. Takes no arguments.

It writes the escape sequence to stdout and flushes it, then returns `nil`. It raises an arity error when called with arguments, and an I/O error if stdout cannot be written. A terminal that does not support the sequence ignores it.

```sema
(io/with-raw-mode
  (term/query-cursor-position)
  (io/read-key-timeout 100))   ; {:kind :cpr :row R :col C}, or nil
```
