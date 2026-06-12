---
name: "pio/jmp"
module: "pio"
section: "PIO Instructions"
params: [{ name: cond, type: keyword }, { name: target, type: symbol }]
returns: "map"
---

Build an RP2040 PIO `jmp` instruction. Called with a single `target` symbol it is an unconditional jump (`:always`); called with `(pio/jmp cond target)` it jumps only when the condition holds. Valid conditions: `:always`, `:!x`, `:x--`, `:!y`, `:y--`, `:x!=y`, `:pin`, `:!osre`. `target` is a label symbol resolved at assembly time.

```sema
(pio/jmp :x-- 'loop)
```
