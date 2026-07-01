---
outline: [2, 3]
---

# Agent & TUI Primitives

Host primitives that let you build self-hosted terminal apps — coding agents,
TUIs, task runners — almost entirely in Sema. These cover terminal screen
control, streaming subprocesses, pseudo-terminals, an event loop, file watching,
diff/patch, read-only git, Sema-on-Sema reflection, secret redaction, archives,
and Markdown/HTML. The reference app built on them is [Sema
Coder](https://github.com/HelgeSverre/sema/tree/main/examples/sema-coder).

Capability gating: process/pty/git builtins require `PROCESS`; file-touching
ones require `FS_READ`/`FS_WRITE` (see [System](/docs/stdlib/system)).

## Terminal screen control

Beyond [terminal styling](/docs/stdlib/terminal), these emit ANSI/VT control
sequences so you never hand-write escape codes. Each self-flushes.

```sema
(term/enter-alt-screen)              ; switch to a clean alternate screen
(term/hide-cursor)
(term/clear)
(term/write-at 3 5 (term/rgb "status: ok" 200 168 85))  ; row, col, text
(term/move-to 1 1)
(term/set-title "my app")
(term/show-cursor)
(term/leave-alt-screen)              ; restore the user's scrollback
```

Also: `term/clear-line`, `term/clear-below`, `term/cursor-home`,
`term/save-cursor`, `term/restore-cursor`, `term/enable-mouse`,
`term/disable-mouse`, `term/bell`, `term/flush`.

### Setup guards

Setting up a TUI leaves the terminal in a fragile state — if your program exits
(or crashes) without restoring, the shell is left in raw mode, the alt screen, or
with mouse reporting spewing escape codes. Guard macros **always** restore on
exit, even if the body throws:

```sema
(io/with-raw-mode                 ; restores cooked mode
  (term/with-alt-screen           ; restores the screen + cursor
    (term/with-mouse              ; disables mouse reporting
      (run-tui))))               ; terminal is fully restored however this exits
```

Compose them outermost-restores-last. Each returns the body's value.

### Rich keyboard & mouse input

`io/read-key` / `io/read-key-timeout` return a key map (`{:kind :char/:ctrl/:alt/:key …}`).
After `term/enable-mouse`, mouse reports decode to
`{:kind :mouse :action :press/:release/:move/:wheel-up/:wheel-down :x :y :button :mods}`.
After `term/enable-kitty-keys!` (opt-in; `term/disable-kitty-keys!` to restore),
keys gain reliable modifier reporting as an optional `:mods` list
(e.g. Shift+A → `{:kind :char :char "A" :mods (:shift)}`) — decoded to the same
shapes as before, so existing code is unaffected. Both are opt-in and backward
compatible.

### Display-aware text

Terminal layout needs *display columns*, not character counts (a CJK glyph is two
columns, ANSI escapes are zero). `string/width` gives the rendered width and
`string/word-wrap` wraps text to a column budget — both wide-char and ANSI aware.

```sema
(string/width "日本語")                 ; => 6   (string-length is 3)
(string/word-wrap "the quick brown fox" 10)  ; => ("the quick" "brown fox")
```

## Streaming processes

Unlike `shell` (which blocks and returns output only after exit), `proc/*` hands
you a live handle whose output streams into pollable buffers — so you can show
test output as it happens.

```sema
(define p (proc/spawn ["cargo" "test"] {:cwd "."}))
(let loop ()
  (let ((out (proc/read-stdout p)))
    (when (not (= out "")) (io/print-error out))
    (when (proc/running? p) (sleep 50) (loop))))
(define code (proc/wait p))           ; exit code; flushes the tail first
(proc/close p)                        ; free the handle
```

Full set: `proc/spawn`, `proc/read-stdout`, `proc/read-stderr`,
`proc/write-stdin`, `proc/close-stdin`, `proc/wait`, `proc/exit-code`,
`proc/running?`, `proc/kill`, `proc/close`.

## Pseudo-terminals

Like `proc/*`, but the child runs under a real PTY, so programs that probe
`isatty` (REPLs, editors, `top`, color-aware tools) behave normally.

```sema
(define t (pty/spawn ["bash"] {:rows 40 :cols 120}))
(pty/write t "ls -la\n")
(sleep 100)
(io/print-error (pty/read t))         ; output incl. control sequences
(pty/resize t 50 200)                 ; delivers SIGWINCH
(pty/kill t)
(pty/close t)
```

Full set: `pty/spawn`, `pty/read`, `pty/write`, `pty/resize`, `pty/wait`,
`pty/exit-code`, `pty/running?`, `pty/kill`, `pty/close`.

## Event loop

`event/select` polls a list of sources and returns the first that's ready (or
`nil` on timeout) — the unified wait a TUI loop needs.

