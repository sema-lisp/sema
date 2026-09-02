---
name: "base64/encode-bytes"
module: "crypto"
section: "Base64 Encoding"
params: [{ name: bytes, type: bytevector }]
returns: "string"
see_also: ["base64/decode-bytes", "base64/encode", "string/to-utf8"]
---

Encode a bytevector to Base64.

**Signature:** `(base64/encode-bytes bytevector) → string`

```sema
(base64/encode-bytes #u8(104 101 108 108 111))   ; => "aGVsbG8="
```
