---
name: "context/remove"
module: "context"
section: "Core Functions"
---

Remove a key from all context frames. Returns the removed value, or `nil`.

```sema
(context/set :temp "data")
(context/remove :temp)    ; => "data"
(context/remove :temp)    ; => nil (already gone)
```
