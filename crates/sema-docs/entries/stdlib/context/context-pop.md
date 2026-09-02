---
name: "context/pop"
module: "context"
section: "Stacks"
params: [{ name: name, type: keyword, doc: "stack name" }]
returns: "any or nil"
see_also: ["context/push", "context/stack"]
---

Remove and return the last value from a stack. Returns `nil` if the stack is empty.

```sema
(context/pop :breadcrumbs)  ; => "settings"
(context/stack :breadcrumbs)
; => ("login" "dashboard")
```
