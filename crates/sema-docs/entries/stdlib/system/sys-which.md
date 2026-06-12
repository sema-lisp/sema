---
name: "sys/which"
module: "system"
section: "Process Information"
---

Find the full path to an executable, or `nil` if not found.

```sema
(sys/which "cargo")   ; => "/Users/ada/.cargo/bin/cargo"
(sys/which "nonexistent")  ; => nil
```
