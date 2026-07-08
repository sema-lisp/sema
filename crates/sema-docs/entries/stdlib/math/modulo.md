---
name: "modulo"
module: "math"
section: "Scheme Aliases"
params: [{ name: a, type: number }, { name: b, type: number }]
returns: "number"
see_also: ["mod", "remainder"]
---

Alias for [`mod`](#mod): floored remainder whose sign follows the *divisor*, per R7RS (floats fall back to the truncated `%`, whose sign follows the dividend).

```sema
(modulo 10 3)  ; => 1
(modulo -7 2)  ; => 1
(modulo 7 -2)  ; => -1
```
