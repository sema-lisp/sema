---
name: "message/with-image"
module: "message"
params: [{ name: role, type: keyword }, { name: text, type: string }, { name: image, type: bytevector }, { name: opts, type: map }]
returns: "message"
---

Build a multimodal message with text plus an attached image. Role is `:system`, `:user`, `:assistant`, or `:tool`; the image is a bytevector that is base64-encoded. The media type is auto-detected unless overridden via opts `:media-type`.

```sema
(message/with-image :user "Describe this picture" png-bytes {:media-type "image/png"})
```
