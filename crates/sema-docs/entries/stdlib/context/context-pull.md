---
name: "context/pull"
module: "context"
section: "Core Functions"
---

Get a value and remove it in one step (identical to `context/remove`).

```sema
(context/set :token "abc")
(context/pull :token)     ; => "abc"
(context/has? :token)     ; => #f
```
