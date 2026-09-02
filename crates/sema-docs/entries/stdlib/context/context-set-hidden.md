---
name: "context/set-hidden"
module: "context"
section: "Hidden Context"
params: [{ name: key }, { name: value }]
returns: "nil"
see_also: ["context/get-hidden", "context/has-hidden?", "context/set"]
---

Store a value in the hidden context under `key`. Hidden entries are not returned by `context/all`
or `context/get`, making them useful for secrets or internal scoped state.

```sema
(context/set-hidden :api-key "sk-secret-123")
```
