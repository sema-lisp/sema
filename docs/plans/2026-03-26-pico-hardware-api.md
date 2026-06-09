# Sema Pico Hardware API Design

**Date:** 2026-03-26
**Status:** On Hold (2026-06-09) — PIO DSL shipped host-side (`crates/sema-stdlib/src/pio.rs`); native gpio/adc/pwm/i2c API deferred until the on-device Pico port exists. Interim hardware control works via the serial bridge (`serial/send` + MicroPython, see `examples/pico-blink.sema`).
**Depends on:** [Raspberry Pi Pico Port](2026-03-25-raspberry-pi-pico-port.md) (Phase 1–4)

## Overview

Design the hardware interaction API for Sema running on Raspberry Pi Pico 2 (RP2350). These are native functions implemented in Rust wrapping `rp2350-hal` (blocking HAL), registered into the Sema global environment under slash-namespaced names following Decision #24.

**Architecture:** Host-compile `.sema` → `.semac`, device-run on Pico. Hardware native functions are only available on the embedded target (`#[cfg(feature = "rp2350")]`).

## Prior Art Surveyed

| Framework | Language | Style | Key Takeaway |
|-----------|----------|-------|--------------|
| **Arduino** | C++ | Flat functions (`pinMode`, `digitalWrite`) | 300+ functions, de-facto standard API names |
| **MicroPython** | Python | Class-based (`machine.Pin`, `machine.I2C`) | Clean object model, great for Pico |
| **CircuitPython** | Python | Board-centric modules (`digitalio`, `busio`) | Hardware abstraction via board aliases |
| **uLisp** | Lisp | Functional, Arduino-legacy (`pinmode`, `digitalwrite`) | `with-i2c` / `with-spi` resource macros |
| **Espruino** | JavaScript | Event-driven (`setWatch`, `setInterval`) | Power-efficient interrupt model |
| **NodeMCU Lua** | Lua | Module-based (`gpio.mode`, `i2c.setup`) | Clean namespace separation |
| **Rust embedded-hal** | Rust | Trait-based (`InputPin`, `OutputPin`, `I2c`) | What we wrap underneath |

**Key design influences:**
- uLisp's `with-i2c` / `with-spi` macros → our `with-*` macros
- Espruino's `setWatch` + event queue → our `irq/watch` + `irq/poll`
- Sema's own slash-namespace convention → `gpio/write`, `i2c/read`

## Naming Conventions

Follows Sema Decision #24 strictly:

- **Slash-namespaced:** `gpio/write`, `i2c/read`, `pwm/set-duty`
- **Predicates end in `?`:** `gpio/high?`, `uart/available?`, `wifi/connected?`
- **Keywords for constants:** `:input`, `:output`, `:pull-up`, `:rising`, `:falling`, `:high`, `:low`
- **Keywords for config maps:** `{:mode :mode-0 :timeout 500 :debounce 50}`

## Tier 1: Essentials

*Must-have for any useful embedded work. Unlocks: blink, button, analog sensor, LED dimming, buzzer.*

### gpio/ — Digital I/O (7 functions)

| Function | Signature | Returns | Description |
|----------|-----------|---------|-------------|
| `gpio/init` | `(gpio/init pin mode)` or `(gpio/init pin mode pull)` | `:ok` | Configure pin. Modes: `:input`, `:output`, `:open-drain`. Pulls: `:pull-up`, `:pull-down`, `:none`. Claims pin in registry. |
| `gpio/write` | `(gpio/write pin value)` | `:ok` | Set output. Value: `:high`/`:low`, `true`/`false`, `1`/`0`. |
| `gpio/read` | `(gpio/read pin)` | `true`/`false` | Read input level. |
| `gpio/high?` | `(gpio/high? pin)` | `true`/`false` | Predicate: is pin high? |
| `gpio/low?` | `(gpio/low? pin)` | `true`/`false` | Predicate: is pin low? |
| `gpio/toggle` | `(gpio/toggle pin)` | `:ok` | Toggle output. |
| `gpio/release` | `(gpio/release pin)` | `:ok` | Release pin back to unclaimed state. |

**Rust wraps:** `DynPin` from rp2350-hal (type-erased, runtime-checked pin operations).

**Example:**
```scheme
(gpio/init 25 :output)
(loop
  (gpio/toggle 25)
  (time/sleep-ms 500))
```

### time/ — Delays & Measurement (5 functions)

| Function | Signature | Returns | Description |
|----------|-----------|---------|-------------|
| `time/sleep-ms` | `(time/sleep-ms ms)` | `:ok` | Blocking delay, milliseconds. |
| `time/sleep-us` | `(time/sleep-us us)` | `:ok` | Blocking delay, microseconds. |
| `time/ticks` | `(time/ticks)` | Integer | Microseconds since boot (64-bit, wraps after ~585K years). |
| `time/ticks-ms` | `(time/ticks-ms)` | Integer | Milliseconds since boot. |
| `time/pulse-in` | `(time/pulse-in pin level)` or `(time/pulse-in pin level timeout-us)` | Integer or `nil` | Measure pulse duration in µs. Default timeout 1s. |

**Rust wraps:** RP2350 64-bit microsecond timer via `timer.get_counter().ticks()`, `DelayNs` trait.

**Note:** `time/pulse-in` is in `time/` not `gpio/` because it measures time, takes a timeout, and returns microseconds. The pin is just the signal source.

### adc/ — Analog Input (3 functions)

| Function | Signature | Returns | Description |
|----------|-----------|---------|-------------|
| `adc/read` | `(adc/read pin)` | Integer 0–4095 | 12-bit ADC read. Pins: GP26 (ADC0), GP27 (ADC1), GP28 (ADC2). |
| `adc/read-voltage` | `(adc/read-voltage pin)` | Float 0.0–3.3 | Convenience: reads and converts to volts. |
| `adc/read-temp` | `(adc/read-temp)` | Float | Internal die temperature in °C. No pin needed (ADC channel 4). |

