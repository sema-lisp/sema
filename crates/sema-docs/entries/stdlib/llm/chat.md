---
name: "llm/chat"
module: "llm"
params: [{ name: messages }, { name: opts, type: map }]
returns: "string"
see_also: ["llm/complete", "llm/send", "llm/stream"]
---

Run a multi-message chat against the default provider and return the assistant's text reply. The first argument is a sequence of messages (build them with `(message :role "...")`). The optional opts map accepts `:model`, `:max-tokens`, `:temperature`, `:system`, `:tools`, `:tool-mode` (`:auto` or `:none`), and `:max-tool-rounds` (default 10).

`llm/chat` is the multi-turn / agentic entry point: pass the full conversation
(prior user and assistant turns) so the model has context — it is stateless, so you
own the history. When you supply `:tools` and leave `:tool-mode` at `:auto`, it runs
a **tool-execution loop**: the model may request a tool call, the runtime runs your
Sema function, feeds the result back, and repeats — up to `:max-tool-rounds` — until
the model produces a final text answer (which is what's returned). Set
`:tool-mode :none` to expose the tools but never auto-execute them. For a single
stateless prompt with no history, `llm/complete` is simpler; for a streamed reply
use `llm/stream`.

```sema
;; Plain multi-turn: include the running history yourself.
(llm/chat (list (message :user "Capital of France?")
                (message :assistant "Paris.")
                (message :user "And its population?"))
          {:model "gpt-4o-mini"})

;; Agentic: define a tool, the model can call it; runtime loops to a final answer.
(deftool get-weather
  "Look up the weather for a city"
  {:city {:type :string :description "City name"}}
  (lambda (city) (string-append "Sunny in " city)))

(llm/chat (list (message :user "What's the weather in Oslo?"))
          {:tools (list get-weather)})
```
