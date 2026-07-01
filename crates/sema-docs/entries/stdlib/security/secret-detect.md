---
name: "secret/detect"
module: "security"
section: "Secrets & Redaction"
---

Scan text for embedded credentials and return a list of maps, one per match, each with `:type`, `:match`, `:start`, and `:end` (byte offsets). Detects AWS access key ids, JWTs, Slack and GitHub tokens, PEM private-key blocks, generic `key = value` assignments, and bare high-entropy blobs. The generic and high-entropy matchers are gated on Shannon entropy (~3.5 bits/char) to suppress false positives on ordinary identifiers.

```sema
(secret/detect "AWS key AKIAIOSFODNN7EXAMPLE here")
```
