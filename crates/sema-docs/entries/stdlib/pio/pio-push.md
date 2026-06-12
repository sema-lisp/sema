---
name: "pio/push"
module: "pio"
section: "PIO Instructions"
params: [{ name: opts, type: keyword }]
returns: "map"
---

Build an RP2040 PIO `push` instruction that moves the ISR contents into the RX FIFO. Accepts up to two option keywords: `:block` (default) / `:no-block` controls whether to stall when the FIFO is full, and `:iffull` only pushes once the ISR has reached its shift threshold.

```sema
(pio/push :iffull :block)
```
