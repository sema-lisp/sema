---
name: "io/tty-restore!"
module: "terminal"
section: "Raw-Mode Input"
params: [{ name: token, type: int }]
returns: "nil"
see_also: ["io/tty-raw!", "io/with-raw-mode"]
---

Restore the TTY to cooked mode using the token returned by `io/tty-raw!`.

```sema
(io/tty-restore! tok)
```
