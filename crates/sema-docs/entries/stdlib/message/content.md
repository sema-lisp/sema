---
name: "message/content"
module: "message"
params: [{ name: msg, type: message }]
returns: "string"
---

Return the text content of a message.

```sema
(message/content msg)   ; => "Hello, world"
```
