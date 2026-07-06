---
outline: [2, 3]
---

# Serial Ports

Talk to microcontrollers, USB-CDC devices, and any UART over a host serial port. Wraps the cross-platform [`serialport`](https://crates.io/crates/serialport) crate.

::: warning Not available in WASM
Serial ports require the host OS — this module is unavailable in the browser playground.
:::

::: tip Sandbox capability
All `serial/*` functions require the `serial` capability. They are denied under `--sandbox=strict` and `--sandbox=all`. Allow with the default sandbox or explicitly opt in (see [CLI sandbox docs](../cli#sandbox)).
:::

## Connection Lifecycle

### `serial/list`

List the available serial port device paths on the host.

```sema
(serial/list)
;; macOS: ("/dev/tty.usbmodem1201" "/dev/tty.Bluetooth-Incoming-Port")
;; Linux: ("/dev/ttyUSB0" "/dev/ttyACM0")
```

### `serial/open`

```sema
(serial/open path baud)            ; default 2000 ms read timeout
(serial/open path baud timeout-ms)
```

Open a serial port and return an integer **handle** used by every other function in this module. Raises an error if the device is busy or doesn't exist; the message includes the path and baud rate as a hint.

```sema
(define pico (serial/open "/dev/tty.usbmodem1201" 115200))
(define modem (serial/open "/dev/ttyUSB0" 9600 5000))   ; 5s read timeout
```

### `serial/close`

```sema
(serial/close handle)
```

Close the port and free the handle. Subsequent calls with that handle raise `invalid handle`.

## I/O

### `serial/write`

```sema
(serial/write handle string)
```

Write a raw string to the port and flush. No newline appended — append `"\n"` yourself if your protocol expects it.

```sema
(serial/write modem "AT\r\n")
```

### `serial/read-line`

```sema
(serial/read-line handle) → string
```

Read until `\n`, then trim trailing `\r` / `\n` and return the line. Blocks until either a newline arrives or the port's read timeout elapses (configured at `serial/open` time) — on timeout, raises an error.

```sema
(serial/read-line pico)   ; => "ready"
```

### `serial/send`

```sema
(serial/send handle command) → parsed-json | nil
```

Convenience for line-oriented JSON protocols (such as the [sema-bridge](https://github.com/sema-lisp/sema/tree/main/examples) firmware that ships with the Pico examples). Writes `command + "\n"`, flushes, reads one line back, and parses it as JSON. Returns `nil` if the response line is empty.

```sema
(serial/send pico "{\"cmd\":\"led-on\",\"pin\":25}")
;; => {:ok #t}

(serial/send pico "{\"cmd\":\"adc-read\",\"pin\":26}")
;; => {:ok #t :value 2048}
```

## Example: Pico 2 LED control

```sema
(define pico (serial/open "/dev/tty.usbmodem1201" 115200))
(println "bridge:" (serial/read-line pico))   ; "ready"

(define (pico-cmd cmd)
  (let ((resp (serial/send pico cmd)))
    (when (not (get resp :ok))
      (error (format "pico error: ~a" (get resp :error))))
    resp))

(pico-cmd "{\"cmd\":\"led-on\",\"pin\":25}")
(sleep 500)
(pico-cmd "{\"cmd\":\"led-off\",\"pin\":25}")

(serial/close pico)
```

See `examples/pico-blink.sema`, `pico-piano.sema`, `pico-jukebox.sema`, `pico-midi.sema`, and `pico-show.sema` for full demos.
