---
name: "pio/mov"
module: "pio"
section: "PIO Instructions"
params: [{ name: dest, type: keyword }, { name: source, type: keyword }, { name: op, type: keyword }]
returns: "map"
---

Build an RP2040 PIO `mov` instruction copying `source` to `dest`. Valid destinations: `:pins`, `:x`, `:y`, `:exec`, `:pc`. Valid sources: `:pins`, `:x`, `:y`, `:null`, `:status`, `:isr`, `:osr`. An optional third `op` keyword applies a transform: `:invert` (bitwise NOT) or `:reverse` (bit-reverse).

```sema
(pio/mov :x :osr)            ; copy OSR into X
(pio/mov :y :x :invert)      ; copy ~X into Y
```
