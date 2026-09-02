---
name: "pty/spawn"
module: "pty"
section: "Pseudo-Terminals"
params: [{ name: argv, type: "list | vector" }, { name: opts, type: map, doc: "optional; keys :rows (default 24), :cols (default 80), :cwd, and :env" }]
returns: "int"
see_also: ["pty/read", "pty/write", "pty/resize", "pty/wait", "pty/close", "proc/spawn"]
---

Spawn a command under a pseudo-terminal and return an integer handle. `(pty/spawn ["bash"] {:rows 40 :cols 120 :cwd "path" :env {...}})`. The child sees a real TTY (isatty is true), so REPLs, editors, and color-aware tools behave normally. Output (stdout+stderr merged) streams into a buffer you drain with `pty/read`.

`argv` is a non-empty list or vector of strings; the first element is the program and the rest are its arguments. The optional `opts` map accepts `:rows` and `:cols` for the initial window size (defaults 24 by 80), `:cwd` for the working directory, and `:env` for extra environment variables. The function returns an integer handle that every other `pty/*` function takes as its first argument. It raises a type error when `argv` is not a list of strings, `argv must be non-empty` for an empty list, and an error when the pty cannot be opened or the program cannot be started. The handle stays valid until `pty/close` frees it.

```sema
(define h (pty/spawn ["sh"] {:rows 24 :cols 80}))
(pty/write h "echo hello\n")
(pty/write h "exit\n")
(pty/wait h)
(pty/read h)   ; the echoed command line and "hello"
(pty/close h)
```
