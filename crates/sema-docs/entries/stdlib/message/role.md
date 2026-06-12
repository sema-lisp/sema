---
name: "message/role"
module: "message"
params: [{ name: msg, type: message }]
returns: "keyword"
---

Return the role of a message as a keyword (`:system`, `:user`, `:assistant`, or `:tool`).

```sema
(message/role msg)   ; => :user
```
