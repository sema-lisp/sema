---
name: "sys/elapsed"
module: "system"
section: "Process Information"
syntax: "(sys/elapsed)"
returns: "int"
see_also: ["time-ms", "time/ms", "time"]
---

Return nanoseconds elapsed since the process started.

```sema
(sys/elapsed)   ; => 482937100
```
