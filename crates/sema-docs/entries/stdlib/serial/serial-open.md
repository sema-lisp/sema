---
name: "serial/open"
module: "serial"
section: "Connection Lifecycle"
---

```sema
(serial/open path baud)            ; default 2000 ms read timeout
(serial/open path baud timeout-ms)
```

Open a serial port and return an integer **handle** used by every other function in this module. Raises an error if the device is busy or doesn't exist; the message includes the path and baud rate as a hint.

```sema
(define pico (serial/open "/dev/tty.usbmodem1201" 115200))
(define modem (serial/open "/dev/ttyUSB0" 9600 5000))   ; 5s read timeout
```
