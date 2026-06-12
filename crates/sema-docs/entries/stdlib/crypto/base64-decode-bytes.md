---
name: "base64/decode-bytes"
module: "crypto"
section: "Base64 Encoding"
---

Decode a Base64 string to a bytevector. Unlike `base64/decode`, this does not require valid UTF-8.

**Signature:** `(base64/decode-bytes base64-string) → bytevector`

```sema
(base64/decode-bytes "aGVsbG8=")   ; => #u8(104 101 108 108 111)
```
