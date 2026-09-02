---
name: "proc/wait"
module: "process"
section: "Processes"
params: [{ name: handle, type: int }]
returns: "int"
see_also: ["proc/exit-code", "proc/running?", "proc/read-stdout", "proc/close"]
---

Block until the process exits and return its exit code (`-1` if killed by a signal). Reader threads finish flushing first, so a subsequent `proc/read-stdout` returns the tail.

Calling `proc/wait` again on the same handle while the first call is still in flight (e.g. two `async/spawn` tasks waiting on the same process) queues rather than racing the child — both calls resolve to the same exit code once it exits. Every other `proc/*` op on a handle that's mid-wait errors clearly instead of racing it.

The function returns the exit code as an integer, or `-1` when a signal terminated the child. A second `proc/wait` on an already-exited child returns the same code again without blocking. It raises `no such handle` when the handle was already freed by `proc/close` or never came from `proc/spawn`. Inside an async task the wait is offloaded, so other tasks keep running.

```sema
(define h (proc/spawn ["echo" "hello"]))
(proc/wait h)
(proc/read-stdout h)   ; "hello\n"
(proc/close h)
```
