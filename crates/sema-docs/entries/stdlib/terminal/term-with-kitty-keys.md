---
name: "term/with-kitty-keys"
module: "terminal"
section: "Screen Control"
syntax: "(term/with-kitty-keys body ...)"
returns: "any"
see_also: ["term/enable-kitty-keys!", "term/disable-kitty-keys!", "term/supports-kitty-keys?"]
---

Guard macro: push the kitty keyboard protocol flags (default 17 = disambiguate + associated-text), run `body`, and **always** pop them on exit — even if `body` throws. Returns `body`'s value. Terminals without kitty support ignore it, so this is safe to wrap unconditionally.

```sema
(io/with-raw-mode
  (term/with-kitty-keys
    (run-tui)))
```
