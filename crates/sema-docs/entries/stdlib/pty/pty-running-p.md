---
name: "pty/running?"
module: "pty"
section: "Pseudo-Terminals"
params: [{ name: handle, type: int }]
returns: "bool"
see_also: ["pty/exit-code", "pty/wait", "pty/kill"]
---

Return `#t` if the pty's child is still running, `#f` if it has exited.

It polls the child without blocking and returns a boolean. Once it returns `#f`, `pty/exit-code` returns the exit code. Every `pty/*` function raises `no such handle` when the handle was never returned by `pty/spawn` or was already freed by `pty/close`, and raises `handle is busy` while a `pty/wait` on the same handle is in flight in another async task.

```sema
(define h (pty/spawn ["sleep" "0.1"]))
(pty/running? h)   ; #t
(pty/wait h)
(pty/running? h)   ; #f
(pty/close h)
```
