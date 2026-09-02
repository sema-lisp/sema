---
name: "proc/write-stdin"
module: "process"
section: "Processes"
params: [{ name: handle, type: int }, { name: text, type: string }]
returns: "nil"
see_also: ["proc/close-stdin", "proc/spawn", "proc/read-stdout"]
---

Write a string to the process's stdin. Use `proc/close-stdin` to send EOF when done.

`text` is written and flushed in one call, so the child sees it immediately; include a trailing newline when the child reads lines. The function returns `nil`. It raises `stdin already closed` after `proc/close-stdin`, a type error when `text` is not a string, and an I/O error when the child has exited and closed its end of the pipe. Every `proc/*` function raises `no such handle` when the handle was never returned by `proc/spawn` or was already freed by `proc/close`, and raises `handle is busy` while a `proc/wait` on the same handle is in flight in another async task.

```sema
(define h (proc/spawn ["cat"]))
(proc/write-stdin h "hello\n")
(proc/close-stdin h)
(proc/wait h)
(proc/read-stdout h)   ; "hello\n"
(proc/close h)
```
