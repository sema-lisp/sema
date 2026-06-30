---
name: "redact/spans"
module: "security"
section: "Secrets & Redaction"
---

Replace explicit byte-offset spans in a string with redaction markers. Takes the text and a list of maps `{:start <int> :end <int>}`, each optionally carrying a `:label`. A labeled span becomes `«redacted:<label>»`, an unlabeled one becomes `«redacted»`. Offsets are clamped to the string and validated against char boundaries; inverted, empty, out-of-range, or non-map entries are skipped. Edits apply right-to-left so earlier offsets remain valid.

```sema
(redact/spans "hello world"
  (list {:start 6 :end 11 :label "name"}))
```
