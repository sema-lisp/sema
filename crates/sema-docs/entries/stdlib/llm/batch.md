---
name: "llm/batch"
module: "llm"
params: [{ name: prompts }, { name: opts, type: map }]
returns: "list"
see_also: ["llm/pmap", "llm/complete", "llm/with-rate-limit"]
---

Send a list of prompt strings to the default provider in parallel and return a list of completion strings in **input order** (result `i` answers prompt `i`). The opts map accepts `:model`, `:max-tokens`, `:temperature`, and `:system`.

Each prompt is an independent, stateless call — there is no shared conversation —
but they run concurrently, so a batch of N is far faster than N sequential
`llm/complete` calls. Order is preserved regardless of which finishes first.
Pair with `llm/with-rate-limit` to stay under a provider's requests-per-second cap.
Use `llm/pmap` when your inputs aren't prompt strings yet — it maps a function over
a collection to build the prompts, then batches them.

```sema
(llm/batch ["Capital of France?" "Capital of Japan?"] {:max-tokens 20})
; => ("Paris" "Tokyo")
```
