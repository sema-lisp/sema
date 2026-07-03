---
outline: [2, 3]
---

# Playground & WASM

When running in the browser playground at [sema.run](https://sema.run), Sema executes as WebAssembly. Most stdlib functions work identically, but some behave differently due to browser sandbox constraints, and a few web-only functions are available.

::: warning Chromium ARM64 Note
Chrome/Chromium builds before version 147 contain a V8 ARM64 WebAssembly compiler bug that can crash the renderer on some heavy tree-walker workloads in the playground. If you hit a reproducible tab crash on Apple Silicon, update Chrome or retry in Firefox, Safari, or Chrome 147+.
:::

## Web-Only Functions

These functions are **only available in the WASM playground** — they access browser APIs that don't exist in the native CLI.

### `web/user-agent`

Return the browser's `navigator.userAgent` string. Works in all browsers.

```sema
(web/user-agent)
; => "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 ..."
```

### `web/user-agent-data`

Return structured browser information from `navigator.userAgentData`. Returns a map on Chromium-based browsers (Chrome, Edge, Opera), `nil` on Firefox and Safari.

```sema
(web/user-agent-data)
; Chromium => {:mobile false :platform "macOS" :brands ("Chromium/120" "Google Chrome/120")}
; Firefox/Safari => nil
```

::: tip
`userAgentData` is the modern replacement for UA string parsing — it returns structured, reliable data instead of a messy string. However, it's Chromium-only. Use `web/user-agent` for cross-browser compatibility.
:::

## WASM Behavior Differences

### System Information

System functions return web-appropriate values instead of OS-specific ones:

| Function           | Native                              | WASM                        |
| ------------------ | ----------------------------------- | --------------------------- |
| `sys/platform`     | `"macos"` / `"linux"` / `"windows"` | `"web"`                     |
| `sys/os`           | `"macos"`                           | `"web"`                     |
| `sys/arch`         | `"aarch64"` / `"x86_64"`            | `"wasm32"`                  |
| `sys/cwd`          | Current directory path              | `"/"`                       |
| `sys/interactive?` | `#t` in REPL                        | `#f`                        |
| `sys/pid`          | Process ID                          | `0`                         |
| `sys/elapsed`      | Nanoseconds since process start     | Nanoseconds since page load |
| `time-ms`          | `SystemTime` milliseconds           | `Date.now()` milliseconds   |

These always return `nil` in WASM: `sys/hostname`, `sys/user`, `sys/home-dir`, `sys/which`, `sys/tty`.

### File I/O (Virtual Filesystem)

File operations work against an **in-memory virtual filesystem** (VFS). Files persist for the duration of your session but are **lost on page reload**.

```sema
;; These all work in the playground
(file/write "/hello.txt" "Hello from WASM!")
(file/read "/hello.txt")       ; => "Hello from WASM!"
(file/exists? "/hello.txt")    ; => #t
(file/mkdir "/mydir")
(file/is-directory? "/mydir")  ; => #t
(file/list "/")                ; => ("hello.txt")
```

All file functions are supported: `file/read`, `file/write`, `file/append`, `file/delete`, `file/rename`, `file/copy`, `file/exists?`, `file/list`, `file/mkdir`, `file/is-file?`, `file/is-directory?`, `file/is-symlink?`, `file/info`, `file/read-lines`, `file/write-lines`. Path functions (`path/join`, `path/dirname`, `path/basename`, `path/extension`, `path/absolute`) also work.

**Quotas**: The VFS enforces limits to prevent runaway memory usage — 1 MB per file, 16 MB total, and 256 files max. Exceeding these limits returns an error.

The `load` function reads from the VFS and evaluates the parsed expressions.

### Terminal Styling

All `term/*` functions work but return text **without ANSI formatting** (since the browser has no terminal):

```sema
(term/bold "hello")   ; => "hello" (no bold applied)
(term/red "error")    ; => "error" (no color applied)
(term/style "hi" :bold :cyan)  ; => "hi"
```

### HTTP Functions

HTTP functions work in the playground via the browser's `fetch()` API. They return the same `{:status :headers :body}` map as the native CLI.

```sema
(define resp (http/get "https://httpbin.org/get"))
(:status resp)    ; => 200
(:body resp)      ; => "{\"args\": {}, ...}"

(http/post "https://httpbin.org/post" {:name "sema"})
; => {:status 200 :headers {...} :body "..."}
```

All HTTP functions are supported: `http/get`, `http/post`, `http/put`, `http/delete`, `http/request`.

::: warning CORS Restrictions
Browser security rules (CORS) may block requests to servers that don't include `Access-Control-Allow-Origin` headers. Public APIs like httpbin.org work fine. If you get a network error, the target server likely doesn't allow cross-origin requests.
:::

### Not Available in WASM

These functions return an error when called in the playground:

| Function     | Reason                                       |
| ------------ | -------------------------------------------- |
| `shell`      | No subprocess execution in browser           |
| `exit`       | No process to exit                           |
| `io/read-line`  | No stdin in browser                          |
| `io/read-stdin` | No stdin in browser                          |
| `sleep`      | Cannot block the browser main thread (no-op) |
