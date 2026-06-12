---
name: "context/get-hidden"
module: "context"
section: "Hidden Context"
params: [{ name: key }]
---

Look up a value previously stored with `context/set-hidden`. Returns nil if the key is absent.
Hidden values are invisible to the regular `context/get`.

```sema
(context/get-hidden :api-key)  ; => "sk-secret-123"
(context/get :api-key)         ; => nil (not visible in regular context)
```
