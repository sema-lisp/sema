---
name: "serial/write"
module: "serial"
section: "I/O"
params: [{ name: handle, type: int }, { name: data, type: string }]
returns: "nil"
see_also: ["serial/read-line", "serial/send", "serial/open"]
---

```sema
(serial/write handle string)
```

Write a raw string to the port and flush. No newline appended — append `"\n"` yourself if your protocol expects it.

```sema
(serial/write modem "AT\r\n")
```
