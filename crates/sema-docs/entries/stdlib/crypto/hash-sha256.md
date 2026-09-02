---
name: "hash/sha256"
module: "crypto"
section: "Hashing"
params: [{ name: s, type: string }]
returns: "string"
see_also: ["hash/md5", "hash/hmac-sha256", "hash/digest"]
---

Compute the SHA-256 hash of a string. Returns a 64-character hex string.

**Signature:** `(hash/sha256 string) → string`

```sema
(hash/sha256 "hello")
; => "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
```
