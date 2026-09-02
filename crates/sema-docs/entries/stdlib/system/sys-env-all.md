---
name: "sys/env-all"
module: "system"
section: "Environment Variables"
syntax: "(sys/env-all)"
returns: "map"
see_also: ["env", "sys/set-env"]
---

Return all environment variables as a map.

```sema
(sys/env-all)   ; => {:HOME "/Users/ada" :PATH "..." ...}
```
