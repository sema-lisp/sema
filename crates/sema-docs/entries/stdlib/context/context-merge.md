---
name: "context/merge"
module: "context"
section: "Core Functions"
---

Merge a map of key-value pairs into the current context.

```sema
(context/merge {:trace-id "abc" :env "production" :version "1.0"})
(context/get :env)  ; => "production"
```
