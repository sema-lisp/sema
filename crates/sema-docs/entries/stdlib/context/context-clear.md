---
name: "context/clear"
module: "context"
section: "Core Functions"
syntax: "(context/clear)"
returns: "nil"
see_also: ["context/all", "context/remove", "context/with"]
---

Clear all context, resetting to an empty state.

```sema
(context/clear)
(context/all)  ; => {}
```
