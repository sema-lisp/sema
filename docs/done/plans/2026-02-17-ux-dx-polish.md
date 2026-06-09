# UX/DX Polish — Future Ideas

Collected ideas for improving the developer experience. No functional changes — these are presentation and ergonomics improvements.

---

## 1. Pretty-print nested data structures in REPL output

**Problem:** Complex nested maps/lists are printed as a single long line, making them hard to read:

```
({:id "doc-1" :score 0.92 :metadata {:source "greeting.txt" :page 1}} {:id "doc-2" :score 0.85 :metadata {:source "readme.md" :page 3}})
```

**Desired:** When output has multiple nested levels or exceeds a width threshold, auto-format with indentation:

```
({:id "doc-1"
  :score 0.92
  :metadata {:source "greeting.txt" :page 1}}
 {:id "doc-2"
  :score 0.85
  :metadata {:source "readme.md" :page 3}})
```

**Scope:**

- Only affects REPL display and `println` output formatting — no changes to `Value` representation or evaluation semantics
- Should also be reflected in website docs example outputs for readability
- Consider a `(set! *print-width* 80)` parameter or `(pprint expr)` function
- Relevant functions: vector-store/search results, llm/extract results, map-heavy pipelines

**Implementation notes:**

- Add a `pretty_print(value, max_width)` function in sema-core or sema-stdlib
- REPL uses it by default for interactive output
- `display`/`println` keep current compact format for scripting compatibility
