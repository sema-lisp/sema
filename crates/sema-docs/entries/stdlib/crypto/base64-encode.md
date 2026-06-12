---
name: "base64/encode"
module: "crypto"
section: "Base64 Encoding"
---

Encode a string to Base64.

**Signature:** `(base64/encode string) → string`

```sema
(base64/encode "hello")   ; => "aGVsbG8="
(base64/encode "")        ; => ""
```
