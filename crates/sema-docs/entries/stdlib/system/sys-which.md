---
name: "sys/which"
module: "system"
section: "Process Information"
params: [{ name: name, type: string, doc: "executable name" }]
returns: "string or nil"
see_also: ["shell", "proc/run"]
---

Find the full path to an executable, or `nil` if not found.

```sema
(sys/which "cargo")   ; => "/Users/ada/.cargo/bin/cargo"
(sys/which "nonexistent")  ; => nil
```
