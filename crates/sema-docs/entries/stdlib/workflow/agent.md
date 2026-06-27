---
name: "agent"
module: "workflow"
section: "Dynamic Workflows"
syntax: "(agent opts)"
---

Build an anonymous, reusable **actor** value from an options map: a configured brain with
a system prompt, tools, model, and turn budget that owns its own tool loop. This is the
plain constructor; the named form is `defagent`. Run one with `agent/run`, or
hand it to a workflow `step` via `:agent` to run it as a journaled step.

`opts` keys (all optional): `:system` (system prompt string), `:tools` (a list/vector of
`deftool` values), `:model` (provider model id; omit for the default), `:max-turns` (tool-loop
cap, default `10`), and `:name` (empty for an anonymous agent).

```sema
(deftool get-weather "Get weather" {:city {:type :string}}
  (lambda (city) (format "{\"temp\": 22}")))

;; anonymous actor — omit :model to use the default provider/model
(define bot (agent {:tools [get-weather]}))
(agent/run bot "Weather in Oslo?")            ; multi-turn tool loop

;; the same brain, run as a journaled workflow step
(step "Weather in Oslo?" {:agent bot})
```

See also: `defagent`, `agent/run`, `agent/name`, `step`.
