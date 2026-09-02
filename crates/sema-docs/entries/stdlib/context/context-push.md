---
name: "context/push"
module: "context"
section: "Stacks"
params: [{ name: name, type: keyword, doc: "stack name" }, { name: value, type: any }]
returns: "nil"
see_also: ["context/pop", "context/stack"]
---

Append a value to a named stack.

```sema
(context/push :breadcrumbs "login")
(context/push :breadcrumbs "dashboard")
(context/push :breadcrumbs "settings")
```
