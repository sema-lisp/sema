---
name: "prompt/fill"
module: "prompt"
params: [{ name: prompt, type: prompt }, { name: vars, type: map }]
returns: "prompt"
---

Substitute `{{key}}` placeholders in every message of the prompt using the vars map (keys looked up as keywords). Slots with no matching key are left unchanged.

```sema
(prompt/fill template {:name "Ada" :topic "loops"})
```
