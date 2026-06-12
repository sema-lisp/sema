---
name: "prompt/slots"
module: "prompt"
params: [{ name: prompt, type: prompt }]
returns: "list"
---

Return the list of distinct `{{slot}}` placeholder names found across the prompt's messages, as keywords.

```sema
(prompt/slots template)   ; => (:name :topic)
```
