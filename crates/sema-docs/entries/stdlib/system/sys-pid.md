---
name: "sys/pid"
module: "system"
section: "Process Information"
syntax: "(sys/pid)"
returns: "int"
see_also: ["sys/hostname", "sys/user"]
---

Return the current process ID.

```sema
(sys/pid)   ; => 12345
```
