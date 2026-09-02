---
name: "hash/md5"
module: "crypto"
section: "Hashing"
params: [{ name: s, type: string }]
returns: "string"
see_also: ["hash/sha256", "hash/hmac-sha256", "hash/digest"]
---

Compute the MD5 hash of a string. Returns a 32-character hex string.

**Signature:** `(hash/md5 string) → string`

```sema
(hash/md5 "hello")   ; => "5d41402abc4b2a76b9719d911017c592"
```
