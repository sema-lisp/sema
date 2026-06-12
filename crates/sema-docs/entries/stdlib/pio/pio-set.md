---
name: "pio/set"
module: "pio"
section: "PIO Instructions"
params: [{ name: dest, type: keyword }, { name: value, type: int }]
returns: "map"
---

Build an RP2040 PIO `set` instruction that writes an immediate `value` (0..31) to `dest`. Valid destinations: `:pins`, `:x`, `:y`, `:pindirs`.

```sema
(pio/set :pins 1)   ; drive the set pins high
(pio/set :x 0)      ; load 0 into X
```
