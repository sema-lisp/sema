---
name: "conversation/filter"
module: "conversation"
params: [{ name: conv, type: conversation }, { name: pred }]
returns: "conversation"
---

Return a new conversation keeping only messages for which `(pred msg)` is truthy. The predicate receives each message value.

```sema
(conversation/filter conv (fn [m] (= (message/role m) :user)))
```
