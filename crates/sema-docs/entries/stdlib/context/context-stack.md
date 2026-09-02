---
name: "context/stack"
module: "context"
section: "Stacks"
params: [{ name: name, type: keyword, doc: "stack name" }]
returns: "list"
see_also: ["context/push", "context/pop"]
---

Get all values in a named stack as a list.

```sema
(context/stack :breadcrumbs)
; => ("login" "dashboard" "settings")
```
