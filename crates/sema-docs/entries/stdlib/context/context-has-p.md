---
name: "context/has?"
module: "context"
section: "Core Functions"
---

Check if a key exists in the context.

```sema
(context/has? :trace-id)  ; => #t
(context/has? :missing)   ; => #f
```
