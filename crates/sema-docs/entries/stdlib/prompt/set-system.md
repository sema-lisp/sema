---
name: "prompt/set-system"
module: "prompt"
params: [{ name: prompt, type: prompt }, { name: system, type: string }]
returns: "prompt"
---

Return a new prompt with its system message set to the given string. Any existing system messages are removed and the new one is placed first.

```sema
(prompt/set-system my-prompt "You are a careful code reviewer.")
```
