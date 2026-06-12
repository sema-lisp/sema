---
name: "conversation/messages"
module: "conversation"
params: [{ name: conv, type: conversation }]
returns: "list"
---

Return the conversation's messages as a list of message values.

```sema
(conversation/messages conv)
```
