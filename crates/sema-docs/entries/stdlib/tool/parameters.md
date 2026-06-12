---
name: "tool/parameters"
module: "tool"
params: [{ name: tool, type: tool }]
returns: "map"
---

Return the parameter schema of a tool definition (the map describing the tool's accepted arguments).

```sema
(tool/parameters get-weather)
```
