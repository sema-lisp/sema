---
outline: [2, 3]
---

# Embedding Sema (JavaScript)

## Overview

Sema can be embedded as a JavaScript scripting engine via WebAssembly. The WASM build runs entirely client-side — no server needed. You get the full Sema standard library (minus shell access and LLM functions) in the browser, including HTTP via `fetch()`, an in-memory virtual filesystem, and persistent definitions across evaluations.

::: warning Chromium ARM64 Compatibility
Chrome/Chromium versions earlier than 147 include a V8 ARM64 WebAssembly compiler bug that can crash the renderer on some large or hot workloads. If you see tab crashes on Apple Silicon, update Chrome or use Firefox, Safari, or Chrome 147+.
:::

Two npm packages are available:

| Package | Description |
| --- | --- |
| [`@sema-lang/sema`](https://www.npmjs.com/package/@sema-lang/sema) | **Recommended.** High-level TypeScript wrapper with ergonomic API |
| [`@sema-lang/sema-wasm`](https://www.npmjs.com/package/@sema-lang/sema-wasm) | Low-level wasm-bindgen output — exports `SemaInterpreter` (used internally) |

This page documents the low-level `@sema-lang/sema-wasm` API. For the wrapper, see the [npm README](https://www.npmjs.com/package/@sema-lang/sema).

## Quick Start

### npm

Install the WASM package:

```sh
npm install @sema-lang/sema-wasm
```

Evaluate an expression in three lines:

```js
import init, { SemaInterpreter } from '@sema-lang/sema-wasm';

await init();
const interp = new SemaInterpreter();

const result = interp.evalGlobal('(+ 1 2 3)');
console.log(result.value); // "6"
```

### CDN

Use Sema directly in a `<script>` tag with no build step:

```html
<script type="module">
  import init, { SemaInterpreter } from 'https://cdn.jsdelivr.net/npm/@sema-lang/sema-wasm/sema_wasm.js';

  await init('https://cdn.jsdelivr.net/npm/@sema-lang/sema-wasm/sema_wasm_bg.wasm');
  const interp = new SemaInterpreter();

  const result = interp.evalGlobal('(+ 1 2 3)');
  console.log(result.value); // "6"
</script>
```

## Creating an Interpreter

`new SemaInterpreter()` creates a new interpreter with the full standard library, I/O overrides for browser output, and a 10M eval-step limit to prevent infinite loops from freezing the tab.

```js
import init, { SemaInterpreter } from '@sema-lang/sema-wasm';

await init();
const interp = new SemaInterpreter();
```

The `init()` call loads and compiles the WASM binary. It only needs to be called once — after that, you can create as many interpreters as you want.

When using CDN, pass the `.wasm` URL explicitly:

```js
await init('https://cdn.jsdelivr.net/npm/@sema-lang/sema-wasm/sema_wasm_bg.wasm');
```

## Evaluating Code

Sema's WASM API provides four evaluation methods. All return a JS object with the same shape:

```ts
// JS object returned directly by eval methods
interface EvalResult {
  value: string | null;   // String representation of the result, or null on error
  output: string[];       // Lines printed by (print), (println), (display)
  error: string | null;   // Error message with stack trace, or null on success
}
```

### `evalGlobal(code)` — Synchronous, Persistent

Evaluates code in the global environment. Definitions persist across calls:

```js
const interp = new SemaInterpreter();

interp.evalGlobal('(define x 42)');
const result = interp.evalGlobal('(+ x 8)');
console.log(result.value); // "50"
```

### `eval(code)` — Synchronous, Isolated

Evaluates code in a child environment. Definitions do **not** persist:

```js
interp.eval('(define y 10)');
interp.eval('y'); // error: unbound variable y
```

### `evalAsync(code)` — Async, with HTTP Support

Use this when your Sema code makes HTTP requests. The async method bridges the synchronous Sema evaluator with the browser's asynchronous `fetch()` API using a replay-with-cache strategy:

```js
const result = await interp.evalAsync('(http/get "https://httpbin.org/get")');
console.log(result.value); // {:status 200 :headers {...} :body "..."}
```

### `evalVM(code)` / `evalVMAsync(code)` — Bytecode VM

Explicit aliases for evaluating via the bytecode VM. Sema runs all code on the VM, so these are equivalent to `eval`/`evalAsync`; they are kept as named entry points for clarity. Same interface:

```js
const result = await interp.evalVMAsync('(http/get "https://httpbin.org/get")');
```

### Error Handling

Errors are returned in the `error` field — they never throw JavaScript exceptions:

```js
const result = interp.evalGlobal('(/ 1 0)');

if (result.error) {
  console.error('Sema error:', result.error);
  // Includes stack trace and hints when available
} else {
  console.log('Result:', result.value);
}

// Output lines are always captured, even on error
for (const line of result.output) {
  console.log('Output:', line);
}
```

### Capturing Output

`(print)`, `(println)`, and `(display)` write to a buffer that is returned in the `output` array:

```js
const result = interp.evalGlobal(`
  (println "hello")
  (println "world")
  (+ 1 2)
`);

console.log(result.output); // ["hello", "world"]
console.log(result.value);  // "3"
```

### Minimal Interpreter (No Stdlib)

Use `SemaInterpreter.createWithOptions()` to create a minimal interpreter with only special forms and core evaluation — no standard library functions:

```js
const minimal = SemaInterpreter.createWithOptions({ stdlib: false });

minimal.evalGlobal('(+ 1 2)').value; // "3" (special forms work)
minimal.evalGlobal('(map identity (list 1 2 3))').error; // "unbound variable: map"
```

### Sandboxed Interpreter

Deny specific capabilities while keeping the full stdlib:

```js
// Deny network access
const sema = SemaInterpreter.createWithOptions({
  deny: ["network"]
});

sema.evalGlobal("(+ 1 2)");                           // works
sema.evalGlobal('(http/get "https://example.com")');   // => PermissionDenied error

// Deny both network and VFS writes
const strict = SemaInterpreter.createWithOptions({
  deny: ["network", "fs-write"]
});
```

Available capabilities to deny:

| Capability | Affected Functions |
| --- | --- |
| `"network"` | `http/get`, `http/post`, `http/put`, `http/delete`, `http/request` |
| `"fs-read"` | `file/read`, `file/exists?`, `file/list`, `file/is-directory?`, `file/is-file?` |
| `"fs-write"` | `file/write`, `file/delete`, `file/rename`, `file/mkdir`, `file/append` |

## Registering JavaScript Functions

Use `registerFunction` to expose JavaScript functions to Sema code. Arguments are passed as native JS values, and the return value is converted back to a Sema value:

```js
const interp = new SemaInterpreter();

// Simple function — args arrive as native JS values
interp.registerFunction('add1', (n) => n + 1);

interp.evalGlobal('(add1 41)').value; // "42"
```

### Multiple Arguments

Each Sema argument is passed as a separate native JS value:

```js
interp.registerFunction('greet', (greeting, name) => `${greeting}, ${name}!`);

interp.evalGlobal('(greet "Hello" "world")').value; // "Hello, world!"
```

### Returning Structured Data

Return a JSON string for objects/arrays — they'll be converted to Sema maps/lists:

```js
interp.registerFunction('get-user', (id) => {
  return JSON.stringify({ name: "Alice", age: 30 });
});

interp.evalGlobal('(:name (get-user 1))').value; // "Alice"
```

### Capturing State

Use closures to share mutable state between JavaScript and Sema:

```js
let counter = 0;
interp.registerFunction('inc!', () => ++counter);

interp.evalGlobal('(inc!)').value; // "1"
interp.evalGlobal('(inc!)').value; // "2"
```

::: info Value Conversion
Arguments are passed as native JS values (numbers, strings, booleans, arrays, objects). Return values are automatically converted: numbers, booleans, `null`/`undefined` → nil, strings, and JSON-stringified objects/arrays are all supported. Non-JSON-serializable values (functions, symbols, circular references) are not supported.
:::

## Preloading Modules

Use `preloadModule` to inject virtual modules that can be imported with `(import "name")` — no filesystem needed:

```js
const interp = new SemaInterpreter();

interp.preloadModule('utils', `
  (define (double x) (* x 2))
  (define pi 3.14159)
`);

interp.evalGlobal(`
  (import "utils")
  (double pi)
`).value; // "6.28318"
```

### Selective Exports

Use `(module ...)` with `(export ...)` to control which bindings are visible:

```js
interp.preloadModule('math', `
  (module math (export square cube)
    (define (square x) (* x x))
    (define (cube x) (* x x x))
    (define internal-helper 42))
`);

interp.evalGlobal(`
  (import "math" square)
  (square 5)
`).value; // "25"
```

## Persistent Definitions

Use `evalGlobal` to build up state across multiple calls — this is the key pattern for embedding:

```js
// Define functions
interp.evalGlobal(`
  (define (greet name)
    (string/append "Hello, " name "!"))
`);

// Define data
interp.evalGlobal('(define users (list "Alice" "Bob" "Carol"))');

// Use them together
const result = interp.evalGlobal('(map greet users)');
console.log(result.value); // ("Hello, Alice!" "Hello, Bob!" "Hello, Carol!")
```

## Virtual Filesystem

The WASM build includes an in-memory virtual filesystem. Files persist for the interpreter's lifetime but are lost on page reload:

```js
interp.evalGlobal('(file/write "/config.json" "{\\"key\\": \\"value\\"}")');

const result = interp.evalGlobal('(file/read "/config.json")');
console.log(result.value); // "{\"key\": \"value\"}"
```

Quotas apply: 1 MB per file, 16 MB total, 256 files max.

## Virtual Filesystem (from JavaScript)

The VFS can also be accessed directly from JavaScript, enabling file browser UIs, script editors, and pre-seeded environments:

### Seeding Files

```js
const sema = new SemaInterpreter();

// Write files from JS
sema.writeFile("/lib/math.sema", "(define (square x) (* x x))");
sema.writeFile("/main.sema", '(import "/lib/math") (square 7)');

// Run the script
sema.evalGlobal('(load "/main.sema")'); // => 49
```

### Building a File Browser

```js
sema.mkdir("/src");
sema.writeFile("/src/app.sema", "(println \"hello\")");
sema.writeFile("/src/utils.sema", "(define pi 3.14)");
sema.writeFile("/README.md", "# My Project");

sema.listFiles("/");      // ["README.md", "src"]
sema.listFiles("/src");   // ["app.sema", "utils.sema"]
sema.isDirectory("/src"); // true
sema.fileExists("/src/app.sema"); // true
```

### Reading Files Back

```js
const source = sema.readFile("/src/app.sema"); // "(println \"hello\")"
const missing = sema.readFile("/nope");        // null
```

### Quota Management

```js
const stats = sema.vfsStats();
// { files: 3, bytes: 62, maxFiles: 256, maxBytes: 16777216, maxFileBytes: 1048576 }

// Clear everything
sema.resetVFS();
sema.vfsStats(); // { files: 0, bytes: 0, ... }
```

Quotas: 1 MB per file, 16 MB total, 256 files max.

## VFS Persistence

By default, VFS files live only in WASM memory and are lost on page reload. To persist files across sessions, pass a **VFS backend** when creating the interpreter:

```js
import { SemaInterpreter, IndexedDBBackend } from "@sema-lang/sema";

const sema = await SemaInterpreter.create({
  vfs: new IndexedDBBackend({ namespace: "my-project" }),
});

// Sema code can read/write files as usual
await sema.evalStrAsync('(file/write "/config.json" "{\\"theme\\": \\"dark\\"}")');

// Persist current VFS state to the backend
await sema.flushVFS();

// On next page load, files are automatically restored via hydrate()
```

### Built-in Backends

Four backends ship with the `@sema-lang/sema` package:

| Backend | Import | Persistence | Limit |
| --- | --- | --- | --- |
| `MemoryBackend` | `@sema-lang/sema` | None — lost on reload | WASM quota only |
| `LocalStorageBackend` | `@sema-lang/sema` | Across page loads, per origin | ~5–10 MB |
| `SessionStorageBackend` | `@sema-lang/sema` | Within the current tab | ~5–10 MB |
| `IndexedDBBackend` | `@sema-lang/sema` | Across page loads, per origin | Hundreds of MB |

::: tip Choosing a Backend
Use **`IndexedDBBackend`** for production apps — it handles large file sets and doesn't compete with other localStorage usage. Use **`LocalStorageBackend`** for quick prototypes. Use **`MemoryBackend`** (or no backend) when persistence isn't needed.
:::

### Backend Options

All backends accept a `namespace` option to isolate storage:

```js
// Two interpreters with separate persistent storage
const editor = await SemaInterpreter.create({
  vfs: new IndexedDBBackend({ namespace: "editor-files" }),
});
const preview = await SemaInterpreter.create({
  vfs: new IndexedDBBackend({ namespace: "preview-files" }),
});
```

### Flush and Reset

```js
// Persist VFS to the backend (call after eval)
await sema.flushVFS();

// Clear both VFS memory and persistent storage
await sema.resetVFSAndBackend();
```

### Custom Backends

Implement the `VFSBackend` interface to persist files anywhere — a remote API, a service worker cache, or a custom database:

```ts
import type { VFSBackend, VFSHost } from "@sema-lang/sema";

class CloudBackend implements VFSBackend {
  async init() {
    // Open connections, authenticate, etc.
  }

  async hydrate(host: VFSHost) {
    // Fetch files from your API and write them into the WASM VFS
    const files = await fetch("/api/files").then(r => r.json());
    for (const { path, content } of files) {
      host.writeFile(path, content);
    }
  }

  async flush(host: VFSHost) {
    // Read files from the WASM VFS and upload them
    const paths = host.listFiles("/");
    for (const name of paths) {
      const content = host.readFile("/" + name);
      if (content !== null) {
        await fetch("/api/files/" + name, {
          method: "PUT",
          body: content,
        });
      }
    }
  }

  async reset() {
    await fetch("/api/files", { method: "DELETE" });
  }
}

const sema = await SemaInterpreter.create({
  vfs: new CloudBackend(),
});
```

::: info VFSHost API
The `host` object passed to `hydrate()` and `flush()` provides these methods:
- `readFile(path)` → `string | null`
- `writeFile(path, content)` — write or overwrite a file
- `deleteFile(path)` → `boolean`
- `mkdir(path)` — create directories recursively
- `listFiles(dir)` → `string[]` — list entries in a directory
- `fileExists(path)` → `boolean`
- `isDirectory(path)` → `boolean`
- `resetVFS()` — clear all files and directories
:::

## Real-World Example: User-Scriptable Web App

A web application that lets users write Sema scripts to customize behavior. The host app evaluates user scripts and uses the results:

### HTML

```html
<div id="app">
  <textarea id="script" rows="8" cols="60">
(define (transform items)
  (filter (lambda (item) (> (:score item) 50))
    (map (lambda (item)
           (assoc item :label
             (string/append (:name item) " (" (number/to-string (:score item)) ")")))
         items)))
  </textarea>
  <button id="run">Run Transform</button>
  <pre id="output"></pre>
</div>
```

### JavaScript

```js
import init, { SemaInterpreter } from '@sema-lang/sema-wasm';

await init();
const interp = new SemaInterpreter();

// Preload sample data
interp.evalGlobal(`
  (define sample-data
    (list
      {:name "Alice" :score 85}
      {:name "Bob"   :score 42}
      {:name "Carol" :score 91}
      {:name "Dave"  :score 33}))
`);

document.getElementById('run').addEventListener('click', () => {
  const script = document.getElementById('script').value;

  // Load user's function definition
  const loadResult = interp.evalGlobal(script);
  if (loadResult.error) {
    document.getElementById('output').textContent = `Error: ${loadResult.error}`;
    return;
  }

  // Call the user's transform function with our data
  const result = interp.evalGlobal('(transform sample-data)');

  if (result.error) {
    document.getElementById('output').textContent = `Error: ${result.error}`;
  } else {
    document.getElementById('output').textContent = result.value;
    // Output: ({:label "Alice (85)" :name "Alice" :score 85}
    //          {:label "Carol (91)" :name "Carol" :score 91})
  }
});
```

## Multiple Interpreters

Each `SemaInterpreter` instance has fully isolated state — its own environment, virtual filesystem, module cache, and eval-step counter:

```js
const interpA = new SemaInterpreter();
const interpB = new SemaInterpreter();

interpA.evalGlobal('(define x 1)');
interpB.evalGlobal('(define x 2)');

interpA.evalGlobal('x').value; // "1"
interpB.evalGlobal('x').value; // "2"
```

## CDN Usage

### jsdelivr

```
https://cdn.jsdelivr.net/npm/@sema-lang/sema-wasm/sema_wasm.js
https://cdn.jsdelivr.net/npm/@sema-lang/sema-wasm/sema_wasm_bg.wasm
```

### unpkg

```
https://unpkg.com/@sema-lang/sema-wasm/sema_wasm.js
https://unpkg.com/@sema-lang/sema-wasm/sema_wasm_bg.wasm
```

### Complete HTML Page

```html
<!DOCTYPE html>
<html>
<head>
  <meta charset="utf-8">
  <title>Sema Playground</title>
</head>
<body>
  <textarea id="code" rows="6" cols="50">(println "Hello from Sema!")
(+ 40 2)</textarea>
  <button id="run">Run</button>
  <pre id="output"></pre>

  <script type="module">
    import init, { SemaInterpreter } from
      'https://cdn.jsdelivr.net/npm/@sema-lang/sema-wasm/sema_wasm.js';

    await init('https://cdn.jsdelivr.net/npm/@sema-lang/sema-wasm/sema_wasm_bg.wasm');
    const interp = new SemaInterpreter();

    document.getElementById('run').addEventListener('click', async () => {
      const code = document.getElementById('code').value;
      const result = await interp.evalAsync(code);

      let text = '';
      if (result.output.length > 0) {
        text += result.output.join('\n') + '\n';
      }
      if (result.error) {
        text += 'Error: ' + result.error;
      } else if (result.value) {
        text += '=> ' + result.value;
      }
      document.getElementById('output').textContent = text;
    });
  </script>
</body>
</html>
```

## Limitations

Compared to the [Rust embedding API](./embedding), the WASM/JavaScript embedding has these differences:

| Feature | Rust | JavaScript (WASM) |
| --- | --- | --- |
| Filesystem | Real filesystem | In-memory VFS with pluggable persistence (1 MB/file, 16 MB total) |
| Shell access | `(shell ...)` works | Not available |
| `registerFunction` | Register native Rust closures | `registerFunction` with native JS value args |
| LLM functions | Full provider support | Not available in browser |
| HTTP | Synchronous (reqwest) | Async via `fetch()` (CORS restrictions apply) |
| Sandbox/Caps | Fine-grained capability control | Inherently sandboxed by the browser |
| Threading | Single-threaded (`Rc`) | Single-threaded (WASM) |
| Eval step limit | Unlimited by default | 10M steps (prevents tab freezes) |
| `stdin` / `io/read-line` | Works | Not available |

### Workarounds

- **No LLM**: Use JavaScript to call LLM APIs, then pass results into Sema via `registerFunction` or `evalGlobal`.
- **Persistence**: Use a VFS backend (`IndexedDBBackend`, `LocalStorageBackend`) to persist files across page reloads. See [VFS Persistence](#vfs-persistence).

## API Reference

| Type / Method | Description |
| --- | --- |
| `init(wasmUrl?)` | Initialize the WASM module. Call once before creating interpreters. Pass URL when using CDN. |
| `SemaInterpreter` | Interpreter instance with isolated state |
| `new SemaInterpreter()` | Create an interpreter with full stdlib and 10M step limit |
| `SemaInterpreter.createWithOptions(opts)` | Create with options: `{ stdlib, deny }`. Use `deny` to restrict capabilities. |
| `interp.eval(code)` | Evaluate in a child env (definitions don't persist). Returns a JS object `{ value, output, error }`. |
| `interp.evalGlobal(code)` | Evaluate in global env (definitions persist). Returns a JS object `{ value, output, error }`. |
| `interp.evalAsync(code)` | Async eval with HTTP support. Returns a `Promise<object>`. |
| `interp.evalVM(code)` | Evaluate via the bytecode VM (alias of `eval`). Returns a JS object `{ value, output, error }`. |
| `interp.evalVMAsync(code)` | Async VM eval with HTTP support (alias of `evalAsync`). Returns a `Promise<object>`. |
| `interp.registerFunction(name, fn)` | Register a JS function callable from Sema. Args passed as native JS values. |
| `interp.preloadModule(name, source)` | Inject a virtual module for `(import "name")`. Returns `{ ok, error }`. |
| `interp.readFile(path)` | Read a VFS file. Returns string or null. |
| `interp.writeFile(path, content)` | Write a file to the VFS. |
| `interp.deleteFile(path)` | Delete a VFS file. Returns boolean. |
| `interp.listFiles(dir)` | List entries in a VFS directory. |
| `interp.fileExists(path)` | Check if path exists in VFS. |
| `interp.mkdir(path)` | Create a directory in the VFS. |
| `interp.isDirectory(path)` | Check if path is a directory. |
| `interp.vfsStats()` | Get VFS usage stats (files, bytes, quotas). |
| `interp.resetVFS()` | Clear all VFS state. |
| `interp.flushVFS()` | Persist VFS to the configured backend. Returns `Promise<void>`. |
| `interp.resetVFSAndBackend()` | Clear VFS and persistent backend. Returns `Promise<void>`. |
| `MemoryBackend` | Ephemeral VFS backend — no persistence |
| `LocalStorageBackend` | Persist VFS to `localStorage` (~5 MB limit) |
| `SessionStorageBackend` | Persist VFS to `sessionStorage` (per-tab) |
| `IndexedDBBackend` | Persist VFS to IndexedDB (recommended for production) |
| `VFSBackend` | Interface for custom backends: `{ init?, hydrate, flush, reset? }` |
| `VFSHost` | Host bridge passed to backends: `{ readFile, writeFile, deleteFile, mkdir, listFiles, fileExists, isDirectory, resetVFS }` |
| `interp.version()` | Returns the Sema version string |
| `EvalResult` | `{ value: string \| null, output: string[], error: string \| null }` |
