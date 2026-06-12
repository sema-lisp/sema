---
name: "text/strip-html"
module: "text-processing"
section: "Text Cleaning"
---

Remove HTML tags and decode common entities (`&amp;`, `&lt;`, `&gt;`, `&quot;`, `&#39;`, `&apos;`, `&nbsp;`).

```sema
(text/strip-html "<p>Hello <b>world</b></p>")  ; => "Hello world"
(text/strip-html "a &amp; b &lt; c")            ; => "a & b < c"
```
