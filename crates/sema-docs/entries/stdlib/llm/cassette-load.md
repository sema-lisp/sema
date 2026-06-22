---
name: "llm/cassette-load"
module: "llm"
params: [{ name: path, type: string }, { name: opts, type: map }]
returns: "nil"
---

Install a cassette globally (record/replay of LLM calls) from a tape file. The opts map is optional and accepts `:mode` (`:auto` default, `:record`, or `:replay`). Use `llm/cassette-save` to flush and `llm/cassette-eject` to remove it. For scoped use prefer `llm/with-cassette`.

```sema
(llm/cassette-load "tapes/suite.jsonl" {:mode :replay})
```
