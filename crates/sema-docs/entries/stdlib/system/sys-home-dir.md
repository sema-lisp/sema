---
name: "sys/home-dir"
module: "system"
section: "Directory Paths"
syntax: "(sys/home-dir)"
returns: "string"
see_also: ["sys/config-dir", "sys/cwd", "sys/temp-dir"]
---

Return the user's home directory.

```sema
(sys/home-dir)   ; => "/Users/ada"
```
