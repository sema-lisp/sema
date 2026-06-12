---
name: "pio/irq"
module: "pio"
section: "PIO Instructions"
params: [{ name: mode, type: keyword }, { name: index, type: int }, { name: rel, type: keyword }]
returns: "map"
---

Build an RP2040 PIO `irq` instruction that operates on IRQ flag `index` (0..7). `mode` is one of `:set` (raise the flag), `:wait` (raise it and stall until cleared), or `:clear` (lower it). Pass an optional trailing `:rel` keyword to make the index relative to the state machine.

```sema
(pio/irq :set 0)
(pio/irq :wait 1 :rel)
```
