---
name: "llm/with-budget"
module: "llm"
params: [{ name: opts, type: map }, { name: thunk }]
returns: "any"
see_also: ["llm/set-budget", "llm/budget-remaining", "llm/clear-budget"]
---

Run a zero-argument function under a scoped spending cap. The opts map requires at least `:max-cost-usd` and/or `:max-tokens`; the scope is pushed before the thunk runs and restored on exit (even on error). LLM calls inside the thunk accumulate against the limit, and the call that would exceed it **raises an error** rather than silently truncating. Returns the thunk's result.

Use this to bound a single agent run or batch job — e.g. "this report is worth at
most 50 cents" — without touching global state. Unlike the global `llm/set-budget`,
the cap is scoped: it only applies inside the thunk, nests cleanly, and is removed
afterward. Enforcement is best-effort: cost is estimated from the pricing table
(`llm/pricing-status`), so a model with unknown pricing contributes `0.0` and won't
trip a `:max-cost-usd` cap — combine with `:max-tokens` if that matters.

```sema
;; Cap a multi-step task at 10 cents; the call that would blow the budget throws.
(try
  (llm/with-budget {:max-cost-usd 0.10}
    (fn ()
      (let [draft (llm/complete "draft a tagline")]
        (llm/complete (string-append "polish: " draft)))))
  (catch e
    (str "stopped: " (:message e))))
```
