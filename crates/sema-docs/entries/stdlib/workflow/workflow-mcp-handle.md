---
name: "workflow/mcp-handle"
module: "workflow"
section: "Dynamic Workflows"
---

The resolved MCP connection handle for a declared `:mcp` alias. `(workflow/mcp-handle alias)` takes a symbol or keyword and returns the opaque handle `workflow/run`'s implicit auth-resolution step connected for that alias, for use with `mcp/call`/`mcp/tools`/`mcp/tools->sema`. Only valid inside a workflow body, after resolution — `defworkflow` generates a `let` binding per declared `:mcp` alias that calls this for you, so most code never calls it directly. Errors (with a hint) if called outside a workflow run, for an alias not declared in the workflow's `:mcp` meta, or for an alias declared but not yet resolved.

```sema
(defworkflow triage
  "Triage new bugs into the Asana board."
  {:mcp {asana {:url "https://mcp.asana.com/mcp" :auth {:scopes ["default"]}}}}
  (phase "Triage")
  ;; `asana` is already bound to (workflow/mcp-handle 'asana) by defworkflow.
  (mcp/call asana "create_task" {"name" "Fix the thing"}))
```

See also: `defworkflow`, `workflow/run`, `mcp/call`.
