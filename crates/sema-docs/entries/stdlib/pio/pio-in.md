---
name: "pio/in"
module: "pio"
section: "PIO Instructions"
params: [{ name: source, type: keyword }, { name: bits, type: int }]
returns: "map"
---

Build an RP2040 PIO `in` instruction that shifts `bits` (1..32) from `source` into the ISR. Valid sources: `:pins`, `:x`, `:y`, `:null`, `:isr`, `:osr`.

```sema
(pio/in :pins 8)  ; shift 8 bits from the input pins into the ISR
```
