---
name: "tool/description"
module: "tool"
params: [{ name: tool, type: tool }]
returns: "string"
---

Return the description of a tool definition.

```sema
(tool/description get-weather)   ; => "Fetch the current weather for a city"
```
