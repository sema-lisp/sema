---
name: "context/pop"
module: "context"
section: "Stacks"
---

Remove and return the last value from a stack. Returns `nil` if the stack is empty.

```sema
(context/pop :breadcrumbs)  ; => "settings"
(context/stack :breadcrumbs)
; => ("login" "dashboard")
```
