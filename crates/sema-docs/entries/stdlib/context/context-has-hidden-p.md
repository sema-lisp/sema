---
name: "context/has-hidden?"
module: "context"
section: "Hidden Context"
params: [{ name: key }]
returns: "bool"
---

Return `#t` if a hidden context entry exists for `key`, else `#f`.

```sema
(context/has-hidden? :api-key)  ; => #t
```
