---
name: "workflow/tool-call"
module: "workflow"
section: "Dynamic Workflows"
---

Journal a tool call made by the agent currently executing. `(workflow/tool-call tool-name [args])` emits an `agent.tool_call` event attributed to the enclosing [`workflow/agent`], so the dashboard renders it as a tool twig in that agent's drill-in. `args` is an opaque/gated descriptor string (omit it for the `"gated"` sentinel — content is not captured). It is a no-op (returns `nil`) outside a `workflow/agent`. Use it to make a leaf's tool usage visible in the run journal.

```sema
(workflow/agent "assembler"
  (fn ()
    (workflow/tool-call "file/read" "drafts/intro.md")
    (workflow/tool-call "file/read" "drafts/scheduler.md")
    (assemble-index (checkpoint :drafts))))
```

See also: `workflow/agent`, `agent`, `pipeline`.
