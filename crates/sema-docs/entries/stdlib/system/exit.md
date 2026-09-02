---
name: "exit"
module: "system"
section: "Shell & Process Control"
syntax: "(exit [code])"
returns: "never returns; terminates the process"
see_also: ["sys/args", "error"]
---

Exit the process with a given status code.

```sema
(exit 0)   ; exit successfully
(exit 1)   ; exit with error
```
