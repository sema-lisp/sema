---
name: "sys/temp-dir"
module: "system"
section: "Directory Paths"
syntax: "(sys/temp-dir)"
returns: "string"
see_also: ["sys/home-dir", "sys/cwd", "sys/config-dir"]
---

Return the system temporary directory.

```sema
(sys/temp-dir)   ; => "/tmp"
```
