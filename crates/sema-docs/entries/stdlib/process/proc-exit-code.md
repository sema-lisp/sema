---
name: "proc/exit-code"
module: "process"
section: "Processes"
params: [{ name: handle, type: int }]
returns: "int or nil"
see_also: ["proc/wait", "proc/running?", "proc/kill"]
---

Return the process's exit code if it has exited, or `nil` if it is still running (non-blocking).

It polls the child without blocking, unlike `proc/wait`. The function returns the exit code as an integer once the child has exited (`-1` when a signal terminated it), and `nil` while it is still running. Every `proc/*` function raises `no such handle` when the handle was never returned by `proc/spawn` or was already freed by `proc/close`, and raises `handle is busy` while a `proc/wait` on the same handle is in flight in another async task.

```sema
(define h (proc/spawn ["sleep" "0.1"]))
(proc/exit-code h)   ; nil while running
(proc/wait h)
(proc/exit-code h)   ; 0
(proc/close h)
```
