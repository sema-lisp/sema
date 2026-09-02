---
name: "sys/hostname"
module: "system"
section: "Session Information"
syntax: "(sys/hostname)"
returns: "string"
see_also: ["sys/user", "sys/pid", "sys/os"]
---

Return the system hostname.

```sema
(sys/hostname)   ; => "my-machine"
```
