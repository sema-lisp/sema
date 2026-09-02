---
name: "term/enable-mouse"
module: "terminal"
section: "Screen Control"
syntax: "(term/enable-mouse)"
returns: "nil"
see_also: ["term/disable-mouse", "term/with-mouse", "io/read-key"]
---

Enable mouse reporting — button events, button-motion (drag), and SGR extended
coordinates — so the terminal sends click/drag/wheel events on stdin. `io/read-key`
decodes them into `{:kind :mouse :action … :x :y :button :mods}` (see `io/read-key`).
Pair with `term/disable-mouse`, or use the `term/with-mouse` guard to disable
automatically on exit. Takes no arguments.

It writes the escape sequence to stdout and flushes it, then returns `nil`. It raises an arity error when called with arguments, and an I/O error if stdout cannot be written. A terminal that does not support the sequence ignores it.

```sema
(io/with-raw-mode
  (term/enable-mouse)
  (io/read-key)          ; {:kind :mouse ...} on a click
  (term/disable-mouse))
```
