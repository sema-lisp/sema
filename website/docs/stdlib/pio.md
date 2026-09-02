---
outline: [2, 3]
---

# PIO Assembler

Build and assemble programs for the RP2040 / RP2350 **Programmable I/O** state
machines from Sema. Each `pio/*` instruction function returns an instruction
map; `pio/assemble` resolves labels and packs the program into the 16-bit words
the hardware loads. The `examples/pico-*.sema` programs use this module with
the serial port bindings to drive a Raspberry Pi Pico.

::: tip
Instruction builders are pure functions: they only build data. Nothing talks to
hardware until you send the assembled words over `serial/*` or another
transport.
:::

## Assembling a program

### `pio/assemble`

```
(pio/assemble program [config]) → map
```

`program` is a list of instruction maps interspersed with label symbols and the
`:wrap-target` / `:wrap` keyword markers. Labels are resolved, the 32-instruction
limit is enforced, and the result is a map with:

| Key | Description |
| --- | --- |
| `:instructions` | A little-endian bytevector of 16-bit instruction words |
| `:length` | Number of instructions |
| `:wrap-target`, `:wrap` | The hardware auto-loop bounds |

`config` accepts `:side-set-bits` (0..5) and `:side-set-opt` (bool), which
reserve delay/side-set bits in every instruction.

```sema
;; A blink loop: drive a pin high, then low, then jump back to 'loop
(pio/assemble
  (list 'loop
        (pio/set :pins 1)
        (pio/set :pins 0)
        (pio/jmp :always 'loop))
  {:side-set-bits 0})
; => {:instructions #u8(1 224 0 224 0 0) :length 3 :wrap 2 :wrap-target 0}
```

## Instructions

Every builder returns an instruction map for `pio/assemble`.

### `pio/jmp`

```
(pio/jmp target)        ; unconditional
(pio/jmp cond target)
```

Conditions: `:always`, `:!x`, `:x--`, `:!y`, `:y--`, `:x!=y`, `:pin`, `:!osre`.
`target` is a label symbol.

### `pio/wait`

```
(pio/wait polarity source index [:rel])
```

Stall until `source` (`:gpio`, `:pin`, `:irq`) at `index` (0..31) matches
`polarity` (0 or 1). `:rel` makes an IRQ index relative to the state machine.

### `pio/in` / `pio/out`

```
(pio/in source bits)    ; shift bits (1..32) from source into the ISR
(pio/out dest bits)     ; shift bits (1..32) from the OSR into dest
```

`in` sources: `:pins`, `:x`, `:y`, `:null`, `:isr`, `:osr`.
`out` destinations: `:pins`, `:x`, `:y`, `:null`, `:pindirs`, `:pc`, `:isr`, `:exec`.

### `pio/push` / `pio/pull`

```
(pio/push [:block | :no-block] [:iffull])
(pio/pull [:block | :no-block] [:ifempty])
```

`push` moves the ISR into the RX FIFO; `pull` moves a TX FIFO word into the
OSR. `:block` (the default) stalls when the FIFO is full/empty; `:iffull` /
`:ifempty` only transfer once the shift threshold is reached.

### `pio/mov`

```
(pio/mov dest source [op])
```

Destinations: `:pins`, `:x`, `:y`, `:exec`, `:pc`. Sources: `:pins`, `:x`, `:y`,
`:null`, `:status`, `:isr`, `:osr`. `op` is `:invert` or `:reverse`.

### `pio/irq`

```
(pio/irq mode index [:rel])
```

`mode` is `:set`, `:wait` (set and stall until cleared), or `:clear`; `index`
is an IRQ flag 0..7.

### `pio/set`

```
(pio/set dest value)
```

Write an immediate `value` (0..31) to `:pins`, `:x`, `:y`, or `:pindirs`.

### `pio/nop`

```
(pio/nop)
```

Encoded as `mov y, y`.

## Modifiers

### `pio/delay`

```
(pio/delay cycles instr)
```

Return a copy of `instr` that stalls for `cycles` (0..31) after it runs. The
available range shrinks as more side-set bits are configured.

### `pio/side`

```
(pio/side value instr)
```

Return a copy of `instr` that drives the side-set pins to `value` (0..31) in the
same cycle. Requires `:side-set-bits` in the `pio/assemble` config.

```sema
(pio/assemble
  (list 'loop
        (pio/side 1 (pio/delay 7 (pio/nop)))
        (pio/side 0 (pio/delay 7 (pio/nop)))
        (pio/jmp 'loop))
  {:side-set-bits 1})
```

## See also

- [Serial Ports](./serial) — send the assembled program to a board
- `examples/pico-blink.sema`, `examples/pico-piano.sema` — complete programs
