---
outline: [2, 3]
---

# Web Server

Sema includes a built-in HTTP server powered by [axum](https://github.com/tokio-rs/axum), with data-driven routing, middleware as function composition, SSE streaming, and WebSocket support. The server runs on a background thread with a Tokio runtime while keeping all Sema evaluation single-threaded — the same model as Node.js.

## Quick Start

```sema
(define (handler req)
  (http/ok {:message "Hello from Sema!"}))

(http/serve handler {:port 3000})
```

```bash
$ curl http://localhost:3000
{"message":"Hello from Sema!"}
```

## Serving

### `http/serve`

Start an HTTP server. Takes a handler function and an optional options map. The handler receives a request map and returns a response map. This function blocks — it becomes the server's run loop.

```sema
(http/serve handler)
(http/serve handler {:port 3000})
(http/serve handler {:port 8080 :host "127.0.0.1"})
```

| Option           | Default     | Description                                                      |
| ---------------- | ----------- | ---------------------------------------------------------------- |
| `:port`          | `3000`      | TCP port to bind                                                 |
| `:host`          | `"0.0.0.0"` | Address to bind to                                               |
| `:port-fallback` | `false`     | If the port is taken, bind the next free port instead of failing |
| `:on-listen`     | —           | Function called once bound with `{:host :port :url}`             |

The handler is any function `(request-map -> response-map)`. This can be a plain function, a router, or a middleware-wrapped stack.

#### Automatic port fallback

By default `http/serve` fails fast when the port is in use. Pass `:port-fallback true`
to walk to the next free port instead. Since the bound port may then differ from the
one requested, use `:on-listen` to learn where the server ended up:

```sema
(http/serve handler
  {:port 3000
   :port-fallback true
   :on-listen (fn (info) (println (string-append "Ready at " (:url info))))})
```

`:on-listen` runs once, on the main thread, right after the socket binds.

## Routing

### `http/router`

Create a handler function from a list of route definitions. Each route is a vector of `[method pattern handler]`.

```sema
(define routes
  [[:get  "/"            handle-home]
   [:get  "/users/:id"   handle-user]
   [:post "/users"       handle-create]
   [:any  "/echo"        handle-echo]])

(define app (http/router routes))
(http/serve app {:port 3000})
```

Supported methods: `:get`, `:post`, `:put`, `:patch`, `:delete`, `:any` (matches all methods), `:ws` (WebSocket upgrade), and `:static` (static file directory).

Routes are matched top-to-bottom — first match wins. Unmatched routes return 404.

### Path Parameters

Use `:param` syntax to capture path segments. Extracted values appear in the request's `:params` map.

```sema
;; Route: [:get "/users/:id" handle-user]
;; Request: GET /users/42

(define (handle-user req)
  (let ((id (:id (:params req))))
    (http/ok {:user-id id})))
; => {"user-id":"42"}
```

Multiple parameters work as expected:

```sema
[:get "/users/:uid/posts/:pid" handler]
;; GET /users/1/posts/99 → {:uid "1" :pid "99"}
```

### Wildcard Routes

Use `*` to capture the rest of the path.

```sema
[:get "/files/*" handle-files]
;; GET /files/docs/readme.md → {:* "docs/readme.md"}
```

## Request Map

Every handler receives a request map with the following fields:

```sema
{:method  :get                                    ; HTTP method as keyword
 :path    "/users/42"                             ; Request path
 :headers {"content-type" "application/json" ...} ; Headers (string keys)
 :query   {:search "term" :page "1"}              ; Query params (keyword keys)
 :params  {:id "42"}                              ; Route params (keyword keys)
 :body    "{\"name\": \"Ada\"}"                   ; Raw body string
 :json    {:name "Ada"}}                          ; Parsed JSON body (if applicable)
```

The `:json` field is automatically populated when the request has `Content-Type: application/json`.

> **Request body limit.** Request bodies are capped at **16 MiB**. A larger body is rejected with `413 Payload Too Large` instead of being buffered into memory, so a client can't exhaust the server's memory with an oversized upload.

### Accessing Request Data

```sema
;; Method
(:method req)         ; => :get

;; Path
(:path req)           ; => "/users/42"

;; A specific header
(get (:headers req) "authorization")  ; => "Bearer ..."

;; Query parameter
(:page (:query req))  ; => "2"

;; Route parameter
(:id (:params req))   ; => "42"

;; JSON body field
(:name (:json req))   ; => "Ada"
```

## Response Map

Handlers return a response map with `:status`, `:headers`, and `:body`:

```sema
{:status  200
 :headers {"content-type" "application/json"}
 :body    "{\"message\": \"ok\"}"}
```

You can construct these by hand, but the response helpers below are more convenient.

## Response Helpers

### `http/ok`

Return 200 with a JSON-encoded body.

```sema
(pprint (http/ok {:message "success"}))
; => {:body "{"message":"success"}"
;     :headers {"content-type" "application/json"}
;     :status 200}

(pprint (http/ok [1 2 3]))
; => {:body "[1,2,3]" :headers {"content-type" "application/json"} :status 200}
```

### `http/created`

Return 201 with a JSON-encoded body.

```sema
(http/created {:id 42 :name "Ada"})
```

### `http/no-content`

Return 204 with an empty body.

```sema
(http/no-content)
```

### `http/not-found`

Return 404 with a JSON-encoded body.

```sema
(http/not-found {:error "User not found"})
```

### `http/error`

Return a custom status code with a JSON-encoded body.

```sema
(http/error 422 {:errors ["Invalid email" "Name required"]})
(http/error 503 {:error "Service unavailable"})
```

### `http/redirect`

Return a 302 redirect to a URL.

```sema
(http/redirect "https://example.com/login")
```

### `http/html`

Return 200 with `Content-Type: text/html`.

```sema
(http/html "<h1>Hello</h1><p>Welcome to Sema.</p>")
```

### `http/text`

Return 200 with `Content-Type: text/plain`.

```sema
(http/text "OK")
```

### `http/file`

Return a file from disk with automatic MIME type detection. The file is read on the I/O thread (not the evaluator), so it handles binary files efficiently.

```sema
(http/file "public/index.html")
(http/file "data/report.pdf" "application/pdf")  ; explicit content type
```

The path is resolved relative to the current working directory. If the file doesn't exist, an error is raised. The MIME type is guessed from the file extension (e.g. `.html` → `text/html`, `.css` → `text/css`, `.js` → `application/javascript`).

## Static File Serving

### `:static` Routes

Serve an entire directory of static files using the `:static` route type in `http/router`. Files are served with automatic MIME types, cache headers, and path traversal protection.

```sema
(define routes
  [[:static "/assets" "./public"]
   [:get    "/*"      handle-spa]])

(http/serve (http/router routes) {:port 3000})
```

```bash
$ curl http://localhost:3000/assets/style.css
body { color: red; }

$ curl -I http://localhost:3000/assets/style.css
Content-Type: text/css
Cache-Control: public, max-age=3600
```

The `:static` route takes a URL prefix and a directory path. Requests matching the prefix are mapped to files in the directory:

- `GET /assets/style.css` → reads `./public/style.css`
- `GET /assets/js/app.js` → reads `./public/js/app.js`
- `GET /assets/` → reads `./public/index.html` (directory index)

**Fallthrough**: If a file doesn't exist, the route does *not* match — the router continues to the next route. This enables SPA (single-page application) patterns where a catch-all route serves `index.html` for client-side routing:

```sema
(define routes
  [[:static "/assets" "./dist/assets"]
   [:get    "/*"      (fn (_) (http/file "./dist/index.html"))]])

(http/serve (http/router routes) {:port 3000})
```

**Security**: Path traversal attempts (e.g. `../etc/passwd`) are rejected with a 400 response. Only GET and HEAD methods are accepted.

## Middleware

Middleware in Sema is just function composition — a function that takes a handler and returns a new handler. No special framework needed.

### Writing Middleware

```sema
;; Logging middleware
(define (with-logging handler)
  (fn (req)
    (let ((resp (handler req)))
      (println (:method req) (:path req) "->" (:status resp))
      resp)))
```

```sema
;; CORS middleware
(define (with-cors handler)
  (fn (req)
    (let ((resp (handler req)))
      (assoc resp :headers
        (merge (or (:headers resp) {})
          {"access-control-allow-origin" "*"
           "access-control-allow-methods" "GET, POST, PUT, DELETE"})))))
```

```sema
;; Auth middleware
(define (with-auth handler)
  (fn (req)
    (let ((token (get (:headers req) "authorization")))
      (if token
        (handler req)
        (http/error 401 {:error "Unauthorized"})))))
```

### Composing Middleware

Stack middleware by nesting function calls. The outermost middleware runs first.

```sema
(define app
  (with-logging
    (with-cors
      (with-auth
        (http/router routes)))))

(http/serve app {:port 3000})
```

Or use the threading macro for a cleaner pipeline:

```sema
(define app
  (-> (http/router routes)
      with-auth
      with-cors
      with-logging))
```

## SSE Streaming

### `http/stream`

Return a Server-Sent Events stream. Takes a handler function that receives a `send` callback.

```sema
(define (handle-events req)
  (http/stream
    (fn (send)
      (send "connected")
      (sleep 1000)
      (send "update 1")
      (sleep 1000)
      (send "update 2"))))
```

The stream stays open as long as the handler is running. When the handler returns, the stream closes.

```sema
;; Route it like any other handler
(define routes
  [[:get "/events" handle-events]])
```

```bash
$ curl -N http://localhost:3000/events
data: connected

data: update 1

data: update 2
```

### Streaming LLM Responses

SSE is particularly useful for streaming LLM completions to the browser:

```sema
(define (handle-chat req)
  (http/stream
    (fn (send)
      (let ((prompt (:prompt (:json req))))
        ;; Stream each token as an SSE event
        (llm/stream prompt (fn (token) (send token)))))))
```

## WebSocket

### `http/websocket`

Handle bidirectional WebSocket connections. Takes a handler function that receives a connection map with `:send`, `:recv`, and `:close` functions.

```sema
(define (handle-ws conn)
  (let ((msg ((:recv conn))))
    (when msg
      ((:send conn) (string/append "echo: " msg))
      (handle-ws conn))))
```

The connection map:

| Key      | Description                                              |
| -------- | -------------------------------------------------------- |
| `:send`  | `(send message)` — send a string (text frame) or a bytevector (binary frame) |
| `:recv`  | `(recv)` — block until a message arrives; a text frame returns a string, a binary frame a bytevector, `nil` on close |
| `:close` | `(close)` — Close the connection                         |

### WebSocket Routes

Use the `:ws` method in the router:

```sema
(define routes
  [[:get "/api/status" handle-status]
   [:ws  "/ws/chat"    handle-ws]])

(http/serve (http/router routes) {:port 3000})
```

### Chat Room Example

```sema
(define clients (atom '()))

(define (broadcast msg)
  (for-each (fn (send) (send msg))
            @clients))

(define (handle-ws conn)
  ;; Add this client's send function to the list
  (swap! clients (fn (lst) (cons (:send conn) lst)))
  ;; Read loop
  (let loop ((msg ((:recv conn))))
    (when msg
      (broadcast msg)
      (loop ((:recv conn))))))

(define routes
  [[:ws "/chat" handle-ws]])

(http/serve (http/router routes) {:port 3000})
```

## WebSocket Client

Connect to a WebSocket server with `ws/connect`. A connection is a closeable
stream, so `with-open` closes it automatically — on both the normal and the
error path.

```sema
(with-open (sock (ws/connect "wss://echo.websocket.events"))
  (ws/send sock "hello")
  (match (ws/recv sock)
    {:text msg}   (println msg)
    {:binary buf} (handle-bytes buf)
    {:close info} :done))
```

### `ws/connect`

`(ws/connect url)` / `(ws/connect url opts)` — open a connection to a `ws://` or
`wss://` URL, returning a connection value. Blocks until the handshake completes
(or fails). Requires the `network` capability. Inside an `async/spawn` task it
yields cooperatively, so sibling tasks run while the handshake and later receives
are in flight.

`opts` is an optional map:

| Key                  | Meaning                                                       |
| -------------------- | ------------------------------------------------------------- |
| `:headers`           | map of extra HTTP headers on the upgrade (e.g. auth tokens)   |
| `:subprotocols`      | list of `Sec-WebSocket-Protocol` values to offer              |
| `:timeout`           | handshake timeout in milliseconds                             |
| `:retries`           | retry a failed handshake this many times (default `0`)        |
| `:retry-backoff-ms`  | base backoff, doubled each retry and capped at 30s (default `500`) |

```sema
(ws/connect "wss://api.example.com/socket"
  {:headers {"Authorization" "Bearer …"}
   :subprotocols ["chat"]
   :timeout 5000
   :retries 3})
```

### `ws/send`

`(ws/send conn msg)` — send a message. The frame type follows `msg`:

| `msg`                | Frame sent                                              |
| -------------------- | ------------------------------------------------------- |
| string               | text frame                                              |
| bytevector           | binary frame                                            |
| `{:text s}`          | text frame (explicit)                                   |
| `{:binary bv}`       | binary frame (explicit)                                 |
| `{:json v}`          | text frame: `v` encoded as JSON                         |
| any other map        | text frame: the map encoded as JSON                     |

### `ws/recv` and `ws/recv-timeout`

`(ws/recv conn)` — receive the next message, blocking until one arrives. Returns
a single-key tagged map so a `match` can dispatch on the frame type:

| Return value               | Meaning                                          |
| -------------------------- | ------------------------------------------------ |
| `{:text "…"}`              | a text frame                                     |
| `{:binary #u8(…)}`         | a binary frame                                   |
| `{:close {:code :reason}}` | the server closed the connection                 |
| `nil`                      | the connection is fully drained and closed       |

`(ws/recv-timeout conn ms)` is the same but returns the keyword `:timeout` if no
message arrives within `ms` milliseconds (distinct from `nil`, which means
closed). A protocol error surfaces as a thrown error you can `try`/`catch`.

### `ws/ping`

`(ws/ping conn)` / `(ws/ping conn payload)` — send a ping frame (optional string
or bytevector payload); the server replies with a matching pong. Incoming pings
are answered automatically.

### `ws/close` and `ws/connected?`

`(ws/close conn)` closes the connection (idempotent; also done for you by
`with-open`). `(ws/connected? conn)` reports whether the socket is still live.

### `ws/listen`

`(ws/listen conn handlers)` drives a receive loop, dispatching each frame to the
matching handler. It spawns an async task and returns its promise — `async/await`
it (or run the scheduler) to drive the loop. All handlers are optional:

| Handler        | Called with         | When                                   |
| -------------- | ------------------- | -------------------------------------- |
| `:on-open`     | `(conn)`            | once, before the loop                  |
| `:on-message`  | `(conn msg)`        | each text (string) or binary (bytevector) frame |
| `:on-close`    | `(conn info)`       | the connection closed (`info` is `{:code :reason}`) |
| `:on-error`    | `(conn err)`        | a recv/protocol error (loop then stops) |

```sema
(with-open (sock (ws/connect "wss://stream.example.com"))
  (async/await
    (ws/listen sock
      {:on-message (fn (conn msg) (println msg))
       :on-close   (fn (conn info) (println "closed"))})))
```

> **Browser support.** The `ws/*` client also runs in the browser (Sema Web /
> WASM), backed by the browser's native `WebSocket`: `ws/connect`, `ws/send`
> (text/binary/JSON + `{:text}`/`{:binary}`/`{:json}` framing), `ws/close`,
> `ws/connected?`, and `ws/listen` all work there. Because the browser main
> thread cannot block, the pull-based `ws/recv` and `ws/recv-timeout` are
> **native-only** — in the browser, receive with the evented `ws/listen`
> (`:on-message` / `:on-open` / `:on-close` / `:on-error`), which mirrors how
> browser SSE and `llm/chat-stream` deliver data. Connection `:headers`,
> `:timeout`, and retry options are native-only too (the browser `WebSocket` API
> only supports `:subprotocols`). See the
> [Sema Web WebSocket guide](https://sema-lang.com/docs/web/websocket).

## Complete Examples

### REST API

A JSON API with CRUD operations, middleware, and error handling.

```sema
;; In-memory data store
(define db (atom {}))
(define next-id (atom 0))

(define (gen-id)
  (swap! next-id (fn (n) (+ n 1)))
  @next-id)

;; Handlers
(define (list-users _)
  (http/ok (vals @db)))

(define (get-user req)
  (let ((id (:id (:params req)))
        (user (get @db id)))
    (if user
      (http/ok user)
      (http/not-found {:error "User not found"}))))

(define (create-user req)
  (let ((data (:json req))
        (id   (str (gen-id)))
        (user (assoc data :id id)))
    (swap! db (fn (d) (assoc d id user)))
    (http/created user)))

(define (delete-user req)
  (let ((id (:id (:params req))))
    (swap! db (fn (d) (dissoc d id)))
    (http/no-content)))

;; Middleware
(define (with-json-errors handler)
  (fn (req)
    (let ((resp (handler req)))
      (if (map? resp) resp
        (http/error 500 {:error "Internal server error"})))))

(define (with-cors handler)
  (fn (req)
    (let ((resp (handler req)))
      (assoc resp :headers
        (merge (or (:headers resp) {})
          {"access-control-allow-origin" "*"
           "access-control-allow-methods" "GET, POST, DELETE"})))))

;; Routes
(define routes
  [[:get    "/users"     list-users]
   [:get    "/users/:id" get-user]
   [:post   "/users"     create-user]
   [:delete "/users/:id" delete-user]])

;; Start
(define app
  (-> (http/router routes)
      with-json-errors
      with-cors))

(http/serve app {:port 3000})
```

### LLM-Powered API

An API endpoint that uses Sema's built-in LLM primitives to generate responses.

```sema
(define (handle-summarize req)
  (let ((text (:text (:json req))))
    (if text
      (http/ok {:summary (llm/complete (str "Summarize this:\n\n" text))})
      (http/error 400 {:error "Missing 'text' field"}))))

(define (handle-extract req)
  (let ((text (:text (:json req))))
    ;; llm/extract takes the schema first, then the text.
    (http/ok (llm/extract {:name "string"
                           :date "string"
                           :amount "number"}
                          text))))

(define routes
  [[:post "/summarize" handle-summarize]
   [:post "/extract"   handle-extract]
   [:get  "/health"    (fn (_) (http/ok {:status "up"}))]])

(http/serve (http/router routes) {:port 3000})
```

### HTML Application

Serve dynamic HTML pages.

```sema
(define (page title body)
  (http/html
    (str "<!DOCTYPE html><html><head><title>" title "</title>"
         "<style>body{font-family:sans-serif;max-width:800px;margin:0 auto;padding:2rem}</style>"
         "</head><body>" body "</body></html>")))

(define (handle-home _)
  (page "Home" "<h1>Welcome</h1><p>Built with Sema.</p>"))

(define (handle-greet req)
  (let ((name (or (:name (:params req)) "world")))
    (page "Greeting" (str "<h1>Hello, " name "!</h1>"))))

(define routes
  [[:get "/"            handle-home]
   [:get "/greet/:name" handle-greet]])

(http/serve (http/router routes) {:port 3000})
```

### SPA with Static Assets

Serve a single-page application with static assets and a catch-all for client-side routing.

```sema
(define routes
  [[:get    "/api/health" (fn (_) (http/ok {:status "up"}))]
   [:static "/assets"     "./dist/assets"]
   [:get    "/*"          (fn (_) (http/file "./dist/index.html"))]])

(http/serve (http/router routes) {:port 3000})
```

CSS, JS, and images under `./dist/assets/` are served with correct MIME types and cache headers. All other GET requests serve `index.html` for client-side routing.

## Architecture Notes

- **Single-threaded evaluation**: All Sema code runs on the main thread. HTTP I/O runs on a background Tokio runtime. Requests are bridged via channels.
- **Concurrency model**: Requests are processed sequentially by the evaluator. For LLM-backed services (where each request takes 1–5s of LLM latency), this is fine. For high-throughput APIs, consider a reverse proxy.
- **Graceful shutdown**: Ctrl+C breaks the channel and the server exits cleanly.
- **Sandbox-aware**: `http/serve` requires the `NETWORK` capability when running in sandbox mode.

## See Also

- [HTTP Client & JSON](./http-json) — outbound HTTP requests and JSON encoding/decoding
- [LLM Primitives](/docs/llm/) — building LLM-powered endpoints
- [Key-Value Store](./kv-store) — persistent storage for server state
