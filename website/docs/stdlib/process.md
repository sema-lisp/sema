---
outline: [2, 3]
---

# Processes & PTYs

Spawn and drive child processes with streaming I/O. Unlike [`shell`](/docs/stdlib/system)
(which blocks and returns output only after the process exits), these hand you a
live handle you poll. All require the `process` capability in a sandboxed run.
They work without extra configuration in Sema's default mode and return
`PermissionDenied` when `process` is denied. See the
[CLI sandbox documentation](/docs/cli#sandbox).

## Streaming processes

`proc/*` streams a child's output into pollable buffers, so you can show output
as it happens.

```sema
(define p (proc/spawn ["cargo" "test"] {:cwd "."}))
(let loop ()
  (let ((out (proc/read-stdout p)))
    (when (not (= out "")) (io/print-error out))
    (when (proc/running? p) (sleep 50) (loop))))
(define code (proc/wait p))           ; exit code; flushes the tail first
(proc/close p)                        ; free the handle
```

Full set: `proc/spawn`, `proc/read-stdout`, `proc/read-stderr`,
`proc/write-stdin`, `proc/close-stdin`, `proc/wait`, `proc/exit-code`,
`proc/running?`, `proc/kill`, `proc/close`, and `proc/run` (below).

## Handing over the terminal

`proc/run` runs a child on the *parent's* terminal and blocks until it exits.
The child inherits stdin, stdout, and stderr unchanged and stays in the
foreground process group, so it can read the keyboard and draw on the screen —
this is how a terminal app shells out to `$EDITOR` or a pager.

```sema
(define code (proc/run [(or (env "EDITOR") "vi") "notes.md"]))
(proc/run ["less" "log.txt"] {:cwd "/tmp" :env {"LESS" "-R"}})
```

Inside [`io/with-raw-mode`](/docs/stdlib/terminal) the child gets a terminal in
cooked mode (line editing and echo on) and raw mode is restored when it exits.
Leave the alternate screen first if the child draws its own full-screen UI:

```sema
(term/leave-alt-screen)
(proc/run ["nvim" path])
(term/enter-alt-screen)
```

`proc/run` blocks the whole VM until the child exits, by design: the child owns
the screen and the keyboard, so no other task may run and write to either. It
errors when stdin is not a terminal (a pipe, `sema mcp`, a notebook cell), where
inheriting the file descriptors would corrupt the host's I/O stream — use
`shell` or `proc/spawn` there.

## Pseudo-terminals

Like `proc/*`, but the child runs under a real PTY, so programs that probe
`isatty` (REPLs, editors, `top`, color-aware tools) behave normally.

```sema
(define t (pty/spawn ["bash"] {:rows 40 :cols 120}))
(pty/write t "ls -la\n")
(sleep 100)
(io/print-error (pty/read t))         ; output incl. control sequences
(pty/resize t 50 200)                 ; delivers SIGWINCH
(pty/kill t)
(pty/close t)
```

Full set: `pty/spawn`, `pty/read`, `pty/write`, `pty/resize`, `pty/wait`,
`pty/exit-code`, `pty/running?`, `pty/kill`, `pty/close`.
