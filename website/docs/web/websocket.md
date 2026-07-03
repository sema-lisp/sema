# WebSockets

Sema Web ships the same `ws/*` client you use on the server, backed by the
browser's native `WebSocket`. Connect to a `ws://`/`wss://` endpoint, send text,
binary, or JSON frames, and receive messages through evented handlers that plug
straight into the reactive component system.

```sema
(def sock (ws/connect "wss://echo.websocket.org"))

(ws/listen sock
  {:on-open    (fn (conn)     (ws/send conn "hello"))
   :on-message (fn (conn msg) (put! last-message msg))
   :on-close   (fn (conn info) (println "closed" (:code info)))
   :on-error   (fn (conn err) (println "error" err))})
```

## Native vs browser

The client API is the same across native and browser, with two differences that
follow from the browser sandbox:

- **Receiving is evented.** The browser main thread cannot block, so the
  pull-based `ws/recv` and `ws/recv-timeout` are **native-only**. In the browser
  you receive through `ws/listen` (`:on-message`, `:on-open`, `:on-close`,
  `:on-error`) — the same evented model as browser SSE and `llm/chat-stream`.
- **Connect options are limited.** The browser `WebSocket` constructor only
  supports subprotocols, so `ws/connect` honors `:subprotocols` but ignores the
  native-only `:headers`, `:timeout`, `:retries`, and `:retry-backoff-ms`. For
  authenticated connections, pass a token via a subprotocol or a query
  parameter, or terminate auth at a proxy.

Everything else — `ws/connect`, `ws/send`, `ws/close`, `ws/connected?`, and
`ws/listen` — behaves identically, so code written against `ws/listen` runs on
both targets unchanged.

## Functions

### `(ws/connect url [opts])` → connection

Open a connection and return an opaque handle. `opts` is a map; in the browser
only `:subprotocols` (a string or list of strings) is used.

```sema
(def sock (ws/connect "wss://example.com/socket"))
(def sub  (ws/connect "wss://example.com/socket" {:subprotocols ["v1.chat"]}))
```

### `(ws/send conn msg)`

Send a frame. `msg` may be:

| Form                | Frame                              |
| ------------------- | ---------------------------------- |
| a string            | text                               |
| a bytevector        | binary                             |
| a map               | JSON text (the map, encoded)       |
| `{:text s}`         | text                               |
| `{:binary bv}`      | binary                             |
| `{:json v}`         | JSON text (the inner value)        |

```sema
(ws/send sock "ping")
(ws/send sock {:type "chat" :body "hi"})     ;; JSON
(ws/send sock {:json {:type "chat"}})        ;; JSON, explicit
(ws/send sock {:binary (bytevector 1 2 3)})  ;; binary
```

### `(ws/listen conn handlers)`

Attach evented handlers. All are optional:

| Handler        | Called with   | When                                         |
| -------------- | ------------- | -------------------------------------------- |
| `:on-open`     | `(conn)`      | when the socket opens                        |
| `:on-message`  | `(conn msg)`  | each text (string) or binary (bytevector) frame |
| `:on-close`    | `(conn info)` | the socket closed (`info` is `{:code :reason}`) |
| `:on-error`    | `(conn err)`  | a connection error                           |

In the browser `ws/listen` returns immediately (it is evented — nothing to
await).

## Binary frames

Binary works the same in the browser as on native. Send a bytevector (or
`{:binary bv}`) and it goes out as a binary frame; an incoming binary frame is
delivered to `:on-message` as a **bytevector** (not a string), byte-for-byte:

```sema
(ws/listen sock
  {:on-open    (fn (c) (ws/send c (bytevector 1 2 3 255)))  ;; binary frame
   :on-message (fn (c m)
     (if (bytevector? m)
       (handle-bytes m)      ;; binary frame → bytevector
       (handle-text m)))})   ;; text frame  → string
```

### `(ws/connected? conn)` → bool

True only while the socket is open.

### `(ws/close conn [code] [reason])`

Close the socket and release its handle. Open sockets are also closed
automatically when the Sema Web instance is disposed.

## Example: live message log

```sema
(def messages (state []))
(def sock (ws/connect "wss://echo.websocket.org"))

(ws/listen sock
  {:on-open    (fn (c)   (ws/send c "hello from sema"))
   :on-message (fn (c m) (update! messages (fn (xs) (append xs (list m)))))})

(defcomponent message-log ()
  [:ul (map (fn (m) [:li m]) (deref messages))])
```

Because `messages` is reactive state, every frame appended in `:on-message`
re-renders the component automatically.
