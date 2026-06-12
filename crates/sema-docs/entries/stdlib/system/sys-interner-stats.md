---
name: "sys/interner-stats"
module: "system"
section: "System Information"
returns: "map"
---

Return statistics about the global symbol/string interner as a map with `:count` (number of interned strings) and `:bytes` (total bytes they occupy). Useful for diagnosing interner growth.

```sema
(sys/interner-stats)  ; => {:count 1234 :bytes 56789}
```
