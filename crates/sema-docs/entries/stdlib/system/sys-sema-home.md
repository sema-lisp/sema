---
name: "sys/sema-home"
module: "system"
section: "System Information"
returns: "string"
syntax: "(sys/sema-home)"
see_also: ["sys/config-dir", "sys/home-dir"]
---

Return the path to the Sema home directory — where Sema stores its configuration and runtime data.

```sema
(sys/sema-home)  ; => "/Users/you/.sema"
```
