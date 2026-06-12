---
name: "agent/model"
module: "agent"
params: [{ name: agent, type: agent }]
returns: "string"
---

Return the model name the agent uses.

```sema
(agent/model my-agent)   ; => "claude-sonnet-4"
```
