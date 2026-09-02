---
name: "text/excerpt"
module: "text-processing"
section: "Text Cleaning"
params: [{ name: text, type: string }, { name: query, type: string }, { name: opts, type: map, doc: "optional; {:radius n :omission str}" }]
returns: "string or nil"
see_also: ["text/truncate", "string/index-of", "string/between"]
---

Extract a snippet around a search term with omission markers. Case-insensitive search. Returns `nil` if query not found.

```sema
(text/excerpt "The quick brown fox jumps over the lazy dog" "fox" {:radius 10})
; => "...ick brown fox jumps ove..."

(text/excerpt "Hello world" "Hello")
; => "Hello world"

;; Custom omission marker
(text/excerpt "Long text here..." "text" {:radius 5 :omission "[…]"})
; => "Long text here[…]"
```

Options map (optional third argument):

- `:radius` — number of characters to show on each side (default: 100)
- `:omission` — marker string for truncated parts (default: `"..."`)
