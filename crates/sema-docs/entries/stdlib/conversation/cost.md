---
name: "conversation/cost"
module: "conversation"
params: [{ name: conv, type: conversation }]
returns: "float"
---

Estimate the cost in USD of the conversation, treating its estimated tokens (~4 chars per token) as input tokens for the conversation's model. Returns nil if pricing for the model is unknown.

```sema
(conversation/cost conv)   ; => 0.0014
```
