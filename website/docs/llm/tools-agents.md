---
outline: [2, 3]
---

# Tools & Agents

## Tools

Tools let you define functions that the LLM can invoke during a conversation. The LLM sees the tool's name, description, and parameter schema, and can call it when appropriate.

### `deftool`

Define a tool with a name, description, parameter schema, and handler function.

```sema
(deftool lookup-capital
  "Look up the capital of a country"
  {:country {:type :string :description "Country name"}}
  (lambda (country)
    (cond
      ((= country "Norway") "Oslo")
      ((= country "France") "Paris")
      (else "Unknown"))))
```

An optional options map goes between the parameter schema and the handler. It
accepts one key, `:policy-subjects`, which declares the file, network, command,
or external action the tool performs. A workflow policy `:subjects` rule matches
those declarations — see
[Semantic subjects](/docs/llm/workflows#semantic-subjects).

```sema
(deftool read-source
  "Read a source file."
  {:path {:type :string}}
  {:policy-subjects [{:kind :file-read :path-arg :path}]}
  (lambda (path) (file/read path)))
```

### Using Tools with Chat

Pass tools to `llm/chat` — the LLM will call them automatically when needed.

```sema
(llm/chat
  (list (message :user "What is the capital of Norway?"))
  {:tools (list lookup-capital) :max-tokens 100})
```

### Inspecting Tools

### `tool/name`

```sema
(tool/name lookup-capital)              ; => "lookup-capital"
```

### `tool/description`

```sema
(tool/description lookup-capital)       ; => "Look up the capital..."
```

### `tool/parameters`

```sema
(tool/parameters lookup-capital)        ; => {:country {:type :string ...}}
```

### `tool/policy-subjects`

```sema
(tool/policy-subjects read-source)      ; => [{:kind :file-read :path-arg :path}]
(tool/policy-subjects lookup-capital)   ; => []
```

### `tool?`

```sema
(tool? lookup-capital)                  ; => #t
```

## Agents

Agents combine a system prompt, tools, and a multi-turn loop. They handle the back-and-forth of tool calls automatically.

### `defagent`

Define an agent with a system prompt, tools, model, and turn limit.

```sema
(deftool get-weather
  "Get weather for a city"
  {:city {:type :string}}
  (lambda (city)
    (format "~a: 22°C, sunny" city)))

(defagent weather-bot
  {:system "You are a weather assistant. Use the get-weather tool."
   :tools [get-weather]
   :model "claude-haiku-4-5-20251001"
   :max-turns 3})
```

### `agent/run`

Run an agent with a user message. The agent loops, calling tools as needed, until it has a final answer or hits the turn limit. The two-argument form returns the final answer as a **string**:

```sema
(agent/run weather-bot "What's the weather in Tokyo?")  ; => "It's sunny, 22°C."
```

An optional third argument takes per-run options. **Passing an options map changes the return value** to a map with the final reply *and* the full message history:

```sema
(define result
  (agent/run weather-bot "What's the weather in Tokyo?"
    {:reasoning-effort :high       ; reasoning effort for this run (see Completion)
     :messages prior-history       ; seed the loop with prior conversation
     :memory mem                   ; persistent thread — see Agent Memory
     :on-tool-call observe-tool     ; observe each tool call — see below
     :on-partial keep-partial}))    ; keep the transcript if the run is cancelled

(:response result)   ; => the final answer string
(:messages result)   ; => the full conversation (to continue or inspect)
```

**Observing tool calls.** `:on-tool-call` fires once when each tool starts and once when it ends. The event is a map — branch on `(:event e)`, the string `"start"` or `"end"`:

```sema
(define (observe-tool e)
  (when (= (:event e) "end")
    (println (:tool e) "→" (:result e) (format "(~ams)" (:duration-ms e)))))
```

The event map carries `:event` (`"start"` / `"end"`), `:tool` (the tool name), and `:args`; on `"end"` it adds `:result` (a preview of the return value), `:error` (a boolean), and `:duration-ms`.

**Keeping the transcript of a cancelled run.** A run stopped with `async/cancel`
has no return value — `async/await` raises instead — so the conversation it had
assembled would be lost. `:on-partial` receives that conversation as the usual
`{:response :messages :session}` map, just before the cancellation propagates:

```sema
(define partial nil)
(define (keep-partial r) (set! partial r))

(let ((p (async/spawn (fn () (agent/run weather-bot "..." {:on-partial keep-partial})))))
  (async/spawn (fn () (async/sleep 250) (async/cancel p)))
  (try (async/await p) (catch e nil)))

(:messages partial)   ; the rounds that completed, ready to pass back as :messages
```

The callback also fires when a run ends with an error. It runs in the cancelled
task's last step, which cannot park again, so it must only capture the value —
never `async/await` or start I/O. It reports the rounds that completed; the text
of a round still streaming when the cancel lands arrives through `:on-text`
only. To persist the turns instead of holding them in memory, use `:memory` — a
cancelled run writes its turns back to the thread too (see Agent Memory).

**Error recovery.** A tool that throws, isn't found, or is called with arguments
that don't match its declared schema does **not** abort the run — the error is
fed back to the model as the tool result so it can correct itself and continue.
The loop is bounded by `:max-turns` and aborts after 5 consecutive tool errors.

### Inspecting Agents

### `agent/name`

```sema
(agent/name weather-bot)                ; => "weather-bot"
```

### `agent/system`

```sema
(agent/system weather-bot)              ; => "You are a weather assistant..."
```

### `agent/tools`

```sema
(agent/tools weather-bot)               ; => list of tool values
```

### `agent/model`

```sema
(agent/model weather-bot)               ; => "claude-haiku-4-5-20251001"
```

### `agent/max-turns`

```sema
(agent/max-turns weather-bot)           ; => 3
```

### `agent?`

```sema
(agent? weather-bot)                    ; => #t
```
