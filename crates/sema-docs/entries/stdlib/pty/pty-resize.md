---
name: "pty/resize"
module: "pty"
section: "Pseudo-Terminals"
params: [{ name: handle, type: int }, { name: rows, type: int }, { name: cols, type: int }]
returns: "nil"
see_also: ["pty/spawn", "pty/write", "sys/term-size"]
---

Resize the pty window to `rows`×`cols`, delivering SIGWINCH to the child so full-screen apps redraw. `(pty/resize h 50 200)`.

`rows` and `cols` are integers; values below 1 are raised to 1 and values above 65535 are lowered to 65535. The kernel delivers SIGWINCH to the child, so a full-screen program redraws at the new size. The function returns `nil`. It raises a type error when `rows` or `cols` is not an integer, and an error when the pty rejects the new size. Every `pty/*` function raises `no such handle` when the handle was never returned by `pty/spawn` or was already freed by `pty/close`, and raises `handle is busy` while a `pty/wait` on the same handle is in flight in another async task.

```sema
(define h (pty/spawn ["sh"]))
(pty/resize h 50 200)
(pty/write h "stty size\n")
(pty/write h "exit\n")
(pty/wait h)
(pty/read h)   ; contains "50 200"
(pty/close h)
```
