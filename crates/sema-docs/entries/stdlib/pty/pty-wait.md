---
name: "pty/wait"
module: "pty"
section: "Pseudo-Terminals"
params: [{ name: handle, type: int }]
returns: "int"
see_also: ["pty/exit-code", "pty/running?", "pty/read", "pty/close"]
---

Block until the child exits and return its exit code. All output is buffered first, so a following `pty/read` returns the tail.

Calling `pty/wait` again on the same handle while the first call is still in flight (e.g. two `async/spawn` tasks waiting on the same child) queues rather than racing it — both calls resolve to the same exit code once it exits. Every other `pty/*` op on a handle that's mid-wait errors clearly instead of racing it.

The function returns the child's exit code as an integer. A second `pty/wait` on an already-exited child returns the same code again without blocking. It raises `no such handle` when the handle was already freed by `pty/close` or never came from `pty/spawn`. Inside an async task the wait is offloaded, so other tasks keep running.

```sema
(define h (pty/spawn ["echo" "hello"]))
(pty/wait h)
(pty/read h)   ; "hello\r\n"
(pty/close h)
```
