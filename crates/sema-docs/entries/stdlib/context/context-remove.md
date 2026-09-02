---
name: "context/remove"
module: "context"
section: "Core Functions"
params: [{ name: key, type: keyword }]
returns: "any or nil"
see_also: ["context/pull", "context/clear", "context/set"]
---

Remove a key from all context frames. Returns the removed value, or `nil`.

```sema
(context/set :temp "data")
(context/remove :temp)    ; => "data"
(context/remove :temp)    ; => nil (already gone)
```
