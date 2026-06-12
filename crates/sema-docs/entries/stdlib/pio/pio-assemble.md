---
name: "pio/assemble"
module: "pio"
section: "PIO Programs"
params: [{ name: program, type: list }, { name: config, type: map }]
returns: "map"
---

Assemble a list of PIO instruction maps (interspersed with label symbols and the `:wrap-target` / `:wrap` keyword markers) into a binary program. Resolves labels, enforces the 32-instruction limit, and returns a map with `:instructions` (a little-endian bytevector of 16-bit words), `:length`, `:wrap-target`, and `:wrap`. The optional `config` map accepts `:side-set-bits` (0..5) and `:side-set-opt` (bool), which reserve delay/side-set bits accordingly.

```sema
(pio/assemble
  (list 'loop
        (pio/set :pins 1)
        (pio/set :pins 0)
        (pio/jmp :always 'loop))
  {:side-set-bits 0})
```