**Rust wraps:** `rp2350_hal::Adc` + `AdcPin`.

### pwm/ — Pulse Width Modulation (4 functions)

| Function | Signature | Returns | Description |
|----------|-----------|---------|-------------|
| `pwm/init` | `(pwm/init pin freq-hz)` | `:ok` | Init PWM at frequency. Duty starts at 0%. |
| `pwm/set-duty` | `(pwm/set-duty pin duty)` | `:ok` | Set duty cycle: float 0.0–1.0 or integer 0–65535. |
| `pwm/set-freq` | `(pwm/set-freq pin freq-hz)` | `:ok` | Change frequency, preserve duty ratio. |
| `pwm/stop` | `(pwm/stop pin)` | `:ok` | Disable PWM, release pin. |

**Rust wraps:** RP2350 PWM slices (8 slices × 2 channels). `SetDutyCycle` trait.

**Example — LED breathing:**
```scheme
(pwm/init 25 1000)
(loop
  (for-each (fn [b] (pwm/set-duty 25 (/ b 100.0)) (time/sleep-ms 10))
    (range 0 101))
  (for-each (fn [b] (pwm/set-duty 25 (/ b 100.0)) (time/sleep-ms 10))
    (range 100 -1 -1)))
```

**Tier 1 total: 19 functions.** Unlocks projects: blink, traffic light, button input, potentiometer read, LED dimming, buzzer tones, servo (via raw PWM).

---

## Tier 2: Very Nice to Have

*Unlocks most real-world projects: sensors, displays, GPS, inter-board communication.*

### i2c/ — I2C Bus (8 functions)

| Function | Signature | Returns | Description |
|----------|-----------|---------|-------------|
| `i2c/init` | `(i2c/init bus-id sda scl speed)` | Bus handle | bus-id: 0/1. Speed: `:standard` (100k), `:fast` (400k), `:fast-plus` (1M), or Hz integer. |
| `i2c/scan` | `(i2c/scan bus)` | List of integers | Scan for ACK'ing addresses (0x08–0x77). |
| `i2c/write` | `(i2c/write bus addr data)` | `:ok` | Write bytevector to device. |
| `i2c/read` | `(i2c/read bus addr count)` | Bytevector | Read count bytes from device. |
| `i2c/write-read` | `(i2c/write-read bus addr write-data read-count)` | Bytevector | Combined write-then-read (no STOP between). Standard register-read pattern. |
| `i2c/reg-read` | `(i2c/reg-read bus addr register count)` | Bytevector | Convenience: `(i2c/write-read bus addr (bytevector register) count)`. |
| `i2c/reg-write` | `(i2c/reg-write bus addr register data)` | `:ok` | Convenience: write register byte + data. |
| `i2c/release` | `(i2c/release bus)` | `:ok` | Release bus and its pins. |

**Data format:** All I2C functions use Sema's existing `bytevector` type (from `sema-stdlib/src/bytevector.rs`).

### spi/ — SPI Bus (5 functions)

| Function | Signature | Returns | Description |
|----------|-----------|---------|-------------|
| `spi/init` | `(spi/init bus-id sck mosi miso speed-hz)` or with opts map | Bus handle | CS managed manually via GPIO. Opts: `{:mode :mode-0 :bit-order :msb-first}`. |
| `spi/transfer` | `(spi/transfer bus tx-data)` | Bytevector | Full-duplex: sends tx-data, returns received bytes. |
| `spi/write` | `(spi/write bus data)` | `:ok` | Write-only (discards MISO). |
| `spi/read` | `(spi/read bus count)` | Bytevector | Read-only (sends zeros). |
| `spi/release` | `(spi/release bus)` | `:ok` | Release bus and pins. |

### uart/ — Serial UART (6 functions)

| Function | Signature | Returns | Description |
|----------|-----------|---------|-------------|
| `uart/init` | `(uart/init bus-id tx rx baud)` or with opts map | Port handle | Opts: `{:data-bits 8 :stop-bits 1 :parity :none}`. |
| `uart/write` | `(uart/write port data)` | Integer (bytes written) | Write bytevector or string. |
| `uart/read` | `(uart/read port count)` or `(uart/read port count timeout-ms)` | Bytevector or `nil` | Read up to count bytes. Default timeout 1000ms. |
| `uart/available?` | `(uart/available? port)` | Integer | Bytes waiting in RX buffer. |
| `uart/read-line` | `(uart/read-line port)` or with timeout | String or `nil` | Read until newline. |
| `uart/release` | `(uart/release port)` | `:ok` | Release port and pins. |

### irq/ — Interrupts & Callbacks (7 functions)

| Function | Signature | Returns | Description |
|----------|-----------|---------|-------------|
| `irq/watch` | `(irq/watch pin edge callback)` or with opts | Watch handle | Edge: `:rising`, `:falling`, `:both`. Opts: `{:debounce 50}`. Callback receives `{:pin N :edge :falling :tick T}`. |
| `irq/remove` | `(irq/remove handle)` | `:ok` | Remove a watch or timer. |
| `irq/timer` | `(irq/timer interval-ms callback)` or `(irq/timer interval-ms callback :once)` | Timer handle | Periodic (default) or one-shot timer. |
| `irq/poll` | `(irq/poll)` | Integer | Process pending interrupt events, dispatch callbacks. Returns count processed. **Must be called in main loop.** |
| `irq/wait` | `(irq/wait)` or `(irq/wait timeout-ms)` | `true`/`false` | WFI sleep until interrupt, then poll. **Always provide a timeout in production code.** |
| `irq/debounce` | Merged into `irq/watch` | — | Use `(irq/watch pin edge callback {:debounce 50})` instead of a separate function. |
| `irq/dropped-count` | `(irq/dropped-count)` | Integer | Events lost due to queue overflow since boot. |

