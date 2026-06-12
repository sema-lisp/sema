---
name: "llm/chat"
module: "llm"
params: [{ name: messages }, { name: opts, type: map }]
returns: "string"
---

Run a multi-message chat against the default provider and return the assistant's text reply. The first argument is a sequence of messages. The optional opts map accepts `:model`, `:max-tokens`, `:temperature`, `:system`, `:tools`, `:tool-mode` (`:auto` or `:none`), and `:max-tool-rounds` (default 10). When tools are supplied and tool-mode is not `:none`, it runs a tool-execution loop.

```sema
(llm/chat (list (message :user "What is the capital of France?"))
          {:model "gpt-4o-mini"})
```
