---
name: "hash/hmac-sha256"
module: "crypto"
section: "Hashing"
params: [{ name: key, type: string }, { name: message, type: string }]
returns: "string"
see_also: ["hash/sha256", "hash/md5", "base64/encode"]
---

Compute an HMAC-SHA256 message authentication code. Returns a 64-character hex string.

**Signature:** `(hash/hmac-sha256 key message) → string`

```sema
(hash/hmac-sha256 "secret-key" "message")
; => "hex-encoded-hmac..."
```

**Webhook verification example:**

```sema
;; Verify a webhook signature from a provider
(define (verify-webhook payload secret signature)
  (equal? (hash/hmac-sha256 secret payload) signature))
```
