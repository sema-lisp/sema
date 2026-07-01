---
outline: [2, 3]
---

# Secrets & Redaction

Detect and redact secrets and PII in text — useful before logging or sending
data to an LLM.

```sema
(secret/detect "key AKIA... and tok eyJ...")  ; list of {:type :match :start :end}
(secret/redact text)               ; => text with secrets → «redacted:<type>»
(pii/detect text)                  ; emails, IPv4, phone numbers
(redact/spans text spans)          ; redact caller-supplied {:start :end :label} ranges
(hash/digest text)                 ; SHA-256 hex (fingerprint a redacted value)
```
