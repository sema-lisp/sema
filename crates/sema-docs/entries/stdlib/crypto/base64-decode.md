---
name: "base64/decode"
module: "crypto"
section: "Base64 Encoding"
---

Decode a Base64 string back to a UTF-8 string. Errors if the decoded bytes are not valid UTF-8.

**Signature:** `(base64/decode base64-string) → string`

```sema
(base64/decode "aGVsbG8=")   ; => "hello"
```
