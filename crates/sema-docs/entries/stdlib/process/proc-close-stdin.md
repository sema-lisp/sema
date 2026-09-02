---
name: "proc/close-stdin"
module: "process"
section: "Processes"
params: [{ name: handle, type: int }]
returns: "nil"
see_also: ["proc/write-stdin", "proc/wait", "proc/spawn"]
---

Close the process's stdin (sends EOF), so commands that read until EOF (e.g. `cat`) can finish.

It drops the write end of the child's stdin pipe, which the child observes as end of file. The function returns `nil`, and calling it a second time on the same handle is a no-op. After it runs, `proc/write-stdin` on the handle raises `stdin already closed`. Every `proc/*` function raises `no such handle` when the handle was never returned by `proc/spawn` or was already freed by `proc/close`, and raises `handle is busy` while a `proc/wait` on the same handle is in flight in another async task.

```sema
(define h (proc/spawn ["cat"]))
(proc/write-stdin h "hello\n")
(proc/close-stdin h)
(proc/wait h)
(proc/close h)
```
