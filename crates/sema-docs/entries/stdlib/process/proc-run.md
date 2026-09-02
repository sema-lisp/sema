---
name: "proc/run"
module: "process"
section: "Processes"
params: [{ name: argv, type: "list | vector" }, { name: opts, type: map, doc: "optional; keys :cwd and :env" }]
returns: "int"
see_also: ["proc/spawn", "shell", "io/with-raw-mode"]
---

Run a child on the parent's terminal and block until it exits, returning its exit code (`-1` if killed by a signal). The child inherits stdin, stdout, and stderr unchanged and stays in the parent's foreground process group, so it can read the keyboard and draw on the screen: this is the primitive for handing the terminal to `$EDITOR`, a pager, or any other interactive program.

```sema
(proc/run ["nvim" "notes.md"])
(proc/run ["less" "log.txt"] {:cwd "/tmp" :env {"LESS" "-R"}})
```

The options map is the same as `shell` and `proc/spawn` (`:cwd`, `:env`).

Use it instead of the other three process primitives when the child needs the real terminal: `shell` captures output into pipes, `proc/spawn` streams it into buffers you poll, and `pty/spawn` gives the child a *new* pty. None of those let an interactive child see the user's terminal.

Inside `io/with-raw-mode` the child is handed a terminal in cooked mode (line editing and echo on), and raw mode is restored when it exits. Leave the alternate screen first if the child draws its own full-screen UI:

```sema
(io/with-raw-mode
  (term/with-alt-screen
    ...
    (term/leave-alt-screen)
    (proc/run [(or (env "EDITOR") "vi") path])
    (term/enter-alt-screen)
    ...))
```

`proc/run` blocks the whole VM until the child exits, by design: the child owns the screen and the keyboard, so no other task may run and write to either. It errors when stdin is not a terminal (a pipe, `sema mcp`, a notebook cell) — inheriting the file descriptors there would corrupt the host's I/O stream. Use `shell` or `proc/spawn` in those contexts.
