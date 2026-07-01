---
name: "agent/run"
module: "agent"
params: [{ name: agent, type: agent }, { name: message, type: string }, { name: opts, type: map }]
returns: "string"
---

Run an agent on a user message, driving the full tool-execution loop up to the agent's `:max-turns`, and return the model's final reply.

The agent (built with `defagent`) carries a system prompt, a model, a tool set, and a turn cap. `agent/run` sends the message, lets the model call the agent's tools, feeds each tool result back, and repeats until the model answers without a tool call or `:max-turns` is hit. Tool-result correlation is handled for you — you never hand-wire the round-trip.

**Two shapes:**

- `(agent/run agent message)` — returns just the final reply **string**.
- `(agent/run agent message opts)` — returns a map `{:response ... :messages ...}`, where `:messages` is the full conversation (so you can continue it). `opts` accepts `:messages` (prior history to resume from), `:on-tool-call` (a callback fired for each tool call), and `:on-text` (a callback fired with each streamed assistant text delta). The tool callback receives an event map: `{:event "start" :tool name :args …}` before the tool runs and `{:event "end" :tool name :args … :result … :error bool :duration-ms n}` after. `:on-text` is called with each text chunk as the reply is generated, so a front-end can render the answer live; the streamed chunks concatenate to `:response`. Without `:on-text` the reply is returned whole (non-streaming). Tool calls work under streaming too.

Each turn is generated with a fixed `max-tokens` of 4096. A turn that hits `:max-turns` returns whatever the model last produced — it does not error, so check the reply if the task may not have completed.

```sema
(defagent weather-bot
  {:system "You report the weather concisely."
   :model "claude-haiku-4-5-20251001"
   :tools [get-weather]
   :max-turns 5})

;; simple: just the final answer
(agent/run weather-bot "What's the weather in Oslo?")
; => "Oslo is 4°C and cloudy."

;; resumable: keep the full transcript and log each tool call
(define out
  (agent/run weather-bot "And tomorrow?"
    {:messages prior-history
     :on-tool-call (fn (ev) (println (:event ev) " " (:tool ev)))}))
(:response out)   ; the reply string
(:messages out)   ; the conversation, ready to pass back in as :messages
```

See also: `defagent`, `deftool`, `agent/tools`, `agent/max-turns`.
