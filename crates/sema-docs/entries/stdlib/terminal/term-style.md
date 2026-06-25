---
name: "term/style"
module: "terminal"
section: "Combined Styles"
syntax: "(term/style text style ...)"
returns: "string"
---

Apply multiple styles at once using keywords. The first argument is the text, followed by one or more style keywords.

```sema
(term/style "danger" :bold :red)
(term/style "notice" :italic :yellow :underline)
(term/style "subtle" :dim :gray)
```

Internally, `term/style` combines ANSI codes with `;` separators into a single escape sequence (e.g., `ESC[1;31m` for bold red), which is more efficient than nesting individual style functions.

If called with no style keywords, the text is returned unstyled.

```sema
(term/style "plain text")   ; => "plain text" (no ANSI codes)
```

An unknown keyword produces an error:

```sema
(term/style "text" :blink)  ; Error: unknown style keyword :blink
```

#### Style keyword reference

| Keyword          | Effect         | ANSI Code |
|------------------|----------------|-----------|
| `:bold`          | Bold           | 1         |
| `:dim`           | Dim            | 2         |
| `:italic`        | Italic         | 3         |
| `:underline`     | Underline      | 4         |
| `:inverse`       | Inverse        | 7         |
| `:strikethrough` | Strikethrough  | 9         |
| `:black`         | Black text     | 30        |
| `:red`           | Red text       | 31        |
| `:green`         | Green text     | 32        |
| `:yellow`        | Yellow text    | 33        |
| `:blue`          | Blue text      | 34        |
| `:magenta`       | Magenta text   | 35        |
| `:cyan`          | Cyan text      | 36        |
| `:white`         | White text     | 37        |
| `:gray`          | Gray text      | 90        |
