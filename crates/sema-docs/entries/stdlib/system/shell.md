---
name: "shell"
module: "system"
section: "Shell & Process Control"
syntax: "(shell cmd arg ... opts?)"
returns: "map"
---

Run a shell command. Returns a map with `:stdout`, `:stderr`, and `:exit-code`.

A single string runs through the system shell (`sh -c` on Unix, `cmd /C` on Windows); extra positional
strings run the program directly (no shell). An optional trailing options map
`{:cwd "path" :env {"KEY" "val"}}` pins the working directory and injects
environment variables (same shape as `proc/spawn`).

```sema
(shell "echo hello")
; => {:exit-code 0 :stderr "" :stdout "hello\n"}

(:stdout (shell "echo hello"))          ; => "hello\n"

(:stdout (shell "pwd" {:cwd "/tmp"}))   ; => "/tmp\n"
(:stdout (shell "echo $FOO" {:env {"FOO" "bar"}}))  ; => "bar\n"
```

Use `shell/quote` to safely interpolate an untrusted value into a single-string
command.
