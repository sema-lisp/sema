# Sema Language Friction Log (micro/confidence)

While dogfooding the Sema language to implement the `micro/confidence` package (#52), I encountered a few areas where the primitives felt awkward or missing:

1. **Recipe-level custom events are NOT expressible today:**
   The recipe-level events proposed in issue #52 (`micro.recipe.started`, `micro.candidate.completed`, `micro.escalated`, `micro.recipe.ended`) are NOT expressible today. The only journal-emit primitive is `workflow/tool-call` (which `step` itself uses in `prelude.rs` via `:on-tool-call`), but that hardcodes the `agent.tool_call` event type. Emitting typed custom events would require a new primitive, such as `(workflow/journal event-name payload)`. This is the concrete "where it cracks" answer for a package that wants to emit run-evidence beyond per-step leaves.

2. **Schema-invalid error handling is untyped:**
   When a step's schema validation fails (or runs out of re-asks), `llm/extract` throws a raw exception. Because we don't have typed errors (e.g. `{:error-type :schema-invalid}`), `try/catch` catches *any* error (including network failures or authentication errors). Without typed errors, it's too risky to implement fallback behavior on schema failures natively in a recipe.
