---
name: "time-ms"
module: "datetime"
section: "Current Time"
aliases: ["time/now-ms"]
syntax: "(time-ms)"
returns: "int"
see_also: ["time/now", "time/ms", "sys/elapsed"]
---

Return the current time as Unix milliseconds (integer). Defined in the system module but useful alongside datetime operations.

```sema
(time-ms)   ; => 1707955200123
```
