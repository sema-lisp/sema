---
name: "shell"
module: "system"
section: "Shell & Process Control"
syntax: "(shell cmd arg ...)"
returns: "map"
---

Run a shell command. Returns a map with `:stdout`, `:stderr`, and `:exit-code`.

```sema
(shell "echo hello")
; => {:exit-code 0 :stderr "" :stdout "hello\n"}

(:stdout (shell "echo hello"))   ; => "hello\n"
```
