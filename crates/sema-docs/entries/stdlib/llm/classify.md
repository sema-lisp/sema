---
name: "llm/classify"
module: "llm"
params: [{ name: categories }, { name: text, type: string }, { name: opts, type: map }]
returns: "keyword or string"
---

Classify text into exactly one of the given categories. The first argument is a sequence of category names (keywords or strings); the model is told to respond with only the matching category name. Returns a keyword if the matched category was supplied as a keyword, otherwise a string. The opts map accepts `:model`.

```sema
(llm/classify [:positive :negative :neutral] "I absolutely love this product!")
; => :positive
```
