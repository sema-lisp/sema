---
name: "sys/args"
module: "system"
section: "System Information"
syntax: "(sys/args)"
returns: "list"
see_also: ["env", "exit"]
---

Return the command-line arguments as a list.

```sema
(sys/args)   ; => ("sema" "script.sema" "--flag")
```
