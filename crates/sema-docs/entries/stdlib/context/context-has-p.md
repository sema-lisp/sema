---
name: "context/has?"
module: "context"
section: "Core Functions"
params: [{ name: key, type: keyword }]
returns: "bool"
see_also: ["context/get", "context/set", "context/has-hidden?"]
---

Check if a key exists in the context.

```sema
(context/has? :trace-id)  ; => #t
(context/has? :missing)   ; => #f
```
