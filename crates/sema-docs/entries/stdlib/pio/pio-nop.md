---
name: "pio/nop"
module: "pio"
section: "PIO Instructions"
returns: "map"
---

Build an RP2040 PIO `nop` instruction (encoded as `mov y, y`, which has no side effects). Returns an instruction map for use in a program passed to `pio/assemble`.

```sema
(pio/nop)  ; => an instruction map
```
