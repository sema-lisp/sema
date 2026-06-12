---
name: "text/excerpt"
module: "text-processing"
section: "Text Cleaning"
---

Extract a snippet around a search term with omission markers. Case-insensitive search. Returns `nil` if query not found.

```sema
(text/excerpt "The quick brown fox jumps over the lazy dog" "fox" {:radius 10})
; => "...brown fox jumps ov..."

(text/excerpt "Hello world" "Hello")
; => "Hello world"

;; Custom omission marker
(text/excerpt "Long text here..." "text" {:radius 5 :omission "[…]"})
; => "[…]g text here[…]"
```

Options map (optional third argument):

- `:radius` — number of characters to show on each side (default: 100)
- `:omission` — marker string for truncated parts (default: `"..."`)
