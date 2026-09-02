---
name: "term/cursor-position"
module: "terminal"
section: "Raw-Mode Input"
syntax: "(term/cursor-position)"
returns: "map or nil"
see_also: ["term/query-cursor-position", "io/with-raw-mode", "term/move-to"]
---

Round-trip the cursor position: send a DSR request (`CSI 6n`) and read the reply, returning `{:row R :col C}` (1-based) or `nil` when stdin is not a TTY or no reply arrives. **Must be called in raw mode.** Unlike `term/query-cursor-position` (which writes the request and lets the reply come back through `io/read-key`), this blocks briefly and returns the position directly.

```sema
(io/with-raw-mode (term/cursor-position))   ; => {:row 12 :col 40}
```
