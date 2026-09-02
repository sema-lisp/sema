---
name: "term/query-kitty-keys"
module: "terminal"
section: "Screen Control"
syntax: "(term/query-kitty-keys)"
returns: "nil"
see_also: ["term/supports-kitty-keys?", "term/enable-kitty-keys!", "io/read-key"]
---

Query the terminal's active kitty keyboard protocol flags (`CSI ?u`). The reply arrives asynchronously through `io/read-key` as `{:kind :kitty-flags :flags N}`; a terminal without kitty support sends nothing. For a synchronous yes/no answer, use `term/supports-kitty-keys?` instead. Takes no arguments.

It writes the escape sequence to stdout and flushes it, then returns `nil`. It raises an arity error when called with arguments, and an I/O error if stdout cannot be written. A terminal that does not support the sequence ignores it.

```sema
(io/with-raw-mode
  (term/query-kitty-keys)
  (io/read-key-timeout 100))   ; {:kind :kitty-flags :flags N}, or nil
```
