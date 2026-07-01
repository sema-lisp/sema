---
name: "term/with-mouse"
module: "terminal"
section: "Screen Control"
---

Guard macro: enable mouse reporting (`term/enable-mouse`), run `body`, and
**always** disable it on exit — even if `body` throws (the error is re-raised
after disabling). Returns `body`'s value. Without the guard, a crash leaves mouse
reporting on and escape reports spew into the shell as garbage. `io/read-key`
decodes reports as `{:kind :mouse …}` while enabled.
