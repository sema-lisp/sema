---
name: "hash/md5"
module: "crypto"
section: "Hashing"
---

Compute the MD5 hash of a string. Returns a 32-character hex string.

**Signature:** `(hash/md5 string) → string`

```sema
(hash/md5 "hello")   ; => "5d41402abc4b2a76b9719d911017c592"
```
