---
name: "conversation/say"
module: "conversation"
params: [{ name: conv, type: conversation }, { name: message, type: string }, { name: opts, type: map }]
returns: "conversation"
---

Send a user message to the conversation's model and return a new conversation with the user message and the assistant's reply appended. The optional opts map accepts `:temperature`, `:max-tokens`, and `:system`.

```sema
(conversation/say conv "What's the capital of France?" {:temperature 0.2})
```
