# Router

The `router/*` namespace provides an SPA router built on signals. Routes are declared as a map of URL patterns to handler names, and the current route is exposed as a reactive signal, so any component that reads it re-renders on navigation.

## Setup

### `(router/init! routes-or-options)` -> nil

Register routes and start listening for URL changes. Two forms are accepted.

A bare map of pattern to handler name:

```sema
(router/init! {"/" "home-page"
               "/todos" "todo-list"
               "/todos/:id" "todo-detail"
               "/settings" "settings-page"})
```

Or an options map, which is a map whose `:routes` key holds a map:

```sema
(router/init!
  {:mode :hash                    ;; :hash (default) or :history
   :not-found "missing-page"      ;; handler for an unmatched path
   :scroll-to-top true            ;; scroll to the top on navigation
   :focus "#main"                 ;; move focus there on navigation
   :routes {"/" "home-page"
            "/todos/:id" "todo-detail"}})
```

| Option | Values | Default | Effect |
|---|---|---|---|
| `:routes` | map | required in this form | pattern to handler name |
| `:mode` | `:hash`, `:history` | `:hash` | `:history` uses real paths via `pushState` |
| `:not-found` | handler name | none | used when no pattern matches |
| `:scroll-to-top` | `true`/`false` | `false` | scroll to the top after a route change |
| `:focus` | `true` or selector | none | `true` returns focus to the document; a selector focuses that element |

The two forms are told apart by what `routes` holds, not by the key's name: a map means the options form, a handler name string means the ordinary route `/routes` (a bare table's leading slash is optional, so `{"routes" "routes-page"}` is just that route).

Calling `router/init!` again replaces the routes and re-resolves the current URL; the previous listeners are removed, so re-initializing never doubles up handlers.

A pattern may hold any character a URL segment can: `{"/søk" "search-page"}` and `{"/a b" "spaced-page"}` match even though the browser stores those URLs percent-encoded (`#/s%C3%B8k`, `#/a%20b`).

`:mode :history` requires the host server to serve the app shell for every route, since a deep link is a real request. `:hash` requires nothing from the host, which is why it is the default.

## Navigation

### `(router/push! path)` -> nil

Navigate to a path, adding a history entry so the back button works.

```sema
(router/push! "/todos/42?tab=open")
```

### `(router/replace! path)` -> nil

Navigate without adding a history entry. Useful for redirects.

```sema
(router/replace! "/login")
```

### `(router/back!)` -> nil

Go back one entry in the browser history.

A path that leaves the app (`https://…`, `//host`, `javascript:`) is refused rather than followed, and reported through the app's error hook.

## Reading the Current Route

### `(router/current)` -> signal-id

Returns the signal ID for the current route match. Use with `deref` or `watch`.

### `(router/current-route)` -> map | nil

Convenience wrapper (defined in Sema) that dereferences the route signal. Returns a map with `:path`, `:params`, `:query`, and `:handler`:

```sema
(router/current-route)
;; => {:path "/todos/42" :params {:id "42"} :query {:tab "open"} :handler "todo-detail"}
```

It returns `nil` only when nothing matched *and* no `:not-found` handler is registered.

## Route Parameters

Route patterns support named parameters with the `:param` syntax. Parameters match any non-slash segment and are percent-decoded:

| Pattern | URL | Params |
|---------|-----|--------|
| `/todos/:id` | `/todos/42` | `{:id "42"}` |
| `/users/:uid/posts/:pid` | `/users/5/posts/99` | `{:uid "5" :pid "99"}` |
| `/` | `/` | `{}` |

Parameter names must match `[a-zA-Z_][a-zA-Z0-9_]*`. Paths are normalized to a leading `/`, so `"todos"` and `"/todos"` name one route.

## Query Strings

The query string is parsed into `:query` and never participates in matching, so a pattern describes the path alone (a `?` in a pattern starts a query string there too, and is ignored).

| URL | `:query` |
|---|---|
| `/todos?tab=open` | `{:tab "open"}` |
| `/todos?tag=a&tag=b` | `{:tag ("a" "b")}` |
| `/todos?draft` | `{:draft ""}` |
| `/todos?q=hello+world` | `{:q "hello world"}` |
| `/todos?filter=a=b` | `{:filter "a=b"}` |

A repeated key collects its values into a list, matching how `dom/form-data` reports repeated field names. A malformed percent escape keeps its raw text instead of failing the route.

## Links

### `(router/link path [label] [attrs])` -> SIP

Returns SIP data for an accessible anchor whose clicks are intercepted, so navigation never reloads the page. `label` defaults to the path and `attrs` to no extra attributes:

```sema
[:nav
  (router/link "/" "Home" {:class "nav-link"})
  (router/link "/todos/42" "Open todo" {:class "nav-link"})]
```

- `aria-current="page"` is added when the link points at the current path (pass your own `:aria-current` to override).
- `label` may be text or SIP data (`[:span "Open"]`). An absent label falls back to the path, so a link always has an accessible name.
- Modified clicks (cmd/ctrl/shift/alt), middle clicks, `:target`, and `:download` are left to the browser.
- An off-site path renders an inert `<span>` instead of a link, and the failure is reported through the app's error hook. "Off-site" covers a scheme (`https:`, `javascript:`), a protocol-relative `//host`, and the backslash spellings of it (`/\host`, `\\host`, `\host`) -- a browser resolves all of those to the same cross-origin URL. The same rule admits paths to `router/push!`, `router/replace!` and `router/href`.

### `(router/href path)` -> string | nil

The `href` a link to `path` needs in the active mode -- `#/todos` in hash mode, `/todos` in history mode. Useful for anchors you build yourself.

## Example: Route-Based Rendering

```sema
(router/init!
  {:not-found "missing-page"
   :focus "#main"
   :routes {"/" "home-page"
            "/about" "about-page"
            "/users/:id" "user-page"}})

(defcomponent app-view ()
  (let ((r (router/current-route)))
    [:main {:id "main"}
     [:nav
      (router/link "/" "Home" {})
      (router/link "/about" "About" {})]
     (cond ((equal? (:handler r) "home-page") [:h1 "Home"])
           ((equal? (:handler r) "about-page") [:h1 "About"])
           ((equal? (:handler r) "user-page")
            [:h1 (string-append "User " (:id (:params r)))])
           (else [:h1 "404"]))]))

(mount! "#app" app-view)
```

## How It Works

When `router/init!` is called, it:

1. Compiles each pattern into a regex, escaping literal metacharacters and accepting each literal character in both its raw and percent-encoded form (the browser rewrites `#/søk` to `#/s%C3%B8k`, so a pattern that only matched what you wrote would never match what you get back).
2. Registers a route listener on `window` (`hashchange`, or `popstate` in history mode) and one delegated `click` listener on `document` for router links.
3. Immediately resolves the current URL into the route signal, without running the focus or scroll side effects -- those belong to navigation, not to page load.

Routes are matched in declaration order: the first matching pattern wins. Navigation resolves the new route synchronously, so code that navigates and then reads `router/current-route` sees the new route, not the previous one.
