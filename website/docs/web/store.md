# Storage

The `store/*` namespace provides access to the browser's `localStorage` and `sessionStorage` APIs. Values are automatically JSON-serialized on write and JSON-parsed on read, so Sema types (strings, numbers, lists, maps) are preserved across storage round-trips.

## localStorage

### `(store/get key)` -> value | nil

Retrieve a value from localStorage. Returns `nil` if the key does not exist or if parsing fails.

```sema
(store/get "username")        ;; => "alice"
(store/get "preferences")     ;; => {:theme "dark" :lang "en"}
(store/get "nonexistent")     ;; => nil
```

### `(store/set! key value)` -> nil

Store a value in localStorage. The value is JSON-serialized.

```sema
(store/set! "username" "alice")
(store/set! "count" 42)
(store/set! "todos" [{:text "Buy milk" :done false}])
```

### `(store/remove! key)` -> nil

Remove a key from localStorage.

```sema
(store/remove! "username")
```

### `(store/clear!)` -> nil

Remove all keys from localStorage.

```sema
(store/clear!)
```

### `(store/keys)` -> list of strings

List all keys currently in localStorage.

```sema
(store/keys)  ;; => ("username" "count" "todos")
```

### `(store/has? key)` -> boolean

Check whether a key exists in localStorage.

```sema
(if (store/has? "auth-token")
  (println "Logged in")
  (println "Not logged in"))
```

## sessionStorage

Session storage functions mirror their localStorage counterparts but data is scoped to the browser tab and cleared when the tab closes.

### `(store/session-get key)` -> value | nil

```sema
(store/session-get "draft")
```

### `(store/session-set! key value)` -> nil

```sema
(store/session-set! "draft" "Work in progress...")
```

### `(store/session-remove! key)` -> nil

```sema
(store/session-remove! "draft")
```

### `(store/session-clear!)` -> nil

```sema
(store/session-clear!)
```

## Type Preservation

Values are stored as JSON, so the following types round-trip correctly:

| Sema Type | JSON Encoding |
|-----------|--------------|
| String | `"hello"` |
| Number | `42`, `3.14` |
| Boolean | `true`, `false` |
| List | `[1, 2, 3]` |
| Map | `{"key": "value"}` |
| nil | `null` |

## Example: Persisting State

```sema
;; Save state on change
(define (save-todos! todos)
  (store/set! "todos" todos))

;; Restore state on load
(define (load-todos)
  (or (store/get "todos") []))

;; Usage
(def todos (load-todos))
;; ... modify todos ...
(save-todos! todos)
```

## Error Handling

Storage operations can fail (e.g., quota exceeded, storage disabled in private browsing). Every `store/*` function reports the error to the context's `onerror` handler and returns a fallback (`nil`, `#f` for `store/has?`, `()` for `store/keys`) rather than throwing.
