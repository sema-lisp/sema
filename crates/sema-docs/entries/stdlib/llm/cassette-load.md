---
name: "llm/cassette-load"
module: "llm"
params: [{ name: path, type: string }, { name: opts, type: map }]
returns: "nil"
---

Install a cassette for subsequent calls in the current evaluation. Tasks spawned after the load inherit it; tasks already spawned keep the cassette they captured. The opts map is optional and accepts `:mode` (`:auto` default, `:record`, or `:replay`). Use `llm/cassette-save` to flush and `llm/cassette-eject` to remove it from the current scope. For scoped use prefer `llm/with-cassette`.

```sema
(llm/cassette-load "tapes/suite.jsonl" {:mode :replay})
```
