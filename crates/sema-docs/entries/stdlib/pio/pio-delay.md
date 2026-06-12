---
name: "pio/delay"
module: "pio"
section: "PIO Instructions"
params: [{ name: cycles, type: int }, { name: instr, type: map }]
returns: "map"
---

Attach a post-instruction delay of `cycles` (0..31) to an existing PIO instruction map, stalling the state machine for that many cycles after the instruction runs. Returns a copy of `instr` with the delay field added. The available delay range shrinks as more side-set bits are configured.

```sema
(pio/delay 5 (pio/set :pins 1))
```
