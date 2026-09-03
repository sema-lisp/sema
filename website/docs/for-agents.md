---
title: Sema for LLM Agents
description: A compact working guide for coding agents that need to write and verify Sema programs.
aside: false
---

# Sema for LLM Agents

Use this page as a fast-start guide when you already know a Lisp. It covers the
rules most likely to cause incorrect generated code, but it is not the full
language or standard-library reference. Search the installed documentation when
possible:

```bash
sema doc if
sema doc string/split
sema doc search "parse json from a string"
```

The online [`/llms.txt`](/llms.txt) file indexes the individual Markdown pages.
Fetch only the page you need. Do not load [`/llms-full.txt`](/llms-full.txt), the
large concatenation of all documentation, into one context.

## Install, run, and verify

```bash
curl -fsSL https://sema-lang.com/install.sh | sh
# Alternatives: brew install helgesverre/tap/sema-lang
#               cargo install sema-lang

sema script.sema          # run a file
sema -e '(println "hi")'  # evaluate an expression
sema                      # start the REPL
sema fmt script.sema      # format in place
```

For generated code, format it and run it. Do not treat plausible Lisp syntax as
proof that the program is valid Sema.

## A complete small program

```sema
(define tickets
  [{:id 101 :priority :high}
   {:id 102 :priority :low}])

(define (urgent? ticket)
  (equal? (:priority ticket) :high))

(define urgent-ids
  (->> tickets
       (filter urgent?)
       (map :id)))

(println urgent-ids) ; => (101)
```

Calls and special forms use prefix s-expressions. Sema also has vector, map,
string, keyword, character, and numeric literals.

## Reader syntax

```sema
; comment                    ; comments run to the end of the line
'(a b c)                    ; quote: data, not a call
`(a ,value ,@items)         ; quasiquote, unquote, splice
:keyword                    ; self-evaluating keyword; also callable as a getter
{:a 1 :b 2}                 ; sorted map literal
[1 2 3]                     ; vector literal, distinct from a list
(:name person)              ; same as (get person :name)
#(* % %)                    ; short lambda; %, %1, %2, and %& are parameters
f"hi ${name}, ${(+ 1 2)}"   ; interpolated string
#"\d+"                      ; regex literal; contents are raw except for \"
```

A list in operator position is evaluated as a call or special form. Quote it
when you need list data. Vectors and maps evaluate their elements and produce
collection values.

## Naming conventions

- New standard-library functions are slash-namespaced: `file/read`, `path/join`,
  `string/split`, `regex/match?`, `http/get`, and `json/encode`. Do not invent
  names such as `read-file` or `split-string`.
- Predicates end in `?`: `null?`, `list?`, `empty?`, and `file/exists?`.
- Conversions use `->`: `string->symbol`, `keyword->string`, and
  `list->vector`.
- A few Scheme string names remain: `string-append`, `string-length`,
  `string-ref`, and `substring`.

Use `sema doc search` instead of guessing a builtin name.

## Truthiness and distinct empty values

Sema has distinct `nil` and empty-list values. This differs from Common Lisp,
where `NIL` is also the empty list.

| Value | Truthiness | Type |
| --- | --- | --- |
| `nil` | falsy | `:nil` |
| `#f` | falsy | `:bool` |
| `()` | truthy | `:list` |
| `""` | truthy | `:string` |
| `0` | truthy | `:int` |

```sema
(if nil :yes :no)  ; => :no
(if '() :yes :no)  ; => :yes
(nil? '())          ; => #f
(null? '())         ; => #t
```

`if` requires a condition and a then expression. The else expression is
optional and defaults to `nil`. It evaluates only the selected branch.

## Bindings, functions, and local scope

```sema
(define answer 42)                         ; alias: def
(set! answer 43)                           ; mutate an existing binding
(define (square x) (* x x))
(defun cube (x) (* x x x))                 ; alias: defn
(define square-again (lambda (x) (* x x))) ; alias: fn

(let ((x 10) (y 20)) (+ x y))              ; parallel initializers
(let* ((x 10) (y (* x 2))) (+ x y))        ; sequential initializers
```

