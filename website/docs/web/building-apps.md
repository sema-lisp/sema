# Building a Sema Web App

This guide is the recommended path for building a real Sema Web application, not just embedding a small script in a page.

The key idea is simple:

- use `.sema` source in development
- ship a compiled `.vfs` archive in production
- keep the same HTML shape in both modes

## Recommended Project Shape

Start with a small, explicit structure:

```text
my-app/
  index.html
  app.sema
  public/
```

- `index.html` bootstraps `SemaWeb`
- `app.sema` contains your Sema UI, state, and components
- `public/` is where you emit `app.vfs` for production

For a larger app, split your Sema code into modules and keep `app.sema` as the entry point.

## Development Flow

During development, point the page at source:

```html
<script type="text/sema" src="/app.sema"></script>
<script type="module">
  import { SemaWeb } from "@sema-lang/sema-web";
  await SemaWeb.init();
</script>
```

That keeps iteration fast and makes the app easy to inspect.

Recommended structure inside `app.sema`:

1. Define top-level state first.
2. Define derived state with `computed`.
3. Define actions and event handlers.
4. Define presentational helpers and components.
5. Mount one root component at the end.

```scheme
(def tasks (state '()))
(def draft (state ""))

(define (set-draft ev)
  (put! draft (dom/event-value ev)))

(defcomponent app ()
  [:div
    [:input {:value @draft :on-input "set-draft"}]
    [:p "Tasks: " (length @tasks)]])

(mount! "#app" "app")
```

## Production Flow

For production, compile the entry file to a `.vfs` archive:

```bash
sema build --target web app.sema -o public/app.vfs
```

Then switch the script tag from source to the compiled artifact:

```html
<script type="text/sema" src="/public/app.vfs"></script>
<script type="module">
  import { SemaWeb } from "@sema-lang/sema-web";
  await SemaWeb.init();
</script>
```

The important part is that your JavaScript bootstrap does not need to change. `SemaWeb.init()` detects `.vfs` automatically.

## Suggested App Architecture

For production apps, this shape scales well:

- persistent application state in top-level `state` values
- derived views and counters in `computed`
- browser effects in `on-mount` and `watch`
- UI rendering in `defcomponent`
- routing in `router/*`
- visual styling in `css`

Use `watch` for persistence and side effects:

```scheme
(def todos (state (or (store/get "todos") '())))

(defcomponent app ()
  (on-mount
    (fn ()
      (let ((watch-id
              (watch todos
                (fn (old new)
                  (store/set! "todos" new)))))
        (fn ()
          (unwatch! watch-id)))))
  [:div ...])
```

That gives you explicit cleanup and keeps side effects out of render logic.

## `init()` vs `create()`

Use `SemaWeb.init()` when:

- your app is driven by `<script type="text/sema">`
- you want automatic script discovery
- you are building a static or embedded page

Use `SemaWeb.create({ autoLoad: false })` when:

- you want to evaluate code manually
- you need tighter JavaScript control over loading
- your app has a custom bootstrap flow

For most browser apps, `init()` is the right default.

## Static Hosting

If your app does not use `llm/*`, deployment is just static hosting:

- `index.html`
- `app.vfs`
- the built `@sema-lang/sema-web` assets from your normal frontend pipeline

If your app uses LLM features, keep the browser app static and deploy the proxy separately. See [LLM Proxy](./llm-proxy) and [Deployment](./deployment).

## Repository Example

This repository includes a complete compiled-app example at:

```text
examples/sema-web-app/
```

It demonstrates:

- compiled `.vfs` loading
- reactive state and `computed`
- `watch` cleanup via `on-mount`
- routing with `router/*`
- persistence with `store/*`
- scoped styles with `css`

From the repository root, build and run it with:

```bash
make sema-web-example
```

Then open:

```text
http://127.0.0.1:8788
```

That target builds the archive, copies the browser runtime files into `dist/vendor/`, and serves the example folder with `npx serve`.

## Related Guides

- [Getting Started](./getting-started)
- [Deployment](./deployment)
- [Examples](./examples)
