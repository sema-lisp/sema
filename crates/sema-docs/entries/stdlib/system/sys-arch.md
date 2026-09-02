---
name: "sys/arch"
module: "system"
section: "System Information"
syntax: "(sys/arch)"
returns: "string"
see_also: ["sys/os", "sys/platform"]
---

Return the CPU architecture.

```sema
(sys/arch)   ; => "aarch64" / "x86_64"
```
