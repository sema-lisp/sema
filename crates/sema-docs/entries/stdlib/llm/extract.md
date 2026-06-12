---
name: "llm/extract"
module: "llm"
params: [{ name: schema }, { name: text, type: string }, { name: opts, type: map }]
returns: "map"
---

Extract structured data from text into a value matching the given schema. The model is instructed to return JSON-only; the response is parsed and (by default) validated against the schema, with automatic retries. The opts map accepts `:model`, `:validate` (default true), `:retries` (default 2), and `:reask?` (default true, sends the prior response and validation error back to the model). Returns the extracted value.

```sema
(llm/extract {:name :string :age :int} "John Doe is 42 years old" {:retries 1})
```
