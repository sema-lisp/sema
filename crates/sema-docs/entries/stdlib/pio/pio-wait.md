---
name: "pio/wait"
module: "pio"
section: "PIO Instructions"
params: [{ name: polarity, type: int }, { name: source, type: keyword }, { name: index, type: int }, { name: rel, type: keyword }]
returns: "map"
---

Build an RP2040 PIO `wait` instruction that stalls until `source` at `index` matches `polarity` (0 or 1). Valid sources: `:gpio`, `:pin`, `:irq`. `index` is in the range 0..31. Pass an optional trailing `:rel` keyword to make an IRQ index relative to the state machine.

```sema
(pio/wait 1 :pin 0)         ; wait for pin 0 to go high
(pio/wait 1 :irq 0 :rel)    ; wait for relative IRQ 0
```
