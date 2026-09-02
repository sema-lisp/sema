---
name: "pty/kill"
module: "pty"
section: "Pseudo-Terminals"
params: [{ name: handle, type: int }]
returns: "nil"
see_also: ["pty/close", "pty/wait", "pty/running?"]
---

Kill the pty's child process. Safe to call if it has already exited.

It sends SIGKILL to the child and returns `nil` without waiting for it to exit; call `pty/wait` afterwards to reap it. A child that has already exited is left as is and no error is raised. The handle stays registered, so `pty/read` still returns the output written before the kill. Every `pty/*` function raises `no such handle` when the handle was never returned by `pty/spawn` or was already freed by `pty/close`, and raises `handle is busy` while a `pty/wait` on the same handle is in flight in another async task.

```sema
(define h (pty/spawn ["sleep" "10"]))
(pty/kill h)
(pty/wait h)
(pty/close h)
```
