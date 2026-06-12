---
name: "conversation/last-reply"
module: "conversation"
params: [{ name: conv, type: conversation }]
returns: "string"
---

Return the content of the most recent assistant message as a string. Errors if the conversation has no assistant reply.

```sema
(conversation/last-reply conv)
```
