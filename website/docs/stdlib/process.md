---
outline: [2, 3]
---

# Processes & PTYs

Spawn and drive child processes with streaming I/O. Unlike [`shell`](/docs/stdlib/system)
(which blocks and returns output only after the process exits), these hand you a
live handle you poll. All require the `PROCESS` capability (see [System](/docs/stdlib/system)).

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
`proc/running?`, `proc/kill`, `proc/close`.

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
