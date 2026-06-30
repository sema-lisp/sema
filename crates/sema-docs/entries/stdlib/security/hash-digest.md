---
name: "hash/digest"
module: "security"
section: "Secrets & Redaction"
---

Return the lowercase hex SHA-256 digest of a string. Handy for fingerprinting redacted values so equal secrets can be correlated across records without storing the secret itself.

```sema
(hash/digest "correct horse battery staple")
```
