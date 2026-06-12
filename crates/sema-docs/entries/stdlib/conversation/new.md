---
name: "conversation/new"
module: "conversation"
params: [{ name: opts, type: map }]
returns: "conversation"
---

Create a new, empty conversation. The optional map sets the `:model` and any other keys are stored as string metadata.

```sema
(conversation/new {:model "claude-sonnet-4" :user "alice"})
```
