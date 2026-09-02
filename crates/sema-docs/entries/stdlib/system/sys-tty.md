---
name: "sys/tty"
module: "system"
section: "Process Information"
syntax: "(sys/tty)"
returns: "string or nil"
see_also: ["sys/interactive?", "sys/term-size"]
---

Return the TTY device path, or `nil` if not running in a terminal.

```sema
(sys/tty)   ; => "/dev/ttys003" or nil
```
