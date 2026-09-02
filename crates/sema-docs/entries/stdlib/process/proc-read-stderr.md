---
name: "proc/read-stderr"
module: "process"
section: "Processes"
params: [{ name: handle, type: int }]
returns: "string"
see_also: ["proc/read-stdout", "proc/spawn", "proc/wait"]
---

Drain and return everything written to the process's stderr since the last call (non-blocking).

The function returns the buffered text as a string and clears the buffer, so each call returns only new output. It returns `""` when nothing new has arrived. Bytes that are not valid UTF-8 are replaced with the replacement character. Every `proc/*` function raises `no such handle` when the handle was never returned by `proc/spawn` or was already freed by `proc/close`, and raises `handle is busy` while a `proc/wait` on the same handle is in flight in another async task.

```sema
(define h (proc/spawn ["sh" "-c" "echo oops >&2"]))
(proc/wait h)
(proc/read-stderr h)   ; "oops\n"
(proc/close h)
```
