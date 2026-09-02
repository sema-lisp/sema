---
name: "proc/spawn"
module: "process"
section: "Processes"
params: [{ name: argv, type: "list | vector" }, { name: opts, type: map, doc: "optional; keys :cwd (string) and :env (map of string to string)" }]
returns: "int"
see_also: ["proc/run", "proc/read-stdout", "proc/write-stdin", "proc/wait", "proc/close", "shell"]
---

Spawn a subprocess and return an integer handle. `(proc/spawn ["cargo" "test"])` or `(proc/spawn argv {:cwd "path" :env {"KEY" "val"}})`. Unlike `shell`, stdout/stderr stream live into buffers you poll with `proc/read-stdout`/`proc/read-stderr`.

`argv` is a list or vector of strings; the first element is the program and the rest are its arguments, with no shell involved. The optional `opts` map accepts `:cwd` (working directory) and `:env` (extra environment variables). The child gets piped stdin, stdout, and stderr and its own process group. The function returns an integer handle that every other `proc/*` function takes as its first argument. It raises a type error when `argv` is not a list of strings, and an I/O error when the program cannot be started (for example, when it is not on `PATH`). The handle stays valid until `proc/close` frees it.

```sema
(define h (proc/spawn ["cat"] {:cwd "/tmp" :env {"LANG" "C"}}))
(proc/write-stdin h "hello\n")
(proc/close-stdin h)
(proc/wait h)          ; 0
(proc/read-stdout h)   ; "hello\n"
(proc/close h)
```
