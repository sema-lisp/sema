---
name: "serial/list"
module: "serial"
section: "Connection Lifecycle"
---

List the available serial port device paths on the host.

```sema
(serial/list)
;; macOS: ("/dev/tty.usbmodem1201" "/dev/tty.Bluetooth-Incoming-Port")
;; Linux: ("/dev/ttyUSB0" "/dev/ttyACM0")
```
