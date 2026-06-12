---
name: "pio/side"
module: "pio"
section: "PIO Instructions"
params: [{ name: value, type: int }, { name: instr, type: map }]
returns: "map"
---

Attach a side-set `value` (0..31) to an existing PIO instruction map, setting the side-set pins in the same cycle the instruction executes. Returns a copy of `instr` with the side-set field added. The side-set width must be configured via the `:side-set-bits` option to `pio/assemble`.

```sema
(pio/side 1 (pio/nop))   ; nop while driving side-set pins to 1
```
