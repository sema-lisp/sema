---
name: "message"
module: "special-forms"
syntax: "(message role content ...)"
---

Construct a single chat message value. The first argument must be a role keyword — one of `:system`, `:user`, `:assistant`, or `:tool`. The remaining arguments are evaluated and their results concatenated into the message content. String values are appended directly; non-string values are converted to strings automatically. The resulting message value can be used standalone or combined inside a `prompt` form.

```sema
(message :user "Hello, " "world")
; => a message with role :user and content "Hello, world"
```

Non-string values are automatically stringified, which makes it convenient to embed computed results directly into messages.

```sema
(message :user "The answer is " (+ 6 7))
; => content "The answer is 13"
```

Messages are typically composed into a prompt for LLM calls, but they can also be inspected or manipulated directly.

```sema
(define m (message :system "You are a Sema tutor."))
(prompt m (message :user "How do I use destructuring?"))
```

**Note:** `message` is a special form rather than a regular function because it evaluates its arguments individually and concatenates their string representations, producing a structured `Message` value rather than a plain string.
