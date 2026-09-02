---
name: "context/all"
module: "context"
section: "Core Functions"
syntax: "(context/all)"
returns: "map"
see_also: ["context/get", "context/set", "context/clear"]
---

Get all context as a merged map.

```sema
(context/set :a 1)
(context/set :b 2)
(context/all)  ; => {:a 1 :b 2}
```
