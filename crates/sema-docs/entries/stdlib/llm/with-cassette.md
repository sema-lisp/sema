---
name: "llm/with-cassette"
module: "llm"
params: [{ name: path, type: string }, { name: opts, type: map }, { name: thunk }]
returns: "any"
---

Record or replay LLM calls against a tape file for the duration of a zero-argument function — for deterministic, keyless tests and reproducible demos. `:mode` is `:auto` (default — replay if recorded, else record), `:record`, or `:replay` (a miss is a hard error). The opts map is optional. The response cache is disabled for the scope, and the tape is flushed on exit. Covers `llm/complete`, `llm/chat`, `llm/embed`, `llm/stream`, and agent loops. Returns the thunk's result.

```sema
(llm/with-cassette "tapes/greeting.jsonl" {:mode :auto}
  (fn () (llm/complete "Say hello in one word." {:model "gpt-5-mini"})))
```
