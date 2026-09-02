---
name: "proc/close"
module: "process"
section: "Processes"
params: [{ name: handle, type: int }]
returns: "nil"
see_also: ["proc/kill", "proc/wait", "proc/spawn"]
---

Kill the process if needed, reap it, and free its handle from the registry.

Call it once per handle when the process is no longer needed, whether or not it has exited; a running child is killed first. The function returns `nil`. A handle that is unknown or already closed is a silent no-op, so cleanup code can call it unconditionally. It raises `handle is busy` while a `proc/wait` on the same handle is in flight in another async task. After `proc/close`, every other `proc/*` function raises `no such handle` for that handle.

```sema
(define h (proc/spawn ["echo" "hello"]))
(proc/wait h)
(proc/read-stdout h)   ; "hello\n"
(proc/close h)
```
