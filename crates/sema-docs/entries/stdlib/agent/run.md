---
name: "agent/run"
module: "agent"
params: [{ name: agent, type: agent }, { name: message, type: string }, { name: opts, type: map }]
returns: "string"
---

Run an agent on a user message, executing its tools in a loop up to the agent's `:max-turns`. With two arguments it returns the final reply string. With an opts map it returns `{:response ... :messages ...}`; opts accepts `:messages` (prior history) and `:on-tool-call` (a callback invoked for each tool call). Each turn is generated with a fixed `max-tokens` of 4096.

```sema
(agent/run my-agent "Look up the weather in Oslo and summarize it.")
```
