---
name: "agent/max-turns"
module: "agent"
params: [{ name: agent, type: agent }]
returns: "int"
---

Return the maximum number of tool-execution rounds the agent will run before stopping.

```sema
(agent/max-turns my-agent)   ; => 10
```
