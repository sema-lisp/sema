---
name: "io/with-raw-mode"
module: "terminal"
section: "Screen Control"
syntax: "(io/with-raw-mode body ...)"
returns: "any"
see_also: ["io/tty-raw!", "io/tty-restore!", "term/with-alt-screen", "term/with-mouse"]
---

Guard macro: put the controlling TTY into raw mode (via `io/tty-raw!`), run
`body`, and **always** restore cooked mode on exit — even if `body` throws (the
error is re-raised after restoring). Returns `body`'s value. This guard matters
most: an unrestored raw TTY leaves the shell unusable (no echo, no line editing).
The restore token is handled internally, so `body` binds nothing.

```sema
(io/with-raw-mode
  (let loop ()
    (let ((k (io/read-key)))
      (unless (ctrl-c? k) (handle k) (loop)))))
```
