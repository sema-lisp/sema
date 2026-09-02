---
name: "memory/messages"
module: "memory"
section: "Agent Memory"
syntax: "(memory/messages handle)"
returns: "conversation"
see_also: ["memory/open", "memory/append", "conversation/messages"]
---

Return the full working set of a memory thread as a `Conversation` value — the same type
that `conversation/new` and `llm/chat` return. The value is compatible with all
`conversation/*` operations (filter, fork, add-message, etc.).

Roles are normalized: `"assistant"` stays `"assistant"`; anything else maps to `"user"`.

```sema
(define mem (memory/open {:id "session-1"}))
(memory/append mem {:role "user" :content "Hi"})

(define conv (memory/messages mem))

;; pass it as conversation history to an LLM call
(llm/chat conv {:model "claude-haiku-4-5-20251001"})

;; count turns
(length (conversation/messages conv))   ; => 1
```

See also: `memory/open`, `memory/append`, `conversation/new`.
