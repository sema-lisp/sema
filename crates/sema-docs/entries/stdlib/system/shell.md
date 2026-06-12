---
name: "shell"
module: "system"
section: "Shell & Process Control"
---

Run a shell command and return its stdout as a string.

```sema
(shell "ls -la")       ; => "total 42\n..."
(shell "echo hello")   ; => "hello\n"
```
