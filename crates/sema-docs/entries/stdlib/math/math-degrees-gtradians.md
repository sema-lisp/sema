---
name: "math/degrees->radians"
module: "math"
section: "Angle Conversion"
params: [{ name: degrees, type: number }]
returns: "float"
see_also: ["math/radians->degrees", "pi"]
---

Convert degrees to radians.

```sema
(math/degrees->radians 180)   ; => 3.14159...
(math/degrees->radians 90)    ; => 1.5707...
```
