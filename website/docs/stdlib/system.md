---
outline: [2, 3]
---

# System

::: tip Sandbox capability
Several `sys/*` functions are gated by sandbox capabilities: environment access (`env`, `sys/env-all`, `sys/set-env`) requires `ENV_READ` or `ENV_WRITE`, and process operations (`shell`, `sys/which`, signal hooks, `exit`) require `PROCESS`. They run unrestricted under `sema` by default but are restricted in sandboxed environments (e.g., the WASM playground). A sandboxed script that attempts to use them without the capability will receive an error.
:::

## Environment Variables

### `env`

Get the value of an environment variable. Returns `nil` if not set.

```sema
(env "HOME")       ; => "/Users/ada"
(env "PATH")       ; => "/usr/bin:/bin:..."
(env "MISSING")    ; => nil
```

### `sys/env-all`

Return all environment variables as a map.

```sema
(sys/env-all)   ; => {:HOME "/Users/ada" :PATH "..." ...}
```

### `sys/set-env`

Set an environment variable for the current process.

```sema
(sys/set-env "KEY" "value")
(env "KEY")   ; => "value"
```

## System Information

### `sys/args`

Return the command-line arguments as a list.

```sema
(sys/args)   ; => ("sema" "script.sema" "--flag")
```

### `sys/cwd`

Return the current working directory.

```sema
(sys/cwd)   ; => "/current/dir"
```

### `sys/platform`

Return a normalized platform name — always one of the closed set `"macos"`,
`"linux"`, `"windows"`, or `"unknown"`. Anything unrecognized collapses to
`"unknown"`, so it is safe to branch on exhaustively. For the raw, open-ended OS
name, use `sys/os`.

```sema
(sys/platform)   ; => "macos" / "linux" / "windows"
```

### `sys/os`

Return the raw operating system name from the Rust target
(`std::env::consts::OS`). This is an open set — besides `"macos"`, `"linux"`, and
`"windows"` it can also report `"ios"`, `"android"`, `"freebsd"`, and others. Use
`sys/platform` when you want a normalized, closed set.

```sema
(sys/os)   ; => "macos"
```

### `sys/arch`

Return the CPU architecture.

```sema
(sys/arch)   ; => "aarch64" / "x86_64"
```

## Process Information

### `sys/pid`

Return the current process ID.

```sema
(sys/pid)   ; => 12345
```

### `sys/tty`

Return the TTY device path, or `nil` if not running in a terminal.

```sema
(sys/tty)   ; => "/dev/ttys003" or nil
```

### `sys/which`

Find the full path to an executable, or `nil` if not found.

```sema
(sys/which "cargo")   ; => "/Users/ada/.cargo/bin/cargo"
(sys/which "nonexistent")  ; => nil
```

### `sys/elapsed`

Return nanoseconds elapsed since the process started.

```sema
(sys/elapsed)   ; => 482937100
```

## Session Information

### `sys/interactive?`

Test if stdin is a TTY (i.e., running interactively).

```sema
(sys/interactive?)   ; => #t in REPL, #f in scripts
```

### `sys/hostname`

Return the system hostname.

```sema
(sys/hostname)   ; => "my-machine"
```

### `sys/user`

Return the current username.

```sema
(sys/user)   ; => "ada"
```

## Directory Paths

### `sys/home-dir`

Return the user's home directory.

```sema
(sys/home-dir)   ; => "/Users/ada"
```

### `sys/temp-dir`

Return the system temporary directory.

```sema
(sys/temp-dir)   ; => "/tmp"
```

## Terminal

### `sys/term-size`

Return the terminal's current size as a map `{:rows N :cols M}`, or `nil` when no controlling TTY is attached (e.g., when stdout is redirected to a file). Queries `ioctl(TIOCGWINSZ)` against stdout, then stderr, then stdin.

```sema
(sys/term-size)
;; => {:rows 47 :cols 180}
```

Pair with `sys/on-signal :winch` to redraw on terminal resize:

```sema
(define (redraw size)
  ;; ... layout for size ...
  )

(redraw (sys/term-size))
(sys/on-signal :winch (fn () (redraw (sys/term-size))))
```

::: warning Unix only
Returns `nil` on Windows and any non-Unix target.
:::

## Signals

Async-signal-safe handlers backed by atomic flags. Signal handlers themselves only flip a flag — your callbacks run later, in the main thread, when you call `sys/check-signals`. This keeps the single-threaded `Rc`-based runtime intact.

::: warning Unix only
Signal hooks are no-ops on Windows.
:::

### `sys/on-signal`

Register a callback for a signal. Multiple callbacks per signal are supported; they fire in registration order.

Supported signals:

| Keyword  | Signal     | Typical use                          |
|----------|------------|--------------------------------------|
| `:winch` | `SIGWINCH` | Terminal resize — redraw the UI      |
| `:int`   | `SIGINT`   | Ctrl-C — clean shutdown              |
| `:term`  | `SIGTERM`  | Termination request — clean shutdown |

```sema
(sys/on-signal :int (fn ()
  (println "interrupted, cleaning up")
  (exit 0)))
```

### `sys/check-signals`

Dispatch any pending signal callbacks. Call this from your event loop (typically right after `io/read-key` / `io/read-key-timeout` returns) so handlers run in a predictable place rather than asynchronously interrupting Sema code.

```sema
(let loop ()
  (sys/check-signals)
  (let ((key (io/read-key-timeout 50)))
    (when key (handle-key key))
    (loop)))
```

If no signals are pending, this is essentially free — it just checks three atomic booleans.

## Shell & Process Control

### `shell`

Run a shell command. Returns a map with `:stdout`, `:stderr`, and `:exit-code`. A single-string command runs through the system shell (`sh -c` / `cmd /C`); passing extra arguments runs the command directly, without shell parsing. Requires the `PROCESS` capability.

```sema
(shell "echo hello")          ; => {:stdout "hello\n" :stderr "" :exit-code 0}
(:stdout (shell "ls -la"))    ; => "total 42\n..."
(:exit-code (shell "false"))  ; => 1
```

### `exit`

Exit the process with a given status code.

```sema
(exit 0)   ; exit successfully
(exit 1)   ; exit with error
```
