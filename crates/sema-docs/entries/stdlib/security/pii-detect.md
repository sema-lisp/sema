---
name: "pii/detect"
module: "security"
section: "Secrets & Redaction"
---

Scan text for personally-identifying data and return a list of maps, one per match, each with `:type`, `:match`, `:start`, and `:end`. Detects email addresses (`:type "email"`), IPv4 addresses (`:type "ipv4"`), and US-style phone numbers (`:type "phone"`). Pair the returned spans with `redact/spans` to scrub the original text.

```sema
(pii/detect "email me@example.com or call (415) 555-2671")
```
