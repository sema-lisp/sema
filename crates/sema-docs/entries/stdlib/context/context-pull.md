---
name: "context/pull"
module: "context"
section: "Core Functions"
params: [{ name: key, type: keyword }]
returns: "any or nil"
see_also: ["context/remove", "context/get"]
---

Get a value and remove it in one step (identical to `context/remove`).

```sema
(context/set :token "abc")
(context/pull :token)     ; => "abc"
(context/has? :token)     ; => #f
```
