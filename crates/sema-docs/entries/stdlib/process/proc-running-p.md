---
name: "proc/running?"
module: "process"
section: "Processes"
params: [{ name: handle, type: int }]
returns: "bool"
see_also: ["proc/exit-code", "proc/wait", "proc/kill"]
---

Return `#t` if the process is still running, `#f` if it has exited.

It polls the child without blocking and returns a boolean. Once it returns `#f`, `proc/exit-code` returns the exit code. Every `proc/*` function raises `no such handle` when the handle was never returned by `proc/spawn` or was already freed by `proc/close`, and raises `handle is busy` while a `proc/wait` on the same handle is in flight in another async task.

```sema
(define h (proc/spawn ["sleep" "0.1"]))
(proc/running? h)   ; #t
(proc/wait h)
(proc/running? h)   ; #f
(proc/close h)
```
