# Sema Web Framework Gaps Roadmap

**Status:** Exploratory plan - 2026-07-02. Implementation not started.

Sema Web now has the core pieces of a small browser framework: DOM bindings,
SIP rendering, reactive state, components, scoped CSS, a hash router, streaming
HTTP/SSE, browser storage, compiled `.vfs` loading, and an optional LLM proxy.
The next step is to close the gaps that users expect from a modern framework
without turning Sema Web into a React clone.

## Goal

Make Sema Web feel complete for small-to-medium browser apps: predictable
components, ergonomic routing, first-class async data, clear diagnostics,
documented deployment, and testing/performance guardrails.

## Current Strengths

- Direct browser scripting with `<script type="text/sema">` and external
  `.sema` files.
- Production archive loading via `.vfs`.
- Declarative SIP rendering with error isolation.
- Reactive components built on `@preact/signals-core`.
- Component-owned cleanup for signals, watches, intervals, streams, listeners,
  and local state.
- Fetch-based SSE with headers, credentials, POST bodies, explicit close, and
  component ownership.
- LLM proxy bindings that keep provider keys out of the browser.
- A growing unit/E2E/performance harness under `packages/sema-web/`.

## Framework Gaps

### 1. Component Props, Children, and Composition

Current components can be functions and can be mounted by name, but the framework
does not yet have a polished composition story.

Expected modern behavior:

- `(mount! "#app" app-view {:user user :theme "dark"})` passes initial props.
- Components can call other components from SIP without manual string dispatch.
- Children can be passed into reusable layout components.
- Props have a documented shape and keyword/string-key normalization.
- Component function failures include component name and props context.

Candidate API:

```sema
(defcomponent app-view (props)
  [:main
    (component/render todo-list {:items (:items props)})
    (component/render shell-layout {}
      [:p "Nested content"])])

(mount! "#app" app-view {:items initial-items})
```

Implementation notes:

- Modify `packages/sema-web/src/component.ts`.
- Keep `defcomponent` as a macro over plain functions.
- Add JS internals for component invocation with args instead of using only
  `interp.invokeGlobal(component.componentFn)`.
- Add unit tests in `packages/sema-web/tests/component.test.ts`.
- Add an E2E fixture that mounts a layout component with nested children.

Acceptance:

- Props pass through `mount!`, `component/mount!`, and `component/render`.
- Nested components rerender when their own tracked signals change.
- Child SIP nodes render in the correct position and survive parent rerenders.
- Invalid component names and render failures still route through `ctx.onerror`.

### 2. Keyed Lists and Stable DOM Identity

SIP currently relies on `morphdom` defaults. For dynamic lists, modern
frameworks need a stable identity model so focus, input values, animations, and
external DOM state do not jump between rows.

Expected modern behavior:

- `{:key item-id}` marks a SIP node as stable across reorders.
- Reordering keyed children preserves the DOM node for each key.
- Duplicate keys produce a development warning.
- Missing keys on obvious repeated structures can be linted later, not required
  at runtime.

Candidate API:

```sema
[:ul
  (map (fn (todo)
         [:li {:key (:id todo)} (:title todo)])
       @todos)]
```

Implementation notes:

- Modify `packages/sema-web/src/sip.ts` to convert `:key` into a `data-sema-key`
  or `id`-compatible morphdom key hook.
- Modify `packages/sema-web/src/component.ts` to configure `morphdom` `getNodeKey`.
- Avoid exposing the key as a user-visible DOM attribute unless needed for
  debugging.
- Add weird tests: reorder focused inputs, duplicate keys, numeric keys, keyword
  keys, removed keyed nodes releasing handles.

Acceptance:

- A focused input in a keyed list keeps focus and typed value when sibling rows
  reorder.
- Removing a keyed row releases handles for its subtree.
- Duplicate keys call `ctx.onerror` in development mode and still render.

### 3. Lifecycle and Effects

`on-mount` exists and can return cleanup, but users will expect explicit
component-scoped effects for subscriptions, observers, timers, and resources.

Expected modern behavior:

- `(on-unmount fn)` is a direct, discoverable cleanup hook.
- `(effect deps fn)` runs after render, reruns when dependencies change, and
  calls cleanup before rerun/unmount.
- Effects created during render are owned by the component and automatically
  disposed.
- Callback errors route through `ctx.onerror`.

Candidate API:

```sema
(defcomponent clock-view ()
  (def now (local :now (date/now)))

  (effect (list)
    (let ((id (js/set-interval (fn () (put! now (date/now))) 1000)))
      (fn () (js/clear-interval id))))

  [:time @now])
```

Implementation notes:

- Extend `MountedComponent` in `packages/sema-web/src/context.ts` with owned
  effect cleanup registrations.
- Add internals and wrappers in `packages/sema-web/src/component.ts`.
- Reuse callback conversion helpers in `packages/sema-web/src/callbacks.ts`.
- Tests should cover cleanup order, rerun cleanup, errors, and unmount disposal.

Acceptance:

- Effects do not leak after `component/unmount!` or `web.dispose()`.
- Cleanup runs once per effect lifecycle.
- Errors in effect body and cleanup route through `ctx.onerror`.

### 4. Async Data Resources

Sema Web has `evalAsync`, browser `http/*`, and SSE streams, but no framework
primitive for "load data, expose loading/error/value, rerender when it changes".

Expected modern behavior:

- A component can create a resource owned by the component.
- The resource exposes `:loading`, `:value`, `:error`, and `:refresh`.
- In-flight requests are abortable on unmount.
- Stale responses cannot overwrite newer refreshes.

Candidate API:

```sema
(defcomponent profile-view (props)
  (def user
    (resource
      (fn () (http/get (string-append "/api/users/" (:id props))))))

  (cond
    (:loading @user) [:p "Loading"]
    (:error @user)   [:p {:class "error"} (:error @user)]
    :else            [:h1 (:name (:value @user))]))
```

Implementation notes:

- Likely new file: `packages/sema-web/src/resource.ts`.
- Register `resource`, `resource/refresh!`, `resource/cancel!`.
- Integrate with `SemaWeb.create()` options in `packages/sema-web/src/index.ts`.
- Test with fake `fetch`, aborted fetch, out-of-order responses, and unmount.

Acceptance:

- Refreshing a slow resource twice keeps the latest response.
- Unmount aborts the resource and removes owned stream/request state.
- Resource errors are visible in the signal and also route through `ctx.onerror`.

### 5. Router Upgrade

The current router is hash-based with simple path params. It is enough for demos,
but larger apps expect query params, fallback routes, link helpers, and route
lifecycle behavior.

Expected modern behavior:

- Query strings parse into `:query`.
- A not-found route can be registered.
- `(router/link path label attrs)` renders an accessible anchor.
- Route changes can reset focus and optionally scroll to top.
- Optional history mode can use `pushState` when the host server is configured.

Candidate API:

```sema
(router/init!
  {:mode :hash
   :not-found "not-found-view"
   :routes {"/" "home-view"
            "/todos/:id" "todo-detail-view"}})

(router/link "/todos/42" "Open todo" {:class "nav-link"})
```

Implementation notes:

- Modify `packages/sema-web/src/router.ts`.
- Keep hash mode as the default.
- Add tests for query decoding, malformed query strings, fallback route,
  link click handling, and cleanup on re-init.
- Add E2E coverage for navigation preserving component cleanup.

Acceptance:

- `router/current-route` returns `{path, params, query, handler}`.
- Unknown routes return the not-found handler when configured.
- Link clicks update the route without full page reload.

### 6. Event Modifiers and Form Helpers

SIP event attributes currently point at handler names. Users will expect common
event modifiers and basic form ergonomics.

Expected modern behavior:

- Event modifiers: `.prevent`, `.stop`, `.once`, `.capture`, `.self`.
- Form data extraction from a form event or form handle.
- Checkbox, radio, select, and multi-select value helpers.
- Handler context exposes target, current target, key, value, checked, and form.

Candidate API:

```sema
[:form {:on-submit.prevent "save"}
  [:input {:name "title" :value @title :on-input "set-title"}]
  [:button {:type "submit"} "Save"]]

(define (save ev)
  (def fields (dom/event-form-data ev))
  ...)
```

Implementation notes:

- Modify `packages/sema-web/src/sip.ts` event attribute parsing.
- Modify `EventDelegator` in `packages/sema-web/src/component.ts`.
- Extend `packages/sema-web/src/dom.ts` event helpers.
- Test event modifier combinations, nested forms, disabled inputs, file inputs,
  repeated field names, and non-element event targets.

Acceptance:

- `.prevent` prevents default before the handler runs.
- `.stop` stops delegated bubbling after the handler.
- `dom/event-form-data` handles repeated names as lists.

### 7. Diagnostics and Dev Mode

Errors now route through `ctx.onerror`, but the public API does not yet expose a
complete diagnostics story for script authors.

Expected modern behavior:

- `SemaWeb.create({ onerror })` installs the app-level error hook.
- Optional dev mode records recent errors, component renders, route changes,
  stream states, and slow renders.
- Loader errors include script URL or inline script index.
- A small dev overlay can be enabled for demos and local apps.

Candidate API:

```js
await SemaWeb.create({
  dev: true,
  onerror(error, context) {
    report(error, { context });
  },
});
```

Implementation notes:

- Modify `packages/sema-web/src/index.ts` and `packages/sema-web/src/context.ts`.
- Add a bounded diagnostics ring buffer to `SemaWebContext`.
- Optional overlay can live in `packages/sema-web/src/devtools.ts`.
- Add tests for custom `onerror`, loader context strings, bounded history, and
  overlay cleanup on dispose.

Acceptance:

- Every existing `ctx.onerror` path can be observed through the public option.
- `web.context` exposes enough diagnostics for tests and app tooling.
- Dev overlay does not ship unless enabled.

### 8. Browser LLM Proxy Hardening