**Interrupt architecture:**

```
ISR (hardware context)          Sema evaluator (main thread)
  │                                │
  ├─ GPIO edge detected            │
  ├─ Write event to ring buffer    │
  │   (heapless::spsc::Queue       │
  │    single-producer: the ISR)   │
  │                                │
  │                                ├─ (irq/poll) drains buffer
  │                                ├─ Matches events to callbacks
  │                                ├─ Calls Sema closures via call_callback
  │                                │
```

**Critical:** Single ISR handler (`IO_IRQ_BANK0`) is the sole producer into the ring buffer. Multiple GPIO pins funnel through this one handler. This satisfies the SPSC constraint.

**Tier 2 total: 26 functions.** Unlocks: I2C sensors (BMP280, MPU6050, SSD1306), SPI displays, GPS (UART), Bluetooth modules, event-driven button handling.

---

## Tier 3: Nice to Have

*Higher-level peripheral drivers for common components.*

### neo/ — NeoPixel / WS2812 (6 functions)

**Dependency: Requires PIO support internally.** The NeoPixel timing protocol (800KHz, precise bit encoding) cannot be reliably bit-banged from software. The implementation uses a PIO state machine under the hood, but the PIO API is not exposed to users until Tier 4. Neo/ is a high-level wrapper.

| Function | Signature | Returns | Description |
|----------|-----------|---------|-------------|
| `neo/init` | `(neo/init pin count)` or with opts `{:type :grb}` | Strip handle | Types: `:grb` (WS2812), `:rgb`, `:grbw` (SK6812). |
| `neo/set` | `(neo/set strip index r g b)` or `(neo/set strip index 0xRRGGBB)` | `:ok` | Set pixel color. Does not update strip. |
| `neo/fill` | `(neo/fill strip r g b)` or `(neo/fill strip color)` | `:ok` | Set all pixels. |
| `neo/show` | `(neo/show strip)` | `:ok` | Push buffer to LEDs via PIO. |
| `neo/brightness` | `(neo/brightness strip level)` | `:ok` | Global brightness 0–255. |
| `neo/release` | `(neo/release strip)` | `:ok` | Release PIO state machine and pin. |

### servo/ — Servo Motors (4 functions)

| Function | Signature | Returns | Description |
|----------|-----------|---------|-------------|
| `servo/init` | `(servo/init pin)` or with opts `{:min-us 500 :max-us 2500}` | Servo handle | Configures PWM at 50Hz. |
| `servo/write` | `(servo/write handle angle)` | `:ok` | Set angle 0–180 degrees. |
| `servo/write-us` | `(servo/write-us handle pulse-us)` | `:ok` | Set raw pulse width in microseconds. |
| `servo/detach` | `(servo/detach handle)` | `:ok` | Stop PWM, release pin. |

### oled/ — SSD1306 OLED Display (11 functions)

Uses `embedded-graphics` crate for drawing primitives. Framebuffer is 1KB for 128×64.

| Function | Signature | Returns | Description |
|----------|-----------|---------|-------------|
| `oled/init` | `(oled/init i2c-bus addr width height)` | Display handle | Common: addr 0x3C, 128×64. |
| `oled/clear` | `(oled/clear display)` | `:ok` | Clear framebuffer. |
| `oled/pixel` | `(oled/pixel display x y color)` | `:ok` | Set pixel. Color: 1 (on) or 0 (off). |
| `oled/text` | `(oled/text display x y string)` or with opts `{:size 2}` | `:ok` | Draw text (built-in 6×8 font). |
| `oled/line` | `(oled/line display x0 y0 x1 y1)` | `:ok` | Draw line. |
| `oled/rect` | `(oled/rect display x y w h)` or with `:fill` | `:ok` | Rectangle, optionally filled. |
| `oled/circle` | `(oled/circle display cx cy r)` or with `:fill` | `:ok` | Circle, optionally filled. |
| `oled/show` | `(oled/show display)` | `:ok` | Flush framebuffer to display over I2C. |
| `oled/invert` | `(oled/invert display on?)` | `:ok` | Invert display colors. |
| `oled/scroll` | `(oled/scroll display direction)` | `:ok` | Hardware scroll: `:left`, `:right`, `:off`. |
| `oled/release` | `(oled/release display)` | `:ok` | Release display handle. |

### tone/ — Buzzer (3 functions)

| Function | Signature | Returns | Description |
|----------|-----------|---------|-------------|
| `tone/play` | `(tone/play pin freq-hz)` or `(tone/play pin freq-hz duration-ms)` | `:ok` | Square wave via PWM. With duration: blocks then stops. |
| `tone/stop` | `(tone/stop pin)` | `:ok` | Stop tone. |
| `tone/note` | `(tone/note :A4)` | Integer (Hz) | Note name to frequency. `:A4`→440, `:C4`→262, `:Fs5`→740. |

**Tier 3 total: 24 functions.** Unlocks: NeoPixel animations, servo robots, OLED dashboards, musical projects.

---

## Tier 4: Future / Planned

Deferred until Tier 1–3 are solid.

| Namespace | Functions | Depends on |
|-----------|-----------|------------|
| `wifi/` | init, connect, connected?, disconnect, ip, rssi, scan, http-get, http-post, serve | Pico 2 W, CYW43 driver |
| `pio/` | init, load-program, sm-init, sm-start, sm-stop, sm-put, sm-get, release | Core PIO HAL support |
| `core/` | launch, fifo-write, fifo-read, fifo-available?, id, stop | Dual-core support, requires Arc |
| `sd/` | init, read-file, write-file, exists?, list, release | SPI + FAT filesystem |
| `ble/` | init, advertise, scan, connect, read, write | Pico 2 W, BLE stack |
| `i2s/` | init, write, stop, release | PIO-based I2S |

