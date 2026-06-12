---
name: "base64/encode-bytes"
module: "crypto"
section: "Base64 Encoding"
---

Encode a bytevector to Base64.

**Signature:** `(base64/encode-bytes bytevector) → string`

```sema
(base64/encode-bytes #u8(104 101 108 108 111))   ; => "aGVsbG8="
```
