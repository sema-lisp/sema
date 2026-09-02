---
name: "pty/write"
module: "pty"
section: "Pseudo-Terminals"
params: [{ name: handle, type: int }, { name: text, type: string }]
returns: "nil"
see_also: ["pty/read", "pty/spawn", "pty/wait"]
---

Write a string to the pty (as if typed at the terminal). Include a trailing newline to submit a line.

The text goes through the terminal's line discipline, so the child sees it as keyboard input and the terminal echoes it back into the output buffer. Control characters work as they would at a keyboard; for example `"\u0003"` sends Ctrl-C. The function writes and flushes in one call and returns `nil`. It raises a type error when `text` is not a string, and an I/O error when the child has exited and the pty is closed. Every `pty/*` function raises `no such handle` when the handle was never returned by `pty/spawn` or was already freed by `pty/close`, and raises `handle is busy` while a `pty/wait` on the same handle is in flight in another async task.

```sema
(define h (pty/spawn ["cat"]))
(pty/write h "hello\n")
(pty/read h)   ; "hello\r\nhello\r\n" once the child has echoed it
(pty/kill h)
(pty/close h)
```
