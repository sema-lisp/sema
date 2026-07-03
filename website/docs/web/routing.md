# Router

The `router/*` namespace provides a hash-based SPA router built on signals. Routes are declared as a map of URL patterns to handler names, and the current route is exposed as a reactive signal.

## Setup

### `(router/init! route-map)` -> nil

Register routes and start listening for hash changes. The route map is a map from path patterns to handler names:

```scheme
(router/init! {"/" "home-page"
               "/todos" "todo-list"
               "/todos/:id" "todo-detail"
               "/settings" "settings-page"})
```

This installs a `hashchange` listener and immediately matches the current URL.

## Navigation

### `(router/push! path)` -> nil

Navigate to a path by setting `location.hash`. This adds a history entry so the back button works.

```scheme
(router/push! "/todos/42")
```

### `(router/replace! path)` -> nil

Navigate to a path without adding a history entry. Useful for redirects.

```scheme
(router/replace! "/login")
```

### `(router/back!)` -> nil

Go back one entry in the browser history.

```scheme
(router/back!)
```

## Reading the Current Route

### `(router/current)` -> signal-id

Returns the signal ID for the current route match. Use with `deref` to read the value.

### `(router/current-route)` -> map | nil

Convenience wrapper (defined in Sema) that dereferences the route signal. Returns a map with `:path`, `:params`, and `:handler`, or `nil` if no route matches.

```scheme
(router/current-route)
;; => {:path "/todos/42" :params {:id "42"} :handler "todo-detail"}
```

## Route Parameters

Route patterns support named parameters with the `:param` syntax. Parameters match any non-slash segment:

| Pattern | URL | Params |
|---------|-----|--------|
| `/todos/:id` | `/todos/42` | `{:id "42"}` |
| `/users/:uid/posts/:pid` | `/users/5/posts/99` | `{:uid "5" :pid "99"}` |
| `/` | `/` | `{}` |

Parameter names must match `[a-zA-Z_][a-zA-Z0-9_]*`.

## Example: Route-Based Rendering

```scheme
(router/init! {"/" "home-page"
               "/about" "about-page"
               "/users/:id" "user-page"})

(define (render-app)
  (def route (router/current-route))
  (if (nil? route)
    [:div [:h1 "404"] [:p "Page not found"]]
    (cond
      (= (:handler route) "home-page")
        [:div [:h1 "Home"] [:a {:href "#/about"} "About"]]
      (= (:handler route) "about-page")
        [:div [:h1 "About"] [:a {:href "#/"} "Home"]]
      (= (:handler route) "user-page")
        [:div [:h1 (string-append "User " (get (:params route) :id))]])))
```

## How It Works

The router uses hash-based URLs (`#/path`). When `router/init!` is called, it:

1. Compiles each pattern into a regex with named capture groups.
2. Registers a `hashchange` event listener on `window`.
3. Immediately evaluates the current hash to set the initial route signal.

Routes are matched in declaration order -- the first matching pattern wins. The route signal updates reactively, so any component dereferencing it will re-render when the route changes.
