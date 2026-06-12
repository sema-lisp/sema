---
name: "defagent"
module: "special-forms"
syntax: "(defagent name {:system \"...\" :tools [...] :model \"...\" :max-turns N})"
---

Define an LLM agent. The `name` must be a symbol. The second argument is an options map that configures the agent. The `:system` key provides the system prompt string that sets the agent's behavior. The `:tools` key is a vector or list of tool values created with `deftool`, which the agent can invoke during a conversation. The `:model` key specifies which LLM model to use (e.g., `"claude-sonnet"` or `"gpt-4"`). The `:max-turns` key controls the maximum number of agent-tool interaction rounds and defaults to `10`.

The agent value is bound to `name` in the current environment and is also returned by the form. You can inspect an agent with accessor functions: `agent/name`, `agent/system`, `agent/tools`, `agent/model`, and `agent/max-turns`. Use `agent?` to check if a value is an agent.

```sema
(defagent greeter
  {:system "You are a friendly greeter. Keep responses brief."
   :model "claude-sonnet"
   :max-turns 5})
```

Defining an agent with tools:

```sema
(deftool get-weather
  "Get the current weather for a location."
  {:location {:type :string :description "City name"}}
  (lambda (location)
    (format "Sunny and 22°C in ~a" location)))

(defagent weather-bot
  {:system "You are a weather assistant."
   :tools [get-weather]
   :max-turns 10})
```

Inspecting an agent:

```sema
(agent/name greeter)       ; => "greeter"
(agent/system greeter)     ; => "You are a friendly greeter..."
(agent/max-turns greeter)  ; => 5
(agent? greeter)           ; => #t
```
