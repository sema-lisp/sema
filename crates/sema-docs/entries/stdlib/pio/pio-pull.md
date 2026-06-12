---
name: "pio/pull"
module: "pio"
section: "PIO Instructions"
params: [{ name: opts, type: keyword }]
returns: "map"
---

Build an RP2040 PIO `pull` instruction that moves a word from the TX FIFO into the OSR. Accepts up to two option keywords: `:block` (default) / `:no-block` controls whether to stall when the FIFO is empty, and `:ifempty` only pulls once the OSR has reached its shift threshold.

```sema
(pio/pull :ifempty :block)
```
