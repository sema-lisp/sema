---
name: "sys/tty"
module: "system"
section: "Process Information"
---

Return the TTY device path, or `nil` if not running in a terminal.

```sema
(sys/tty)   ; => "/dev/ttys003" or nil
```
