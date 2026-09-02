---
name: "pty/read"
module: "pty"
section: "Pseudo-Terminals"
params: [{ name: handle, type: int }]
returns: "string"
see_also: ["pty/write", "pty/wait", "pty/spawn"]
---

Drain and return everything the pty has emitted since the last call (non-blocking). Output may include terminal control sequences and CR/LF translation.

The function returns the buffered text as a string and clears the buffer, so each call returns only new output. It returns `""` when nothing new has arrived. Because the child writes to a terminal, the text contains what the child echoed, ANSI escape sequences, and `\r\n` line endings. Bytes that are not valid UTF-8 are replaced with the replacement character. Every `pty/*` function raises `no such handle` when the handle was never returned by `pty/spawn` or was already freed by `pty/close`, and raises `handle is busy` while a `pty/wait` on the same handle is in flight in another async task.

```sema
(define h (pty/spawn ["echo" "hello"]))
(pty/wait h)
(pty/read h)   ; "hello\r\n"
(pty/close h)
```
