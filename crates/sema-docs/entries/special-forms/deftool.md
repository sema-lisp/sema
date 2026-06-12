---
name: "deftool"
module: "special-forms"
syntax: "(deftool name \"description\" parameters-map handler-expr)"
---

Define a tool that can be invoked by an LLM agent. The `name` must be a symbol. The `description` is a human-readable string explaining what the tool does — the LLM uses this to decide when to call the tool. The `parameters-map` describes the tool's arguments using a JSON Schema-like structure; each key is a parameter name mapping to a map with `:type`, `:description`, and optionally other schema fields. The `handler-expr` is evaluated to produce a function that receives the tool arguments and returns a result.

The tool value is bound to `name` in the current environment and is also returned by the form. You can inspect a tool with `tool/name`, `tool/description`, `tool/parameters`, and test values with `tool?`. Tools are passed to agents via the `:tools` key in `defagent`.

```sema
(deftool add-numbers
  "Add two numbers together."
  {:a {:type :number :description "First number"}
   :b {:type :number :description "Second number"}}
  (lambda (a b) (+ a b)))
```

A tool that works with a single map argument (common pattern for flexible schemas):

```sema
(deftool greet-person
  "Greet someone by name."
  {:name {:type :string :description "The person's name"}}
  (lambda (name)
    (string-append "Hello, " name "!")))
```

Inspecting a tool:

```sema
(tool/name add-numbers)           ; => "add-numbers"
(tool/description add-numbers)    ; => "Add two numbers together."
(map? (tool/parameters add-numbers))  ; => #t
(tool? add-numbers)               ; => #t
```

Using a tool with an agent:

```sema
(defagent calculator
  {:system "You help with math."
   :tools [add-numbers]
   :max-turns 5})
```
