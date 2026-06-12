---
name: "prompt"
module: "special-forms"
syntax: "(prompt (role content ...) ... )"
---

Build an LLM prompt value from a sequence of messages. Each argument to `prompt` is either a `(role content ...)` form or an existing message value produced by the `message` form. The supported role names are `system`, `user`, `assistant`, and `tool`, passed as bare symbols (not keywords). When a role form is used, the remaining arguments in that form are evaluated and their results concatenated into the message content. String values are appended directly; non-string values are stringified. The resulting prompt value can be passed to LLM functions such as `llm/chat`, `llm/stream`, or `llm/complete`.

```sema
(prompt
  (system "You are a helpful assistant.")
  (user "Explain recursion in one sentence"))
```

You can mix role forms with pre-built message values, which is useful when constructing prompts dynamically or reusing message fragments.

```sema
(define system-msg (message :system "You are a Sema expert."))
(define user-msg (message :user "What is a macro?"))
(prompt system-msg user-msg)
```

For multi-turn conversations, simply list messages in order. The LLM will see them as a single conversation thread.

```sema
(prompt
  (system "You are a coding assistant.")
  (user "Write a factorial function in Sema.")
  (assistant "(define (fact n) (if (= n 0) 1 (* n (fact (- n 1)))))")
  (user "Can you optimize it with tail recursion?"))
```

**Note:** `prompt` is a special form because it does not evaluate its arguments in the normal function-call way — it inspects each argument to detect role forms before evaluating their content parts.
