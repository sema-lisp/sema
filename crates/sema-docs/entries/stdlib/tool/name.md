---
name: "tool/name"
module: "tool"
params: [{ name: tool, type: tool }]
returns: "string"
---

Return the name of a tool definition.

```sema
(tool/name get-weather)   ; => "get-weather"
```
