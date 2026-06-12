---
name: "conversation/fork"
module: "conversation"
params: [{ name: conv, type: conversation }]
returns: "conversation"
---

Return an independent copy of the conversation. Since conversations are immutable, the fork shares no future mutations and can be branched separately.

```sema
(let [branch (conversation/fork conv)]
  (conversation/say branch "Try a different approach"))
```
