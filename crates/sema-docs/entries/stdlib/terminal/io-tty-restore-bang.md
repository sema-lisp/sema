---
name: "io/tty-restore!"
module: "terminal"
section: "Raw-Mode Input"
---

Restore the TTY to cooked mode using the token returned by `io/tty-raw!`.

```sema
(io/tty-restore! tok)
```
