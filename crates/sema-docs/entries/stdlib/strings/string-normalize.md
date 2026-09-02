---
name: "string/normalize"
module: "strings"
section: "Unicode & Encoding"
params: [{ name: s, type: string }, { name: form, type: keyword, doc: "one of :nfc, :nfd, :nfkc, :nfkd (keyword or string)" }]
returns: "string"
see_also: ["string/foldcase", "string/codepoints", "string/length"]
---

Normalize a string to a Unicode normalization form. Supported forms: `:nfc`, `:nfd`, `:nfkc`, `:nfkd` (as keywords or strings).

- **NFC** — Canonical Decomposition, followed by Canonical Composition (most common)
- **NFD** — Canonical Decomposition
- **NFKC** — Compatibility Decomposition, followed by Canonical Composition
- **NFKD** — Compatibility Decomposition

```sema
;; NFC: combine decomposed characters
;; e + combining acute accent → é
(string/normalize "e\u0301" :nfc)    ; => "é"

;; NFD: decompose composed characters
(string/length (string/normalize "é" :nfd))  ; => 2 (e + combining accent)

;; NFKC/NFKD: compatibility decomposition (ligatures, etc.)
(string/normalize "\uFB01" :nfkc)    ; => "fi" (ﬁ ligature → two letters)

;; String form names also work
(string/normalize "e\u0301" "NFC")   ; => "é"
```
