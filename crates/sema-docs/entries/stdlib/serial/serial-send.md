---
name: "serial/send"
module: "serial"
section: "I/O"
---

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
