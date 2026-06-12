---
name: "conversation/model"
module: "conversation"
params: [{ name: conv, type: conversation }]
returns: "string"
---

Return the model name associated with the conversation.

```sema
(conversation/model conv)   ; => "claude-sonnet-4"
```
