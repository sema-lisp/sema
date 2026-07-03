---
outline: [2, 3]
---

# Getting Started

::: tip Just want to run an app?
The fastest path is the [dev server](/docs/web/dev-server): `sema web app.sema`
serves your app in the browser with hot reload and a built-in LLM proxy — no
`npm install`, no bundler. The rest of this page covers embedding the runtime in
your own page.
:::

## Installation

```sh
npm install @sema-lang/sema-web
```

This pulls in the WASM binary (`@sema-lang/sema-wasm`) and all browser bindings. No additional dependencies needed.

## Minimal HTML Page

The fastest way to get running -- add two script tags:

```html
<!DOCTYPE html>
<html>
<body>
  <div id="app"></div>

  <!-- 1. Initialize the Sema runtime -->
  <script type="module">
    import { SemaWeb } from "@sema-lang/sema-web";
    await SemaWeb.init();
  </script>

  <!-- 2. Write Sema code -->
  <script type="text/sema">
    (def count (state 0))

    (define (increment ev)
      (update! count (fn (n) (+ n 1))))

    (defcomponent app ()
      [:div
        [:h1 "Counter: " @count]
        [:button {:on-click "increment"} "Click me"]])

    (mount! "#app" "app")
  </script>
</body>
</html>
```

`SemaWeb.init()` does three things:

1. Loads the WASM binary and creates an interpreter
2. Registers all browser bindings (DOM, reactive, components, etc.)
3. Discovers all `<script type="text/sema">` tags and evaluates them in order

::: tip Dev vs Prod
During development, pointing a script tag at `.sema` source is fine. For production, build a compiled `.vfs` archive and keep the same HTML shape. See [Building a Sema Web App](./building-apps) and [Deployment](./deployment).
:::

## Manual Usage

For more control, use `SemaWeb.create()` with `autoLoad: false`:

```js
import { SemaWeb } from "@sema-lang/sema-web";

const web = await SemaWeb.create({ autoLoad: false });

// Evaluate code directly
web.eval('(def greeting "Hello from Sema!")');
web.eval('(dom/set-text! (dom/query "#app") greeting)');

// Register JS functions callable from Sema
web.registerFunction("get-timestamp", () => Date.now());
web.eval('(console/log "Time:" (get-timestamp))');

// Preload modules for (import ...)
web.preloadModule("utils", `
  (export (define (double x) (* x 2)))
`);
web.eval('(import "utils") (console/log (double 21))');
```

### Async Evaluation

For code that uses `http/get` or other async operations:

```js
const result = await web.evalAsync('(http/get "https://api.example.com/data")');
```

## SemaWebOptions Reference

Pass options to `SemaWeb.init()` or `SemaWeb.create()`:

```js
const web = await SemaWeb.create({
  // Auto-discover <script type="text/sema"> tags (default: true)
  autoLoad: true,

  // Feature flags — disable modules you don't need
  dom: true,          // dom/* namespace
  store: true,        // store/* namespace (localStorage/sessionStorage)
  reactive: true,     // state, put!, update!, computed, batch, watch
  sip: true,          // sip/* namespace (SIP rendering)
  components: true,   // mount!, defcomponent, local, on-mount
  router: true,       // router/* namespace (hash-based SPA routing)
  css: true,          // css/* namespace (scoped styles)
  http: true,         // http/* namespace (fetch, SSE)
  console: true,      // console/* namespace

  // LLM proxy — forward llm/* calls to a backend server
  llmProxy: "https://api.example.com/llm",
  // or with full options:
  llmProxy: {
    url: "https://api.example.com/llm",
    token: "user-session-token",
    timeout: 30000,
  },
});
```

### Feature Flags

Every binding module can be individually disabled. This is useful for:

- **Security** -- disable `dom` or `http` for sandboxed evaluation
- **Bundle size** -- tree-shaking removes unused modules
- **Testing** -- isolate reactive logic without DOM side effects

Enabling `components` automatically enables `reactive` and `sip` (components depend on both). You can still explicitly disable them with `reactive: false`, which overrides the auto-enable.

```js
// Minimal: only reactive state, no DOM/components
const web = await SemaWeb.create({
  autoLoad: false,
  dom: false,
  store: false,
  sip: false,
  components: false,
  router: false,
  css: false,
  http: false,
});

web.eval('(def x (state 10))');
web.eval('(put! x 20)');
const result = web.eval('@x');
console.log(result.value); // "20"
```

## Eval Results

Every `web.eval()` call returns an `EvalResult`:

```ts
interface EvalResult {
  value: string | null;   // Stringified return value, or null
  output: string[];       // Lines printed to stdout (via display, println, etc.)
  error: string | null;   // Error message, or null on success
}
```

Example:

```js
const result = web.eval('(+ 1 2)');
// { value: "3", output: [], error: null }

const result2 = web.eval('(println "hi") (+ 1 2)');
// { value: "3", output: ["hi"], error: null }

const result3 = web.eval('(/ 1 0)');
// { value: null, output: [], error: "Division by zero" }
```

## Multiple Instances

Each `SemaWeb.create()` call returns an independent instance with its own interpreter, state, and DOM bindings. No shared globals:

```js
const app1 = await SemaWeb.create({ autoLoad: false });
const app2 = await SemaWeb.create({ autoLoad: false });

app1.eval('(def x (state 1))');
app2.eval('(def x (state 2))');
// These are completely independent
```

## Cleanup

When you are done with an instance, call `dispose()` to free the WASM memory:

```js
web.dispose();
// web.eval(...) will throw after this
```

## What's Next

- [Reactive State](./reactive-state) -- signals, computed values, and watchers
- [Components](./components) -- building interactive UIs with auto-re-rendering
- [Building a Sema Web App](./building-apps) -- recommended project structure and production flow
- [HTTP & Streams](./http) -- browser fetch integration and SSE streams
- [Overview](./index) -- architecture and full feature list
