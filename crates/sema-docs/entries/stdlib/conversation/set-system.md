---
name: "conversation/set-system"
module: "conversation"
params: [{ name: conv, type: conversation }, { name: system, type: string }]
returns: "conversation"
---

Return a new conversation with its system message set to (or replaced by) the given string, placed first among the messages.

```sema
(conversation/set-system conv "You are a concise expert.")
```
