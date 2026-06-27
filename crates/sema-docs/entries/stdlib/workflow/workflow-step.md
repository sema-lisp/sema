---
name: "workflow/step"
module: "workflow"
section: "Dynamic Workflows"
---

Run a leaf as a journaled **step**: `(workflow/step role thunk)` emits an
`agent.started` event before the thunk, an `agent.result` after (output + duration), and a
per-step `budget` event, so the dashboard renders it as a correlated row under the current
phase. (The `agent.*` event names are the journal's frozen internal contract, unchanged by
the step rename.) `role` is an opts map (`{:name "scout" …}`) or a bare label; the default
role is `"step"`. Returns the thunk's value, or propagates its error after journaling the
result. Outside a `workflow/run` it is transparent — it just calls the thunk.

This is the journaling wrapper; the `step` macro is the ergonomic surface (it supplies the
LLM-call thunk and handles `:schema`/`:tools`/`:agent`). Compose it inside `pipeline` or
`parallel` to make a fanned-out set of leaves show up as sibling rows.

Pass `:prompt` in the opts map to capture the prompt on the `agent.started` event (the
dashboard's Prompt panel). The `step` macro does this for you; a hand-wrapped
`workflow/step` that builds its prompt inside the thunk should pass it explicitly, or the
Prompt panel shows "not captured".

```sema
;; usually written as the `step` macro (captures the prompt automatically):
(step (str "Explain " topic) {:name "writer" :schema article})

;; the macro expands to the wrapper around an LLM-call thunk:
(workflow/step {:name "writer"}
  (fn () (llm/extract article (str "Explain " topic))))

;; hand-wrapped: bind the prompt once and pass :prompt so it is journaled + shown
(let ((p (str "Explain " topic)))
  (workflow/step {:name "writer" :prompt p}
    (fn () (llm/complete p))))
```

See also: `step`, `pipeline`, `workflow/run`, `checkpoint`.
