---
name: "conversation/map"
module: "conversation"
params: [{ name: conv, type: conversation }, { name: f }]
returns: "list"
---

Apply `f` to each message in the conversation and return the list of results. Unlike `conversation/filter`, this returns a plain list rather than a conversation.

```sema
(conversation/map conv message/content)
```
