---
name: "math/radians->degrees"
module: "math"
section: "Angle Conversion"
params: [{ name: radians, type: number }]
returns: "float"
see_also: ["math/degrees->radians", "pi"]
---

Convert radians to degrees.

```sema
(math/radians->degrees pi)    ; => 180.0
(math/radians->degrees 1)     ; => 57.295...
```
