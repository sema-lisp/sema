---
name: "pty/exit-code"
module: "pty"
section: "Pseudo-Terminals"
params: [{ name: handle, type: int }]
returns: "int or nil"
see_also: ["pty/wait", "pty/running?", "pty/kill"]
---

Return the child's exit code if it has exited, or `nil` if it is still running (non-blocking).

It polls the child without blocking, unlike `pty/wait`. The function returns the exit code as an integer once the child has exited, and `nil` while it is still running. Every `pty/*` function raises `no such handle` when the handle was never returned by `pty/spawn` or was already freed by `pty/close`, and raises `handle is busy` while a `pty/wait` on the same handle is in flight in another async task.

```sema
(define h (pty/spawn ["sleep" "0.1"]))
(pty/exit-code h)   ; nil while running
(pty/wait h)
(pty/exit-code h)   ; 0
(pty/close h)
```
