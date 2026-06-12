---
name: "serial/write"
module: "serial"
section: "I/O"
---

```sema
(serial/write handle string)
```

Write a raw string to the port and flush. No newline appended — append `"\n"` yourself if your protocol expects it.

```sema
(serial/write modem "AT\r\n")
```
