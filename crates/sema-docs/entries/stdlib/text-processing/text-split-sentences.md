---
name: "text/split-sentences"
module: "text-processing"
section: "Text Chunking"
params: [{ name: text, type: string }]
returns: "list"
see_also: ["text/chunk", "text/word-count", "string/words"]
---

Split `text` into a list of sentences. A sentence ends at a `.`, `!`, or `?`
that is followed by whitespace, and the terminator stays attached to its
sentence. Surrounding whitespace is trimmed from each piece; an empty string
gives an empty list.

The rule is deliberately simple and has no abbreviation dictionary, so
`"Dr. Smith"` splits after `"Dr."`, and a period followed directly by a
newline or by text without a space does not split. It is good enough for
chunking prose for embeddings or for counting sentences; for linguistically
exact segmentation use a dedicated tool.

```sema
(text/split-sentences "Hello world. How are you? Fine!")
; => ("Hello world." "How are you?" "Fine!")

(text/split-sentences "Dr. Smith went home. Then slept.")
; => ("Dr." "Smith went home." "Then slept.")

(text/split-sentences "")   ; => ()
(length (text/split-sentences "One. Two. Three."))   ; => 3
```

Combine with `text/chunk` to keep embedding chunks on sentence boundaries.
