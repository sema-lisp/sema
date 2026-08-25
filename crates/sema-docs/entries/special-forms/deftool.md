---
name: "deftool"
module: "special-forms"
syntax: "(deftool name \"description\" parameters-map [options-map] handler-expr)"
---

Define a tool that can be invoked by an LLM agent. The `name` must be a symbol. The `description` is a human-readable string explaining what the tool does — the LLM uses this to decide when to call the tool. The `parameters-map` describes the tool's arguments using a JSON Schema-like structure; each key is a parameter name mapping to a map with `:type`, `:description`, and optionally other schema fields. The `handler-expr` is evaluated to produce a function that receives the tool arguments and returns a result.

The tool value is bound to `name` in the current environment and is also returned by the form. You can inspect a tool with `tool/name`, `tool/description`, `tool/parameters`, `tool/policy-subjects`, and test values with `tool?`. Tools are passed to agents via the `:tools` key in `defagent`.

Each parameter is required by default. Add `:optional #t` to allow the caller (the LLM, or a direct `tool/invoke`) to omit it — the handler then receives `nil` for that argument. Add `:default <value>` to give it a fallback instead of `nil`; a declared `:default` also makes the parameter optional, so `:optional #t` is redundant once `:default` is set. Both keys are reflected in the generated JSON Schema sent to the LLM (`:default` becomes the schema's `default`, and the field drops out of `required`).

```sema
(deftool greet
  "Greet someone by name."
  {:name {:type :string :description "The person's name" :default "world"}}
  (lambda (name) (string-append "Hello, " name "!")))

(tool/invoke greet {})              ; => "Hello, world!"
(tool/invoke greet {:name "Ada"})   ; => "Hello, Ada!"
```

An optional `options-map` may sit between `parameters-map` and `handler-expr`. Its only key is `:policy-subjects`, a list or vector of maps that declare what the tool acts on, so a `defpolicy` `:subjects` rule matches by meaning instead of by tool name. Each subject map needs a `:kind`: `:file-read`, `:file-write`, or `:file-delete` (with `:path-arg`); `:network-request` (with `:url-arg` and optional `:method`); `:command` (with `:command-arg`); or `:external-action` (with `:action` and optional `:target-arg`). Each `*-arg` value names the parameter that carries the value at call time.

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

A tool that declares a policy subject:

```sema
(deftool read-source
  "Read a source file."
  {:path {:type :string :description "File to read"}}
  {:policy-subjects [{:kind :file-read :path-arg :path}]}
  (lambda (path) (file/read path)))

(tool/policy-subjects read-source)
; => [{:kind :file-read :path-arg :path}]
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