`let` bindings use `((name value) ...)`, not Clojure's flat
`[name value ...]`. `let`, `let*`, `define`, and function parameters support
vector and map destructuring. Tail calls are optimized.

There are no Clojure `atom`, `swap!`, or `reset!` builtins. Use bindings with
`set!`, or the documented mutable container APIs when shared mutable state is
required.

## Collections and equality

Sema lists are immutable contiguous sequences, not chains of cons cells. This
is visible in their behavior and costs:

- `nth`, `length`, and `car`/`first` are O(1).
- `cons`, `cdr`/`rest`, and `append` can copy O(n) elements.
- Use `map`, `filter`, `fold`, and vectors for repeated sequence processing.
- Do not depend on cons-cell identity or arbitrary dotted-pair construction.

Map literals are sorted maps with deterministic iteration. `(hashmap/new)`
creates an unordered hash map for workloads that need it.

`=` applies numeric equality across numeric representations, so `(= 1 1.0)` is
`#t`; for non-numbers it uses structural equality. `eq?` and `equal?` are
structural aliases without numeric coercion, so `(equal? 1 1.0)` is `#f`.

## Conditionals and pattern matching

```sema
(cond
  ((< n 0) :negative)
  ((= n 0) :zero)
  (else :positive))

(match response
  ({:status :ok :data value} value)
  ({:status :error :message message} (throw message))
  (_ nil))
```

Each `match` clause must be a list or vector containing a pattern and body.
`match` raises when no clause matches. Add `(_ ...)`, or use `match*` when no
match should return `nil`.

## Errors

Use `throw` or the `raise` function to raise an error. Catch errors with
`try`/`catch`:

```sema
(try
  (file/read "missing.txt")
  (catch e
    (println (:type e) (:message e))))
```

A caught error is a map containing `:type`, `:message`, `:value` for a
user-thrown value, and `:stack-trace`. Do not assume that errors are plain
strings.

## Modules and packages

Modules expose only explicitly exported names:

```sema
(module math-tools
  (export double)
  (define (double x) (* x 2)))
```

Load a module file with `import`; paths resolve relative to the importing file.
Use `sema pkg` for registry or Git dependencies. Read
[Macros & Modules](/docs/language/macros-modules) and
[Packages & Modules](/docs/packages) before generating a multi-file project.

## Async execution

Sema uses a single-threaded cooperative scheduler, not shared-memory OS threads.
`async` creates a task and returns an async promise; `await` yields until it
settles. Channels coordinate tasks. Do not use blocking host operations as a
substitute for the async APIs.

```sema
(define task (async (http/get "https://example.com")))
(define response (await task))
```

## What the LLM runtime integrates

Most individual features below could be implemented in a Common Lisp or Scheme
library. Sema packages them with one value model and runtime: provider
translation, typed tools, bounded agent loops, scoped controls, cooperative
async, policy enforcement, tracing, workflows, and standalone builds are
implemented and tested together.

Prompt, message, conversation, tool, and agent values have distinct runtime
types and inspection functions:

```sema
(define review
  (prompt
    (system "Review the code. Be concise.")
    (user "{{code}}")))

(type review)                               ; => :prompt
(prompt/slots review)                       ; => (:code)
(map message/role (prompt/messages review)) ; => (:system :user)
(define ready (prompt/fill review {:code source}))
```

These values can be bound, passed to functions, inspected, transformed, and
placed in in-memory collections. They are runtime objects, not a promise that
every value can be encoded as JSON or stored in a compiled constant pool. Store
serializable data derived from them; use conversations, agent memory, workflow
journals, or cassettes for the documented persistence cases.

## Configure a provider

Sema configures providers from environment variables at startup. Common native
providers are:

