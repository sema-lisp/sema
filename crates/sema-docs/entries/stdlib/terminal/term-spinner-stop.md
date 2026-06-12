---
name: "term/spinner-stop"
module: "terminal"
section: "Spinners"
---

Stop a running spinner and optionally display a final status line. The spinner line is cleared from the terminal before the final status is printed.

**Without options** — just clears the spinner:

```sema
(term/spinner-stop id)
```

**With options map** — displays a final symbol and text:

```sema
(term/spinner-stop id {:symbol "✔" :text "Done"})
```

The options map supports two keys:

| Key       | Type   | Description                          |
|-----------|--------|--------------------------------------|
| `:symbol` | string | Symbol to display (e.g., `"✔"`, `"✗"`, `"⚠"`) |
| `:text`   | string | Final status message                 |

Both keys are optional. The final line is printed to stderr as `symbol text`.
