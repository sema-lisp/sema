---
outline: [2, 3]
---

# Getting Started

Sema is a Scheme-like Lisp where prompts are s-expressions, conversations are persistent data structures, and LLM calls are just another form of evaluation. It combines a Scheme core with Clojure-style keywords (`:foo`), map literals (`{:key val}`), and vector literals (`[1 2 3]`).

> Sema takes its name from Ancient Greek *sêma* (σῆμα), a sign or token of meaning — the same root as *semantics*, *semaphore*, and *semiotics*.

## Why Sema?

- **LLMs as language primitives** — prompts, messages, conversations, tools, and agents are first-class data types, not string templates bolted on
- **Multi-provider** — Anthropic, OpenAI, Gemini, Groq, xAI, Mistral, Ollama, and more, all auto-configured from environment variables
- **Practical Lisp** — closures, tail-call optimization, macros, modules, error handling, HTTP, file I/O, regex, JSON — everything you need to build real programs
- **Embeddable** — clean Rust crate structure, builder API, sync interface ([learn more](./embedding.md))

## Installation

Install pre-built binaries (no Rust required):

```bash
# macOS / Linux
curl -fsSL https://sema-lang.com/install.sh | sh

# Windows (PowerShell)
powershell -ExecutionPolicy ByPass -c "irm https://sema-lang.com/install.ps1 | iex"

# Homebrew (macOS / Linux)
brew install helgesverre/tap/sema-lang
```

Or install from [crates.io](https://crates.io/crates/sema-lang):

```bash
cargo install sema-lang
```

Or build from source:

```bash
git clone https://github.com/sema-lisp/sema
cd sema
cargo build --release
# Binary is at target/release/sema
```

## Quick Start

```bash
sema                          # Start the REPL
sema script.sema              # Run a file
sema -e '(+ 1 2)'             # Evaluate an expression
sema -p '(map sqr (range 5))' # Evaluate and always print
```

```sema
;; In the REPL:
sema> (define (greet name) f"Hello, ${name}!")
sema> (greet "world")
"Hello, world!"

sema> (map #(* % %) (range 1 6))
(1 4 9 16 25)

sema> (define person {:name "Ada" :age 36})
sema> (:name person)
"Ada"
```

## Examples

### Working with Data

```sema
;; Keywords as accessor functions, short lambdas with #(...)
(define people [{:name "Ada" :age 36}
                {:name "Bob" :age 28}
                {:name "Cat" :age 42}])

(map #(:name %) people)              ; => ("Ada" "Bob" "Cat")

(->> people
     (filter #(> (:age %) 30))
     (map #(:name %)))               ; => ("Ada" "Cat")

;; Destructuring and f-strings
(let (({:keys [name age]} (first people)))
  (println f"${name} is ${age} years old"))

;; Pattern matching
(define (describe person)
  (match person
    ({:keys [name age]} when (> age 40)
      f"${name} is experienced")
    ({:keys [name]}
      f"${name} is on the team")))
```

### LLM Completion

```sema
;; Simple completion (requires an API key env var)
(llm/complete "Explain recursion in one sentence" {:max-tokens 50})

;; Structured chat with message history
(llm/chat
  (list (message :system "You are a helpful assistant.")
        (message :user "What is Lisp? One sentence."))
  {:max-tokens 100})
```

### Persistent Conversations

```sema
;; Each conversation/say makes a real LLM call, threading prior turns as history.
(define conv (conversation/new {:model "claude-haiku-4-5-20251001"}))
(define conv (conversation/say conv "Remember: the secret number is 7"))
(define conv (conversation/say conv "What is the secret number?"))
(conversation/last-reply conv)
; => the model's reply, e.g. "The secret number is 7." — it recalls the earlier turn
```

## What's Next?

- [CLI Reference](./cli.md) — all flags, subcommands, and environment variables
- [Shell Completions](./shell-completions.md) — tab completions for bash, zsh, fish, and more
- [Editor Support](./editors.md) — plugins for VS Code, Vim/Neovim, Emacs, Helix, and Zed
- [Embedding in Rust](./embedding.md) — use Sema as a scripting engine in your app
- [Data Types](./language/data-types.md) — all built-in types
- [Special Forms](./language/special-forms.md) — control flow, bindings, and iteration
- [Macros & Modules](./language/macros-modules.md) — metaprogramming and code organization
- [LLM Primitives](./llm/) — completions, chat, tools, agents, embeddings, and more