---

## Resource Management Macros

Prelude macros using `try`/`catch` for guaranteed cleanup. Based on uLisp's `with-i2c` pattern.

```scheme
;; with-i2c — auto-init and release I2C bus
(with-i2c [bus 0 0 1 :fast]
  (def devices (i2c/scan bus))
  (println (str "Found " (length devices) " devices")))
;; bus released here, even on error

;; with-spi — auto-init and release SPI bus
(with-spi [bus 0 18 19 16 1000000]
  (gpio/write cs :low)
  (spi/write bus data)
  (gpio/write cs :high))

;; with-pins — init multiple GPIO pins, release all on exit
(with-pins [(led 25 :output) (btn 15 :input :pull-up)]
  (loop
    (gpio/write led (gpio/low? btn))
    (time/sleep-ms 10)))
;; GP25 and GP15 released here

;; Also: with-uart, with-neo, with-oled
```

**Implementation:** Expands to `let` + `try`/`catch` + release call. See [Feasibility: with-* macros](#with-macros-feasibility) for the exact expansion.

---

## Board Aliases

```scheme
(board/pin :led)        ;; => 25 (onboard LED)
(board/pin :adc0)       ;; => 26
(board/pin :i2c0-sda)   ;; => 0  (default I2C0 pins)
(board/pin :i2c0-scl)   ;; => 1
(board/pin :uart0-tx)   ;; => 0
(board/pin :uart0-rx)   ;; => 1

(board/info)
;; => {:name "Raspberry Pi Pico 2" :chip "RP2350" :sram 524288 :flash 4194304}
```

---

## Error Handling

All hardware errors throw `SemaError` with keyword types. Sema's existing `try`/`catch` handles recovery.

| Error Keyword | Cause |
|---------------|-------|
| `:invalid-pin` | Pin number out of range (0–29) |
| `:pin-in-use` | Pin claimed by another peripheral |
| `:invalid-mode` | Unrecognized keyword for config |
| `:i2c-nack` | No acknowledge from I2C device |
| `:i2c-timeout` | I2C transfer timed out |
| `:spi-error` | SPI transfer failure |
| `:uart-overrun` | UART buffer overrun |
| `:timeout` | Generic timeout (pulse-in, read, etc.) |
| `:hw-fault` | Catch-all HAL error |

```scheme
(try
  (i2c/write bus 0x50 data)
  (catch e
    (when (= (error/type e) :i2c-nack)
      (println "Device not responding"))))
```

---

## Implementation Architecture

### Peripheral State Management

```rust
// Stored in EvalContext, feature-gated
#[cfg(feature = "rp2350")]
struct PeripheralManager {
    pins: [Option<DynPin>; 30],
    pin_owners: [Option<PinOwner>; 30],  // tracks who claimed each pin
    i2c: [Option<I2cBus>; 2],
    spi: [Option<SpiBus>; 2],
    uart: [Option<UartPort>; 2],
    pwm_slices: [Option<PwmSlice>; 8],
    adc: Option<Adc>,
    timer: Timer,
    interrupt_queue: spsc::Consumer<Event, 32>,
    callbacks: CallbackRegistry,
}
```

Accessed via `EvalContext` → `peripherals: RefCell<Option<Box<dyn Any>>>` → downcast to `PeripheralManager`.

### Pin Type-State Solution

Use `DynPin` from rp2350-hal. All pins stored as type-erased `DynPin` indexed by pin number. Runtime mode changes via `try_into_function()`. The pin registry (`pin_owners` array) prevents conflicts between GPIO, I2C, SPI, UART, and PWM.

```rust
fn claim_pin(mgr: &mut PeripheralManager, pin: u8, owner: PinOwner) -> Result<(), SemaError> {
    if let Some(existing) = mgr.pin_owners[pin as usize] {
        return Err(SemaError::hardware(":pin-in-use",
            &format!("GP{} already claimed by {:?}", pin, existing))
            .with_hint("Call gpio/release or release the owning peripheral first"));
    }
    mgr.pin_owners[pin as usize] = Some(owner);
    Ok(())
}
```

### Interrupt Queue

Lock-free SPSC ring buffer from `heapless` crate. Single producer: the `IO_IRQ_BANK0` ISR handler (one handler for all GPIO interrupts). Single consumer: `irq/poll` on the main thread.

```rust
static mut EVENT_QUEUE: Queue<Event, 32> = Queue::new();

#[interrupt]
fn IO_IRQ_BANK0() {
    // Determine which pin triggered, which edge
    // Push Event { pin, edge, tick } to EVENT_QUEUE
    // Clear interrupt flag
}
```

### Serial Output (println)

On Pico, `println` output goes to:
1. **USB-CDC serial** (default) — shows up as a serial port on the connected computer
2. **UART0** — if configured for debug output
3. **defmt/RTT** — if using debug probe

The Sema `println` native function is wired to write to USB-CDC by default. This is configured at init time.

### Memory Budget (520KB SRAM)

| Component | Budget |
|-----------|--------|
| Rust stack | 8 KB |
| VM state (stack, frames, inline cache) | 10 KB |
| String interner | 12 KB |
| Global env + ~100 native fns | 25 KB |
| OLED framebuffer (128×64) | 1 KB |
| NeoPixel buffer (60 LEDs × 3 bytes) | 180 bytes |
| Interrupt queue + callbacks | 2 KB |
| `embedded-alloc` heap metadata | 4 KB |
| **Value heap (user programs)** | **~450 KB** |
| → At 8 bytes/cell | **~56K cells** |

---

## Prerequisites (must exist before hardware API work)

These features must be confirmed or added to Sema before implementing the hardware API:

1. **Bitwise operations** — `bit-and`, `bit-or`, `bit-xor`, `bit-shift-left`, `bit-shift-right`, `bit-set?` are essential for register manipulation. **Status:** Already exist in `sema-stdlib/src/bitwise.rs`.
2. **Bytevector type** — Raw byte arrays for I2C/SPI data. **Status:** Already exists in `sema-stdlib/src/bytevector.rs` with `make-bytevector`, `bytevector`, `bytevector-u8-ref`, `bytevector-u8-set!`, `bytevector-length`.
3. **`try`/`catch`** — Error handling for hardware faults. **Status:** Already exists as special forms.
4. **`finally` in `try`/`catch`** — Needed for clean `with-*` macros. **Status: TBD** — if not present, macros use the success+error dual-release pattern instead.
5. **`no_std` + `alloc` support in sema-core** — Phase 1 of the Pico port plan.

---

## Embedded-Specific Additions to Consider

### GPIO Hardware Functions (Pico-specific)

```scheme
;; Bulk operations for keypad scanning / LED matrices
(gpio/read-mask mask)          ;; Read multiple pins at once via SIO
(gpio/write-mask mask values)  ;; Write multiple pins at once

;; Pin configuration
(gpio/drive-strength pin :ma-12)  ;; 2, 4, 8, 12 mA
(gpio/slew-rate pin :fast)        ;; :slow, :fast
```

### System Functions

```scheme
(sys/reset)                  ;; Hard reset
(sys/bootloader)             ;; Enter UF2 bootloader mode
(sys/chip-id)                ;; Unique chip identifier
(sys/free-memory)            ;; Bytes of heap available
(sys/clock-mhz)              ;; Current CPU clock (150 MHz default)
```

---

## Complete Example Programs

### Blink (Tier 1 only)

```scheme
(gpio/init 25 :output)
(loop
  (gpio/toggle 25)
  (time/sleep-ms 500))
```

### Button-Controlled LED with Debounce (Tier 1 + 2)

```scheme
(gpio/init 25 :output)
(gpio/init 15 :input :pull-up)

(irq/watch 15 :falling
  (fn [evt] (gpio/toggle 25))
  {:debounce 50})

(loop (irq/wait 10000))
```

### Temperature on OLED (Tier 1 + 2 + 3)

```scheme
(with-i2c [bus 0 0 1 :fast]
  (with-oled [screen bus 0x3C 128 64]
    (loop
      (def temp (adc/read-temp))
      (oled/clear screen)
      (oled/text screen 0 0 "Sema Pico")
      (oled/line screen 0 10 127 10)
      (oled/text screen 0 20
        (str (/ (round (* temp 10)) 10) " C")
        {:size 2})
      (oled/show screen)
      (time/sleep-ms 1000))))
```

### NeoPixel Rainbow (Tier 3)

```scheme
(with-neo [strip 16 8]
  (neo/brightness strip 50)
  (def hue (atom 0))
  (loop
    (for-each
      (fn [i]
        (def h (% (+ @hue (* i 32)) 256))
        (neo/set strip i (hsv->rgb h 255 255)))
      (range 0 8))
    (neo/show strip)
    (swap! hue (fn [h] (% (+ h 1) 256)))
    (time/sleep-ms 20)))
```

---

## API Summary

| Tier | Namespace | Count | Description |
|------|-----------|-------|-------------|
| 1 | `gpio/` | 7 | Digital I/O |
| 1 | `time/` | 5 | Delays, ticks, pulse measurement |
| 1 | `adc/` | 3 | Analog input, temperature |
| 1 | `pwm/` | 4 | PWM output |
| 2 | `i2c/` | 8 | I2C bus communication |
| 2 | `spi/` | 5 | SPI bus communication |
| 2 | `uart/` | 6 | Serial UART |
| 2 | `irq/` | 6 | Interrupts, timers, callbacks |
| 3 | `neo/` | 6 | NeoPixel / WS2812 LEDs |
| 3 | `servo/` | 4 | Servo motors |
| 3 | `oled/` | 11 | SSD1306 OLED display |
| 3 | `tone/` | 3 | Buzzer / frequency generation |
| — | `board/` | 2 | Board aliases and info |
| — | macros | 5 | with-i2c, with-spi, with-uart, with-neo, with-oled, with-pins |
| | **Total** | **75 functions + 6 macros** | |

## Advanced Features

Three advanced design ideas that play to Lisp's strengths in ways Arduino/MicroPython can't match. These are detailed separately below, after the sanity check section.

1. **PIO as a Lisp DSL** — Express RP2350 PIO programs as Sema s-expressions, assembled to machine code. Programs are data, manipulable by macros.
2. **Reactive Event Model** — Espruino-style declarative programming: register event handlers, runtime sleeps automatically between events. No user main loop.
3. **Unified Stream Abstraction** — A single `stream/read` / `stream/write` protocol across files, UART, I2C, SPI, TCP, and in-memory buffers. Works on both desktop and Pico.

---

## Known Issues from Sanity Check

1. **NeoPixels depend on PIO internally.** The `neo/` implementation uses a PIO state machine under the hood even though PIO user API is Tier 4. This is intentional — PIO is used as an implementation detail, not exposed.
2. **`irq/wait` should always have a timeout** in production code to avoid bricking. Documented as a best practice.
3. **Pin conflict detection** between GPIO and bus peripherals is handled by the `pin_owners` registry in `PeripheralManager`.
4. **`finally` support in try/catch** — if missing, `with-*` macros use dual-path release pattern.
5. **OLED framebuffer costs 1KB** — acceptable on 520KB RP2350, tight on 264KB RP2040.
6. **Timer alarms are limited** (4 on RP2040, more on RP2350). `irq/timer` must document the limit and error on exhaustion.
7. **`try`/`catch` has no `finally` clause** (confirmed). `with-*` macros use dual-path release: catch rethrows after cleanup, success path also calls cleanup.

---

## Advanced Feature 1: PIO as a Lisp DSL

The RP2350 has 3 PIO blocks, each with 4 state machines and 32 instruction slots. PIO programs are tiny (9 instructions) but timing-critical — they run independently at up to 150MHz. The DSL expresses PIO programs as Sema lists, assembled to 16-bit machine code.

### Design Principles

- **Instructions are functions returning data (maps).** A PIO program is a plain Sema list. The assembler is a separate function that walks the list and emits bytevectors.
- **Labels are symbols** in the instruction list, stripped by the assembler and resolved to addresses.
- **Side-set and delay are composable wrappers** (`pio/side`, `pio/delay`) that attach metadata to any instruction via `assoc`. Most instructions don't use them, so the common case stays clean.
- **Programs are data.** You can `map`, `filter`, `concat` PIO programs. Macros can generate instruction sequences.

### Instruction Set

```scheme
;; JMP — conditional jump
(pio/jmp 'loop)                    ; always
(pio/jmp :!x 'done)               ; if x == 0
(pio/jmp :x-- 'loop)              ; decrement x, jump if was non-zero
(pio/jmp :pin 'high)              ; if input pin is high

;; WAIT — block until condition
(pio/wait 1 :gpio 15)             ; wait for GPIO 15 high
(pio/wait 0 :pin 0)               ; wait for mapped pin 0 low
(pio/wait 1 :irq 4)               ; wait for IRQ 4

;; IN/OUT — shift data between pins and shift registers
(pio/in :pins 8)                   ; shift 8 bits from pins into ISR
(pio/out :pins 1)                  ; shift 1 bit from OSR to pins
(pio/out :x 32)                    ; load full OSR into X register

;; PUSH/PULL — FIFO interaction
(pio/push)                         ; ISR → RX FIFO (blocks if full)
(pio/push :no-block)               ; non-blocking
(pio/pull)                         ; TX FIFO → OSR (blocks if empty)
(pio/pull :ifempty)                ; only pull if OSR is empty

;; MOV — move/copy between registers
(pio/mov :x :y)                    ; x = y
(pio/mov :x :!y)                   ; x = ~y (bitwise invert)
(pio/mov :x :y::reverse)           ; x = bit-reverse(y)

;; IRQ — set/clear/wait interrupt flags
(pio/irq :set 0)                   ; set IRQ 0
(pio/irq :wait 2)                  ; set IRQ 2 and wait for clear
(pio/irq :clear 3)                 ; clear IRQ 3

;; SET — immediate value to pins/registers (0-31)
(pio/set :pins 1)                  ; output pins = 1
(pio/set :x 31)                    ; x = 31

;; NOP
(pio/nop)                          ; alias for (pio/mov :y :y)
```

### Side-Set and Delay (Composable Wrappers)

```scheme
;; Without (common case — clean)
(pio/set :pins 1)

;; With side-set
(pio/side 1 (pio/set :pins 0))

;; With delay
(pio/delay 3 (pio/nop))

;; Both compose in any order
(pio/side 1 (pio/delay 3 (pio/set :pins 0)))
```

Both just `assoc` keys (`:side-set`, `:delay`) onto the instruction map.

### Labels and Wrap Points

```scheme
(pio/program
  :wrap-target                      ; hardware loop start (keyword marker)
  'bitloop                          ; label (symbol, resolved by assembler)
  (pio/out :x 1)
  (pio/jmp :!x 'zero)              ; forward reference — works via 2-pass assembly
  (pio/jmp 'bitloop)
  'zero
  (pio/nop)
  :wrap)                            ; hardware loop end
```

**Labels** are symbols — first-class in Lisp, trivially distinguishable from instruction maps. **Wrap points** are keywords (`:wrap-target`, `:wrap`) — structural markers, not jump targets.

### Program Configuration (Separate from Instructions)

```scheme
(def ws2812-config
  (pio/config
    :clock-div   1.0
    :side-set    {:base 16 :count 1}
    :out-pins    {:base 16 :count 1}
    :shift-out   {:direction :left :autopull true :threshold 24}
    :fifo-join   :tx))
```

Same program can run with different pin mappings — config is orthogonal to logic.

### Assembly and State Machine Control

```scheme
;; Assemble: list → bytevector of 16-bit instructions
(def bytecode (pio/assemble ws2812-program :config ws2812-config))

;; Load into PIO block, init state machine
(def offset (pio/load! 0 bytecode))
(pio/sm-init! 0 0 offset ws2812-config)
(pio/sm-start! 0 0)

;; Data transfer via FIFO
(pio/sm-put! 0 0 0x00FF00)        ; send green pixel
(def val (pio/sm-get! 0 0))       ; read from RX FIFO

;; Cleanup
(pio/sm-stop! 0 0)
(pio/sm-unclaim! 0 0)
```

### WS2812 NeoPixel Driver — Complete Example

```scheme
(def ws2812-program
  (list
    'bitloop
    (pio/out :x 1)
    (pio/side 0 (pio/delay 2 (pio/jmp :!x 'do-zero)))
    (pio/side 1 (pio/delay 1 (pio/jmp 'bitloop)))
    'do-zero
    (pio/side 1 (pio/delay 1 (pio/nop)))
    :wrap))

;; Assemble and run
(def code (pio/assemble ws2812-program :config ws2812-config))
(def offset (pio/load! 0 code))
(pio/sm-init! 0 0 offset ws2812-config)
(pio/sm-start! 0 0)

;; Send pixels
(for-each (fn [color] (pio/sm-put! 0 0 color))
  (list 0x00FF00 0xFF0000 0x0000FF))
```

### Macro Opportunities

Since PIO programs are data, Sema macros can generate them:

```scheme
;; Generate a bit-banged waveform from timing pairs
(defmacro pio/bitbang (pin . phases)
  `(list :wrap-target
     ,@(map (fn (phase) `(pio/delay ,(second phase)
                           (pio/set :pins ,(first phase))))
            phases)
     :wrap))

;; Usage: blink at PIO speed
(def blink (pio/bitbang 25 (1 31) (0 31)))

;; Generate SPI from polarity/phase spec
(defmacro pio/defspi (name opts)
  ;; ... generates PIO program from :cpol :cpha :bits spec
  )
```

This is the killer feature: **a Lisp macro system generating real-time hardware programs.** Arduino/MicroPython can't do this — their PIO programs are static text, not manipulable data.

### Assembler Implementation

`pio/assemble` runs as a normal Sema function (or at macro-expand time via `pio/defprogram`):

1. **Pass 1:** Scan list for symbols → build label address map. Strip labels and wrap markers.
2. **Pass 2:** Encode each instruction map to 16-bit word. Resolve jump targets from label map.
3. **Validate:** Check instruction count ≤ 32, all values fit bit fields, all labels referenced exist.
4. **Return:** `{:instructions <bytevector> :wrap-target N :wrap M :length N :labels {...}}`

On the Pico, assembly happens at load-time (when the `.semac` module is evaluated). For production, a `pio/defprogram` macro could pre-assemble at compile time.

---

## Advanced Feature 2: Reactive Event Model

Replace the imperative `irq/poll` main loop with a declarative event registration model inspired by Espruino. Programs declare what should happen in response to events; the runtime manages sleeping and waking.

### The `on` / `run` Pattern

```scheme
;; Register event handlers (pure declarations)
(on (gpio/edge 15 :falling)
  (fn [e] (gpio/toggle 25))
  {:debounce 50})

(on (every 1000)
  (fn [_] (println (adc/read-temp))))

(on (uart/rx serial)
  (fn [data] (uart/write serial data)))

;; Enter event loop — runtime sleeps (WFI) between events
(run)
```

**No user main loop.** The `(run)` function is the event loop. Between events, the RP2350 enters WFI (wait for interrupt), saving ~60% power.

### Why Explicit `(run)` Not Implicit

Sema is a Lisp — explicitness and composability are valued. The event loop is an explicit function call, not hidden magic. Reasons:

1. One-shot programs work naturally (no special-casing "no handlers registered")
2. Setup code can run between handler registration and `(run)`
3. `(run)` is composable — can be conditional, inside a function, etc.
4. A safety warning fires if handlers exist but `(run)` was never called

### Event Sources

Event source constructors return opaque descriptors. `on` is the single registration point.

```scheme
(gpio/edge pin edge)     ;; GPIO pin change
(every interval-ms)      ;; periodic timer
(after delay-ms)         ;; one-shot timer
(uart/rx port)           ;; UART data available
(pio/fifo-rx sm)         ;; PIO FIFO not empty (future)
```

### Control Flow

```scheme
(def h (on (every 1000) callback))  ;; returns handle
(cancel h)                           ;; remove handler
(stop)                               ;; exit (run) from inside a callback
(run-for 5000)                       ;; run for 5 seconds then return
```

### Execution Model

- **No preemption.** Callbacks run to completion before the next event is dispatched (like JavaScript's event loop).
- **Priority:** GPIO edges > UART RX > one-shot timers > periodic timers. Events drained in priority order from the SPSC queue.
- **Watchdog safety:** Hardware watchdog fed before each callback dispatch. If a callback hangs, the chip resets.

### Error Handling in Callbacks

```scheme
;; Default: log error to USB-CDC, continue event loop
(on (every 1000) (fn [_] (/ 1 0)))
;; prints: "Error in callback [timer:0]: division by zero"

;; Custom error handler
(on-error (fn [source err]
  (println (str "FAULT: " (error/message err)))
  (gpio/write error-led :high)))

;; Fatal mode: stop on first error
(on-error (fn [_ _] (stop)))
```

Rationale: On an embedded device, crashing the event loop bricks the device. Log-and-continue is the safe default. `on-error` gives full control.

### Implementation: No Evaluator Changes Needed

`(run)` is a NativeFn that loops:

```rust
fn native_run(ctx: &EvalContext, _args: &[Value]) -> Result<Value, SemaError> {
    loop {
        if stop_flag.get() { break; }
        while let Some(event) = EVENT_QUEUE.dequeue() {
            if let Some(cb) = registry.lookup(event.source) {
                match call_callback(ctx, &cb, &[event.to_value()]) {
                    Ok(_) => {},
                    Err(e) => handle_callback_error(ctx, e),
                }
            }
        }
        cortex_m::asm::wfi();  // sleep until next interrupt
    }
    Ok(Value::keyword("ok"))
}
```

Uses existing `call_callback` → VM dispatch loop → return. No new Trampoline variant, no changes to vm.rs or eval.rs.

### Complete Example: Multi-Event Program

```scheme
(gpio/init 25 :output)
(gpio/init 15 :input :pull-up)
(def serial (uart/init 0 0 1 115200))
(def press-count (atom 0))

(on (gpio/edge 15 :falling) (fn [e]
  (swap! press-count inc)
  (gpio/toggle 25))
  {:debounce 50})

(on (every 10000) (fn [_]
  (uart/write serial
    (str "presses=" @press-count
         " temp=" (adc/read-temp) "\r\n"))))

(on (uart/rx serial) (fn [data]
  (def cmd (string/trim (bytes->string data)))
  (when (= cmd "stop") (stop))))

(run)
```

Handles GPIO, timers, and UART simultaneously. Zero explicit loops. Automatic power management.

---

## Advanced Feature 3: Unified Stream Abstraction

A protocol where the same `stream/read` / `stream/write` works on files, UART, I2C, SPI, TCP, and in-memory buffers. Works on both desktop Sema and Pico.

### Core Protocol (7 Functions)

```scheme
(stream/read s n)           ;; read up to n bytes → bytevector
(stream/write s bv)         ;; write bytevector → int (bytes written)
(stream/read-byte s)        ;; read one byte → int or nil at EOF
(stream/write-byte s b)     ;; write one byte (0-255)
(stream/available? s)       ;; data ready? → bool
(stream/close s)            ;; release resource
(stream/flush s)            ;; flush output buffer
```

Plus predicates: `stream?`, `stream/readable?`, `stream/writable?`.

### Implementation: New Value Tag

```rust
const TAG_STREAM: u64 = 25;  // next available after TAG_MULTIMETHOD = 24
```

Backed by a Rust trait:

```rust
// sema-core/src/stream.rs
pub trait SemaStream: fmt::Debug {
    fn read(&self, buf: &mut [u8]) -> Result<usize, SemaError>;
    fn write(&self, data: &[u8]) -> Result<usize, SemaError>;
    fn available(&self) -> Result<bool, SemaError> { Ok(false) }
    fn flush(&self) -> Result<(), SemaError> { Ok(()) }
    fn close(&self) -> Result<(), SemaError> { Ok(()) }
    fn stream_type(&self) -> &'static str;
}
```

Dispatch is in Rust (trait vtable call), not Sema-land multimethods. This keeps it fast and Pico-viable.

**Fat pointer solution:** `dyn SemaStream` is a fat pointer (16 bytes) but NaN-boxing only fits thin pointers. Solved by double-boxing: `Rc<StreamBox>` where `StreamBox(RefCell<Box<dyn SemaStream>>)` is a concrete Sized type with a thin pointer.

### Desktop Streams

```scheme
(stream/open-input "data.txt")    ;; → readable file stream
(stream/open-output "out.txt")    ;; → writable file stream
(stream/from-string "hello")      ;; → readable stream from string
(stream/from-bytes bv)            ;; → readable stream from bytevector
(stream/byte-buffer)              ;; → writable buffer, extract later
(stream/tcp-connect "host" 80)    ;; → bidirectional TCP stream
```

Global streams: `*stdin*`, `*stdout*`, `*stderr*`.

### Pico Streams

```scheme
(stream/uart-open 0 0 1 115200)  ;; → bidirectional UART stream
(stream/i2c-open bus 0x48)       ;; → I2C device stream
(stream/spi-open bus cs-pin)     ;; → SPI device stream
```

USB-CDC serial becomes `*stdin*`/`*stdout*` on Pico — code that reads from `*stdin*` works on both platforms.

### Resource Management

```scheme
(with-stream [s (stream/open-input "data.txt")]
  (stream/read s 1024))
;; s closed here, even on error
```

### Composable Wrappers

```scheme
(stream/buffered s 4096)          ;; add buffering (configurable size)
(stream/tee s1 s2)                ;; writes go to both
```

### Backward Compatibility

- `file/read` and `file/write` remain as-is (convenience functions)
- `println` gains optional stream argument: `(println "hello" serial-port)`
- `read-line` gains optional stream: `(read-line s)`
- Existing code unchanged; streams are additive

### Migration Path for Pico with-* Macros

```scheme
;; Phase 1 (current plan): with-i2c binds a raw bus handle
(with-i2c [bus 0 0 1 :fast]
  (i2c/write bus 0x48 data))

;; Phase 2 (future): with-i2c can bind a stream
(with-stream [s (stream/i2c-open bus 0x48)]
  (stream/write s data))
```

Both APIs coexist. The stream version is optional but enables portable code.

### R7RS Alignment

| R7RS | Sema | Notes |
|------|------|-------|
| `open-input-file` | `stream/open-input` | Same semantics |
| `read-u8` | `stream/read-byte` | Returns int, not char |
| `write-bytevector` | `stream/write` | Takes bytevector |
| `port?` | `stream?` | Type predicate |
| `close-port` | `stream/close` | Any direction |
| `call-with-port` | `with-stream` macro | Macro instead of HOF |

Text vs binary distinction skipped — Sema streams are always byte-oriented. Text operations layer on top via UTF-8.

### Sema Platform Support Confirmed

- **Multi-methods (`defmulti`/`defmethod`):** Exist but not needed — dispatch is in Rust via trait vtable.
- **Records (`define-record-type`):** Exist, could be used for stream metadata.
- **NaN-box tag space:** 39 slots remain (25 of 64 used). TAG_STREAM = 25 fits easily.
- **`try`/`catch`:** Exists. **No `finally`** — `with-stream` macro uses dual-path cleanup.
- **Bytevectors:** Exist (`sema-stdlib/src/bytevector.rs`) — natural data type for stream I/O.
- **Context stacks:** Exist — could track open streams for leak detection.

### Incremental Implementation

| Phase | Work | Scope |
|-------|------|-------|
| 1 | `SemaStream` trait + TAG_STREAM + `ByteBufferStream` | sema-core |
| 2 | `FileStream` + `stream/open-*` + `*stdin*`/`*stdout*` | sema-stdlib |
| 3 | `stream/from-string`, `stream/from-bytes`, `with-stream` macro | sema-stdlib + prelude |
| 4 | `TcpStream` (gated on `Caps::NETWORK`) | sema-stdlib |
| 5 | `BufferedStream`, `stream/tee` | sema-stdlib |
| 6 | Pico streams (`UartStream`, `I2cStream`, `SpiStream`) | sema-pico (future) |

Phases 1-3 are useful on desktop Sema immediately and don't require the Pico port.
