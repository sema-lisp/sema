---
name: "step"
module: "workflow"
section: "Dynamic Workflows"
syntax: "(step prompt [opts])"
---

Macro: a journaled **step** — a workflow's atomic orchestration unit (Claude Code
`workflow.js` `step(prompt, {…})` semantics). A step is an anonymous, workflow-owned call
site; the named, reusable counterpart is an `agent`. Runs `prompt` through the configured
provider and returns **typed data** when `opts` carries a `:schema` (validated via
`llm/extract`), or the completion text otherwise — so the next stage of a `pipeline`
can consume the result directly without re-parsing. `opts` also carries `:name`, the role
label shown in the dashboard (default `"step"`).

The call is wrapped by `workflow/step`, which emits `agent.started`/`agent.result`
plus a per-step `budget` event, so each invocation becomes a correlated row under the
current phase. (The `agent.*` event names are the journal's frozen internal contract and
predate the step rename.) Outside a `workflow/run` the journaling is transparent — the LLM
call still runs.

Routing on `opts` (`:agent` and inline `:tools`/`:model` are mutually exclusive — the agent
owns those):

- `:agent A` — run the configured `defagent` `A` **as** this step via `agent/run` (its
  own system prompt + tools + model + max-turns), with `prompt` as the user message. The
  agent's genuine tool calls still journal as `agent.tool_call`. `:name` defaults to `A`'s
  own name. With `:schema`, `A`'s text is validated.
- `:tools [...]` (a list of `deftool` values) — run the real multi-round tool loop and
  journal **each genuine tool call** as an `agent.tool_call` event (a tool twig in the
  drill-in). With no `:schema` it returns the loop's final text; with `:schema` the text is
  validated. Per-step budget for a multi-round tool loop is best-effort (the Budget event
  reflects the final round's usage).
- `:schema S` — `llm/extract` (typed data).
- otherwise — `llm/complete` (text).

```sema
;; typed: returns the parsed list of strings
(step "List the auth-relevant source files under src/."
      {:name "scout" :schema [:list :string]})

;; untyped: returns completion text
(step "Summarize the changelog in one line.")

;; run a configured defagent as a step
(step "Review file Y" {:agent code-reviewer :schema verdict})

;; fanned out — one row per item, two stages overlapping across items
(pipeline files
  (fn (f) (step (str "Audit " f) {:name "auditor" :schema finding}))
  (fn (x) (step (str "Verify " (:claim x)) {:name "verifier" :schema verdict})))
```

See also: `agent`, `workflow/step`, `pipeline`, `parallel`, `defworkflow`, `checkpoint`.
