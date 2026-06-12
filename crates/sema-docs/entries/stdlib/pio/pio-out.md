---
name: "pio/out"
module: "pio"
section: "PIO Instructions"
params: [{ name: dest, type: keyword }, { name: bits, type: int }]
returns: "map"
---

Build an RP2040 PIO `out` instruction that shifts `bits` (1..32) out of the OSR into `dest`. Valid destinations: `:pins`, `:x`, `:y`, `:null`, `:pindirs`, `:pc`, `:isr`, `:exec`.

```sema
(pio/out :pins 1)  ; shift 1 bit from the OSR onto the output pins
```
