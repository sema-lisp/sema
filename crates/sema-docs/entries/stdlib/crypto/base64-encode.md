---
name: "base64/encode"
module: "crypto"
section: "Base64 Encoding"
params: [{ name: s, type: string }]
returns: "string"
see_also: ["base64/decode", "base64/encode-bytes", "hash/sha256"]
---

Encode a string to Base64.

**Signature:** `(base64/encode string) → string`

```sema
(base64/encode "hello")   ; => "aGVsbG8="
(base64/encode "")        ; => ""
```