`llm.ts` remains the weakest-covered sema-web file. The API is useful, but it
needs a stricter contract before users build browser agents on it.

Expected modern behavior:

- Proxy request/response schema is documented and tested.
- Streaming and non-streaming calls normalize errors the same way.
- Stream cancellation updates signal state and aborts network work.
- Proxy auth headers cannot be accidentally overwritten in unsafe ways.
- Tests cover malformed JSON, missing fields, model lists, embeddings, and
  classification responses.

Implementation notes:

- Extend `packages/sema-web/tests/llm.test.ts`.
- Add E2E fixture for `llm/chat-stream` cancellation and proxy error events.
- Consider a small shared normalizer for proxy responses in
  `packages/sema-web/src/llm.ts`.

Acceptance:

- `llm.ts` coverage reaches the same standard as the rest of `src/`.
- Stream and non-stream proxy errors produce predictable Sema values/errors.
- Component-owned LLM streams close on unmount.

### 9. Build and Tooling DX

Sema Web can load source and archives, but app authors will expect a smoother
development path.

Expected modern behavior:

- Vite plugin for `.sema` files and `.vfs` archive generation.
- Watch mode for `sema build --target web`.
- Clear CDN, npm, and Vite quickstarts.
- CSP guidance for apps that disallow inline scripts.
- Bundle-size and runtime performance budgets in CI.

Candidate deliverables:

- `packages/vite-plugin-sema/` or `packages/sema-web/vite-plugin.ts`.
- `npm create sema-web` template after the API stabilizes.
- `packages/sema-web/bench/` promoted into CI with threshold reporting.

Acceptance:

- A new app can be created, run, tested, built, and deployed from documented
  commands.
- Benchmark regressions are visible in CI without making local iteration noisy.

### 10. Testing Utilities

Framework users need a way to test Sema Web components without hand-rolling
interpreter setup.

Expected modern behavior:

- `renderSema()` mounts a Sema component into JSDOM.
- Test helpers can fire DOM events and read signals.
- Tests can assert `ctx.onerror` calls and cleanup state.
- Helpers support both source strings and compiled archive fixtures.

Candidate API:

```ts
const screen = await renderSema(`
  (def count (state 0))
  (defcomponent view () [:button {:on-click "inc"} @count])
  (define (inc ev) (update! count inc))
`, { mount: "view" });

await screen.click("button");
expect(screen.text("button")).toBe("1");
```

Implementation notes:

- New file: `packages/sema-web/src/testing.ts` or separate test-only export.
- Avoid shipping large test-only dependencies in the production bundle.
- Dogfood it by migrating a few existing tests.

Acceptance:

- Users can test a component in under 20 lines.
- Cleanup assertions prove no listeners, handles, streams, or signals leak.

## Documentation Gaps

These should move with the feature work, not after it.

- **Quickstart:** one Vite-based tutorial that ends in a running component app.
- **Mental model:** explain `SemaWeb`, the interpreter, `SemaWebContext`,
  handles, signals, SIP, components, and ownership cleanup.
- **Components guide:** props, local state, lifecycle, effects, events, keyed
  lists, and cleanup rules.
- **Routing guide:** route maps, params, query strings, not-found routes, links,
  and deployment implications of hash/history mode.
- **Async data guide:** HTTP, SSE, resources, cancellation, and component-owned
  cleanup.
- **LLM proxy guide:** required endpoints, auth, streaming event format, failure
  modes, and a minimal proxy implementation.
- **Deployment guide:** `.sema` source vs `.vfs`, CDN vs npm, CSP, caching,
  archive version mismatch behavior, and sema/wasm asset paths.
- **API reference:** generate or maintain one page per namespace:
  `dom/*`, `store/*`, `state/*`, `sip/*`, `component/*`, `router/*`, `css/*`,
  `http/*`, `llm/*`, `console/*`.

## Suggested Order

1. Public diagnostics option and LLM proxy coverage. Low design risk, high
   confidence, closes known weak coverage.
2. Component props/children and keyed lists. These shape almost every future app.
3. Lifecycle/effects and async resources. These prevent leaks and make real apps
   ergonomic.
4. Router upgrade and event/form helpers. These round out application workflows.
5. Tooling, testing utilities, and docs expansion. These make the framework
   teachable and repeatable.

## Verification Plan

Every feature should add:

- Focused Vitest unit tests under `packages/sema-web/tests/`.
- At least one real Playwright E2E path when the feature affects browser
  behavior, lifecycle, routing, focus, streams, or archive loading.
- Coverage check with `npm run test:coverage --workspace=@sema-lang/sema-web`.
- Build check with `npm run build --workspace=@sema-lang/sema-web`.
- Headed smoke when requested:
  `npm run test:e2e --workspace=@sema-lang/sema-web -- --workers=1 --headed`.

## Non-Goals for the Next Pass

- No server-side rendering or hydration until client-side composition, keyed
  identity, and async resources are stable.
- No router data loaders until the resource primitive exists.
- No full design-system/component-library layer inside `sema-web`.
- No browser-side direct provider API keys for LLM calls.
