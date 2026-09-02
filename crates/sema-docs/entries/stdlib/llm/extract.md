---
name: "llm/extract"
module: "llm"
params: [{ name: schema }, { name: text, type: string }, { name: opts, type: map }]
returns: "map"
see_also: ["llm/extract-from-image", "llm/classify", "llm/complete"]
---

Extract structured data from unstructured text into a value matching a schema you describe as a Sema map. The model is told to return JSON only; the response is parsed and (by default) validated against the schema, retrying on failure. Returns the extracted value (a map keyed like the schema).

The schema is the contract: keys are the fields you want, values describe the
shape — a type tag (`:string`, `:int`, `:number`, `:bool`), a nested map, or
`[:type]` for a list. The schema is rendered into the model's instructions, so it
steers the output; validation then checks that the returned JSON has the right keys
(field specs written as `{:type :string ...}` are also type-checked, and may carry
a `:validate` predicate). This beats hand-prompting "return JSON" because the
validate→reask loop self-corrects: when the output doesn't fit, the validation
error is sent **back** to the model (`:reask?`, default true) so it fixes its own
mistake, up to `:retries` times (default 2). Turn validation off with
`:validate false` to accept whatever JSON the model returns. For data trapped in an
image (receipts, screenshots) use the sibling `llm/extract-from-image`.

```sema
;; Flat schema.
(llm/extract {:name :string :age :int}
             "John Doe is 42 years old")
; => {:name "John Doe" :age 42}

;; Nested + list fields, with one retry.
(llm/extract {:title :string
              :authors [:string]
              :year :int}
             "\"Dune\" (1965) was written by Frank Herbert."
             {:retries 1})
; => {:title "Dune" :authors ["Frank Herbert"] :year 1965}
```
