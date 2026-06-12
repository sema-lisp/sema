---
name: "uuid/v4"
module: "crypto"
section: "UUID"
---

Generate a random UUID v4 string.

**Signature:** `(uuid/v4) → string`

```sema
(uuid/v4)   ; => "550e8400-e29b-41d4-a716-446655440000" (varies)
```

Each call returns a new unique identifier:

```sema
(equal? (uuid/v4) (uuid/v4))   ; => #f
```
