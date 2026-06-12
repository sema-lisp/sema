---
name: "conversation/system"
module: "conversation"
params: [{ name: conv, type: conversation }]
returns: "string"
---

Return the content of the conversation's system message, or nil if none is set.

```sema
(conversation/system conv)   ; => "You are a helpful assistant."
```
