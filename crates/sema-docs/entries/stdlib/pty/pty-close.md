---
name: "pty/close"
module: "pty"
section: "Pseudo-Terminals"
params: [{ name: handle, type: int }]
returns: "nil"
see_also: ["pty/kill", "pty/wait", "pty/spawn"]
---

Kill the child if needed and free the pty handle from the registry.

Call it once per handle when the child is no longer needed, whether or not it has exited; a running child is killed first. The function returns `nil`. A handle that is unknown or already closed is a silent no-op, so cleanup code can call it unconditionally. It raises `handle is busy` while a `pty/wait` on the same handle is in flight in another async task. After `pty/close`, every other `pty/*` function raises `no such handle` for that handle.

```sema
(define h (pty/spawn ["echo" "hello"]))
(pty/wait h)
(pty/read h)   ; "hello\r\n"
(pty/close h)
```
