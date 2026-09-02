---
name: "llm/stream"
module: "llm"
params: [{ name: prompt }, { name: callback }, { name: opts, type: map }]
returns: "string"
see_also: ["llm/complete", "llm/chat"]
---

Stream a completion from the default provider, delivering the reply incrementally as it is generated instead of waiting for the whole thing. The first argument is a prompt string, a prompt value, or a messages sequence. An optional function argument is invoked with each text chunk as it arrives; with no callback, chunks are printed to stdout. The opts map accepts `:model`, `:max-tokens`, `:temperature`, and `:system`. Returns the **full accumulated** response string once streaming finishes.

Use this for responsive UIs/REPLs — show tokens as they come rather than a long
silent pause. The callback runs once per chunk (a partial fragment, not a whole
word/sentence); accumulate or render them as you like. Because the full text is also
returned, you don't need to rebuild it yourself in the callback. For a single
blocking call where you only need the final string, `llm/complete` is simpler.

```sema
;; Print chunks live, and capture the full text in `full`.
(define full
  (llm/stream "Tell me a short story"
              (fn (chunk) (display chunk))
              {:max-tokens 200}))

;; No callback: chunks stream straight to stdout, full string still returned.
(llm/stream "Count to five.")
```
