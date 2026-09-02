---
name: "base64/decode"
module: "crypto"
section: "Base64 Encoding"
params: [{ name: base64-string, type: string }]
returns: "string"
see_also: ["base64/encode", "base64/decode-bytes", "string/to-utf8"]
---

Decode a Base64 string back to a UTF-8 string. Errors if the decoded bytes are not valid UTF-8.

**Signature:** `(base64/decode base64-string) → string`

```sema
(base64/decode "aGVsbG8=")   ; => "hello"
```
