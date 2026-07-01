---
name: "secret/redact"
module: "security"
section: "Secrets & Redaction"
---

Return a copy of the text with every secret found by `secret/detect` replaced by a `«redacted:<type>»` marker. Replacement is applied from the rightmost match to the leftmost so byte offsets stay valid as edits are made. Useful for sanitizing logs, prompts, or LLM context before they leave the process.

```sema
(secret/redact "deploy key AKIAIOSFODNN7EXAMPLE to prod")
```