| Provider | Environment variable | Default model |
| --- | --- | --- |
| Anthropic | `ANTHROPIC_API_KEY` | `claude-sonnet-4-6` |
| OpenAI | `OPENAI_API_KEY` | `gpt-5.5` |
| Google Gemini | `GOOGLE_API_KEY` | `gemini-3.5-flash` |
| Ollama | `OLLAMA_HOST` | `gemma4` |

Groq, xAI, Mistral, Moonshot, and other compatible providers are also
supported. See [Providers](/docs/llm/providers) for the full list.

The first configured chat provider becomes the default. Inspect or change it:

```sema
(llm/current-provider)
(llm/set-default :openai)
```

A model name must belong to the selected provider. Prefer omitting `:model`
unless the program deliberately selects the matching provider first.

## Tools and agents

```sema
(deftool get-weather
  "Get weather for a city"
  {:city {:type :string :description "City name"}}
  (lambda (city)
    {:city city :temperature-c 22}))

(defagent weather-bot
  {:system "Give a short forecast. Use get-weather."
   :tools [get-weather]
   :max-turns 3})

(agent/run weather-bot "Weather in Oslo?")
```

`deftool` joins a name, description, JSON-schema-like parameter map, policy
subjects, and handler. Tool and agent definitions are inspectable with
`tool/*`, `agent/*`, `tool?`, and `agent?` functions.

The two-argument `agent/run` returns the final string. Passing a third options
map returns a result map containing `:response`, `:messages`, `:session`, and
`:usage`. Tool exceptions and schema errors are returned to the model so it can
correct the call; the loop is still bounded by `:max-turns` and the consecutive
tool-error limit.

## Scoped controls and reproducible runs

Controls wrap ordinary functions, so they compose lexically:

```sema
(llm/with-cassette
  "weather.jsonl" {:mode :auto}
  (lambda ()
    (llm/with-budget
      {:max-cost-usd 0.10}
      (lambda ()
        (agent/run weather-bot "Weather in Oslo?")))))
```

Use the documented forms for response caching, provider fallback, retry,
budgets, cassettes, and OpenTelemetry. A cassette records provider traffic for
offline deterministic tests; it does not serialize an arbitrary live agent
value.

## Journaled workflows

Use workflows for resumable multi-step agent runs:

```sema
(defworkflow audit
  "Audit src without writing files."
  {:phases ["Scan" "Report"]
   :permissions "no-fs-write,no-network"}

  (phase "Scan")
  (define files (checkpoint :files (file/list "src")))

  (phase "Report")
  (define summary (step "Summarize the files." {:name "reporter"}))
  {:status :success :files files :summary summary})
```

`phase` is a marker followed by sibling forms, not a wrapper around a body.
`checkpoint` memoizes a value in the run journal. Read
[Workflows](/docs/llm/workflows) before generating permissions, approvals,
parallel steps, or resume logic.

## Generated-code checklist

- Did you search for builtin names instead of guessing them?
- Are list data and call expressions quoted correctly?
- Does every `if` have a condition and then expression, with an else expression
  when the falsy case must return something other than `nil`?
- Did you treat `nil` and `()` as distinct values?
- Are `let` bindings nested rather than written in flat Clojure form?
- Is every `match` clause a list or vector, with a fallback when required?
- Does the selected model belong to the active provider?
- Did you handle the correct `agent/run` return shape?
- Did you use async APIs for operations that can wait?
- Did you run `sema fmt` and execute the result?

## Targeted references

- [Basic syntax](/docs/tutorial/basics)
- [Data types](/docs/language/data-types)
- [Special forms](/docs/language/special-forms)
- [Macros and modules](/docs/language/macros-modules)
- [Tools and agents](/docs/llm/tools-agents)
- [Providers](/docs/llm/providers)
- [Standard library](/docs/stdlib/)
- [Glossary](/docs/internals/glossary)
