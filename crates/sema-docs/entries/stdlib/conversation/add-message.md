---
name: "conversation/add-message"
module: "conversation"
params: [{ name: conv, type: conversation }, { name: role, type: keyword }, { name: content, type: string }]
returns: "conversation"
---

Append a message with the given role to the conversation without calling the model, returning a new conversation. Role must be `:system`, `:user`, `:assistant`, or `:tool`.

```sema
(conversation/add-message conv :assistant "Sure, I can help with that.")
```
