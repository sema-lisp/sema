---
name: "proc/kill"
module: "process"
section: "Processes"
params: [{ name: handle, type: int }]
returns: "nil"
see_also: ["proc/close", "proc/wait", "proc/running?"]
---

Kill the process (SIGKILL). Safe to call if it has already exited.

It sends SIGKILL to the child and returns `nil` without waiting for it to exit; call `proc/wait` afterwards to reap it. A child that has already exited is left as is and no error is raised. The handle stays registered, so `proc/read-stdout` still returns the output written before the kill. Every `proc/*` function raises `no such handle` when the handle was never returned by `proc/spawn` or was already freed by `proc/close`, and raises `handle is busy` while a `proc/wait` on the same handle is in flight in another async task.

```sema
(define h (proc/spawn ["sleep" "10"]))
(proc/kill h)
(proc/wait h)   ; -1
(proc/close h)
```
