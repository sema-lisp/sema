---
name: "workflow/tool-result"
module: "workflow"
section: "Dynamic Workflows"
syntax: "(workflow/tool-result tool-name)"
returns: "nil"
see_also: ["workflow/tool-call", "workflow/step", "step"]
---

Journal a successful tool completion for the step currently executing.
`tool-name` is a keyword or string. The event records only the `"gated"`
sentinel and does not store the tool result.

The function returns `nil`. It is a no-op outside a `workflow/step`. Agent and
tool steps call it automatically after a successful tool invocation, so direct
use is only needed by custom workflow integrations.

```sema
(workflow/step "custom tool"
  (fn ()
    (workflow/tool-call "lookup" {:id 42})
    (def result (lookup 42))
    (workflow/tool-result "lookup")
    result))
```

See also: `workflow/tool-call`, `workflow/step`, `step`.
