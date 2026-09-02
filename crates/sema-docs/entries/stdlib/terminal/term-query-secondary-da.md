---
name: "term/query-secondary-da"
module: "terminal"
section: "Screen Control"
syntax: "(term/query-secondary-da)"
returns: "nil"
see_also: ["term/query-primary-da", "io/read-key"]
---

Request Secondary Device Attributes (`CSI >c`). The reply arrives through `io/read-key` as `{:kind :device-attributes :device :secondary :params (…)}`, whose params commonly encode the terminal's type id and version — a more reliable fingerprint than `$TERM` over SSH/tmux. Takes no arguments.

It writes the escape sequence to stdout and flushes it, then returns `nil`. It raises an arity error when called with arguments, and an I/O error if stdout cannot be written. A terminal that does not support the sequence ignores it.

```sema
(io/with-raw-mode
  (term/query-secondary-da)
  (io/read-key-timeout 100))   ; {:kind :device-attributes :device :secondary :params (...)}
```
