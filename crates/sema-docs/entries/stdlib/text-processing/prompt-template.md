---
name: "prompt/template"
module: "text-processing"
section: "Prompt Templates"
params: [{ name: text, type: string }]
returns: "string"
see_also: ["prompt/render", "prompt/fill", "format"]
---

Mark a string as a prompt template. The function returns its argument
unchanged: templates are plain strings, and the call exists so that a
template definition reads as one at the definition site. The placeholders
are `{{name}}` with no spaces inside the braces; `prompt/render` replaces
each one with the value of the matching key in a map.

Rendering rules that matter in practice: a placeholder with no matching key
is left as is (so a typo is visible in the output rather than silently
blank), values that are not strings are printed with `str`, and `{{ name }}`
with inner spaces is not recognized.

```sema
(define greeting (prompt/template "Hello {{name}}, welcome to {{place}}."))
(prompt/render greeting {:name "Ada" :place "Sema"})
; => "Hello Ada, welcome to Sema."

(prompt/render "Hi {{a}} {{b}}" {:a 1})     ; => "Hi 1 {{b}}"
(prompt/render "{{x}}" {:x (list 1 2)})     ; => "(1 2)"
```

`prompt/fill` does the same substitution on a structured prompt value; the
`prompt/*` builders (`prompt/set-system`, `prompt/append`, `prompt/concat`)
compose prompts from several parts.