```sema
(define proc (proc/spawn ["make" "watch"]))
(let loop ()
  (let ((ev (event/select
              (list {:type :key}                 ; a keypress
                    {:type :proc :handle proc}   ; output or exit
                    (time/tick 16))              ; ~60fps redraw tick
              1000)))                            ; ms timeout
    (cond
      ((nil? ev) (loop))                          ; timed out
      ((= (:type ev) :key)   (handle-key (:value ev)))
      ((= (:type ev) :proc)  (drain-output proc))
      ((= (:type ev) :timer) (redraw)))
    (loop)))
```

## File watching

```sema
(define w (fs/watch "src" {:recursive true}))
(for-each
  (lambda (ev) (println (:kind ev) (:paths ev)))  ; :create/:modify/:remove/...
  (fs/watch-events w))                            ; non-blocking drain
(fs/unwatch w)
```

## Diff & patch

```sema
(define patch (diff/unified old-text new-text))   ; unified diff string
(diff/apply old-text patch)                        ; => new-text
(diff/stat patch)                                  ; => {:added :removed :hunks}
(diff/hunks patch)                                 ; list of hunk maps
(patch/apply-file "src/main.rs" patch)             ; apply to a file in place
```

## Git (read-only)

```sema
(git/root)                 ; repo toplevel
(git/current-branch)
(git/status)               ; list of {:path :status :staged :untracked}
(git/changed-files)        ; list of paths
(git/diff)                 ; or (git/diff "path") — unified diff
(git/recent-files 20)      ; files touched by the last N commits
(git/ignore-matches? "target/x")   ; => #t
```

Paths are returned as real UTF-8 (quoting disabled), and renames/spaces are
parsed unambiguously via NUL-delimited porcelain.

## Reflection & diagnostics

Parse, format, and check Sema source from Sema — diagnostics come back as data,
ideal for agent repair loops.

```sema
(read/string "(+ 1 2)")            ; => the form (+ 1 2)
(read/all "(a) (b)")               ; => ((a) (b))
(format/form '(define  x  1))      ; => "(define x 1)"

(sema/check-string "(+ 1 2")       ; => {:ok #f :diagnostics [{:level :error
                                   ;        :code "syntax" :message ...
                                   ;        :span {:line :col :end-line :end-col}}]}
(sema/check-file "workflow.sema")  ; same, reading a file
```

## Secrets & redaction

```sema
(secret/detect "key AKIA... and tok eyJ...")  ; list of {:type :match :start :end}
(secret/redact text)               ; => text with secrets → «redacted:<type>»
(pii/detect text)                  ; emails, IPv4, phone numbers
(redact/spans text spans)          ; redact caller-supplied {:start :end :label} ranges
(hash/digest text)                 ; SHA-256 hex (fingerprint a redacted value)
```

## Archives

```sema
(gzip/compress (string->bytevector "hello"))   ; => gzip bytevector
(gzip/decompress bytes)
(zip/create "out.zip" '("a.txt" "b.txt"))      ; => entry count
(zip/extract "out.zip" "dest/")                ; zip-slip guarded
(zip/list "out.zip")
(tar/create "out.tar.gz" '("a.txt"))           ; .tar.gz/.tgz auto-gzips
(tar/extract "out.tar.gz" "dest/")             ; traversal + symlink guarded
```

## Markdown & HTML

```sema
(markdown/to-html "# Title\n\nHello **world**.")
(markdown/headings md)             ; list of {:level :text}
(markdown/frontmatter md)          ; {:frontmatter :body}
(html/select html "a.button")      ; list of matched elements' outer HTML
(html/select-text html "h1")       ; list of matched elements' text
(html/text html)                   ; all visible text, whitespace-collapsed
```

## Path safety

```sema
(path/within? "/repo" "/repo/src/x")  ; => #t   (catches ../ and symlink escapes)
(path/canonicalize "./src/../x")      ; real absolute path (errors if missing)
(path/relative-to "/a/b" "/a/b/c/d")  ; => "c/d"
(sys/config-dir)                      ; platform config base for app config
```
