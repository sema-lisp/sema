---
name: "conversation/say-as"
module: "conversation"
params: [{ name: conv, type: conversation }, { name: system }, { name: message, type: string }, { name: opts, type: map }]
returns: "conversation"
---

Like `conversation/say` but uses a one-off system prompt (a string or prompt value) for this turn only; the conversation's existing system message is preserved in the returned conversation. The optional opts map accepts `:temperature` and `:max-tokens`.

```sema
(conversation/say-as conv "You are a terse pirate." "Describe the weather.")
```
