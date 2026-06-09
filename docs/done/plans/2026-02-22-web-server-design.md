# Web Server Design

## Summary

Add an HTTP server to Sema with data-driven routing, middleware as function composition, SSE streaming, and WebSocket support. Built on axum, using a channel-bridged event loop that keeps Sema evaluation single-threaded while handling I/O concurrently.

## Architecture

### Event Loop Model

```
┌─────────────────────────────────────┐
│         Background Thread            │
│     (tokio multi-thread runtime)     │
│                                      │
│  axum server                         │
│    ├─ parse request into Sema map    │
│    ├─ send (req, oneshot_tx) to mpsc │
│    └─ await oneshot_rx for response  │
│                                      │
│  WS connections:                     │
│    ├─ upgrade via axum::extract::ws  │
│    └─ bridge with mpsc channels      │
└──────────────┬───────────────────────┘
               │ mpsc channel
┌──────────────▼───────────────────────┐
│         Main Thread                   │
│     (Sema evaluator loop)            │
│                                      │
│  loop {                              │
│    recv request from channel          │
│    match request type to route        │
│    call handler lambda via callback   │
│    send response Value via oneshot    │
│  }                                   │
└──────────────────────────────────────┘
```

- `http/serve` spawns a tokio runtime on a background thread running axum
- The main thread becomes the evaluator loop, reading requests from an mpsc channel
- Each HTTP request includes a `oneshot::Sender` for the response
- SSE: evaluator sends tokens through an mpsc channel that the tokio side streams out
- WS: bidirectional channels between tokio WS connection and evaluator

### Why This Model

Sema uses `Rc` (not `Arc`) everywhere — Values are not `Send`/`Sync`. The channel-bridged model keeps all Sema evaluation on the main thread while tokio handles concurrent network I/O. This is the same model as Node.js: single-threaded evaluation with async I/O.

For LLM-backed services where each request takes 1-5s of LLM latency, serialized evaluation is fine. A future thread-pool model (with deep-cloned environments) can be added later.

## Rust Types

```rust
enum ServerRequest {
    Http {
        request: Value,                   // {:method :get :path "/..." ...}
        respond: oneshot::Sender<Value>,  // response map back
    },
    Sse {
        request: Value,
        sender: mpsc::Sender<String>,     // stream tokens out
        done: oneshot::Sender<()>,        // signal completion
    },
    WebSocket {
        request: Value,
        incoming: mpsc::Receiver<String>, // messages from client
        outgoing: mpsc::Sender<String>,   // messages to client
    },
}
```

## Sema API

### Response Helpers

Pure map constructors, registered as `NativeFn::simple`:

```scheme
(http/ok body)              ;; {:status 200 :body (json/encode body) :headers {"content-type" "application/json"}}
(http/created body)         ;; {:status 201 ...}
(http/no-content)           ;; {:status 204 :body ""}
(http/not-found msg)        ;; {:status 404 ...}
(http/redirect url)         ;; {:status 302 :headers {"location" url}}
(http/error status body)    ;; {:status <status> ...}
(http/html content)         ;; {:status 200 :headers {"content-type" "text/html"} :body content}
(http/text content)         ;; {:status 200 :headers {"content-type" "text/plain"} :body content}
```

### Router

Data-driven routes. `http/router` takes a list of vectors and returns a handler function.

```scheme
(define routes
  [[:get  "/"          handle-home]
   [:get  "/users/:id" handle-user]
   [:post "/users"     handle-create]
   [:any  "/health"    handle-health]])

(define app (http/router routes))
```

Route matching: linear scan, first match wins. Path params (`:id`) extracted into `(:params req)`. `*` matches rest-of-path. Unmatched routes return 404.

### Server

```scheme
(http/serve handler {:port 3000})
(http/serve handler {:port 3000 :host "0.0.0.0"})
```

`http/serve` blocks the main thread — it IS the server's run loop. The handler is a function `(Request -> Response)`.

### Middleware

100% userland — no Rust code needed:

```scheme
(define (with-logging handler)
  (fn (req)
    (let [resp (handler req)]
      (println (:method req) (:path req) "->" (:status resp))
      resp)))

(define app
  (-> (http/router routes)
      with-cors
      with-logging))
```

### SSE Streaming

```scheme
(define (handle-stream req)
  (http/stream (fn (send)
    (send "first event")
    (send "second event"))))
```

`http/stream` returns a marker value. The server detects it and uses the SSE channel variant.

### WebSocket

```scheme
(define routes
  [[:ws "/chat" handle-ws]])

(define (handle-ws conn)
  (let [msg ((:recv conn))]
    ((:send conn) (string-append "echo: " msg))
    (handle-ws conn)))
```

`:ws` route type triggers WebSocket upgrade. Handler receives `{:send fn :recv fn :close fn}`.

### Request/Response Maps

```scheme
;; Request (what handler receives)
{:method  :get
 :path    "/users/42"
 :headers {"content-type" "application/json"}
 :query   {:page "2" :limit "10"}
 :params  {:id "42"}
 :body    "{\"name\": \"Ada\"}"
 :json    {:name "Ada"}
 :remote  "127.0.0.1"}

;; Response (what handler returns)
{:status  200
 :headers {"content-type" "application/json"}
 :body    "{\"message\": \"ok\"}"}
```

## Implementation Location

- **New file**: `crates/sema-stdlib/src/server.rs`
- **Registration**: in `lib.rs` under `#[cfg(not(target_arch = "wasm32"))]` gate
- **Dependencies**: `axum` with `ws` feature added to sema-stdlib

Keeps server code separate from the HTTP client in `http.rs`.

## Dependencies

```toml
# In crates/sema-stdlib/Cargo.toml
[target.'cfg(not(target_arch = "wasm32"))'.dependencies]
axum = { version = "0.8", features = ["ws"] }
```

`tokio` (already present), `tower`, and `hyper` are transitive deps.

## Error Handling

- **Handler errors**: caught, logged to stderr, returned as `{:status 500 :body "{\"error\": \"Internal server error\"}"}`
- **Startup errors**: `http/serve` returns `SemaError::Io` immediately if bind fails
- **Graceful shutdown**: Ctrl+C breaks the channel, evaluator loop exits, tokio runtime drops

## Testing

1. **Rust unit tests** in `server.rs`: response helpers, route matching, request/response conversion
2. **Integration tests** in `integration_test.rs`: spawn server as child process, hit with reqwest, assert responses
3. **Example file**: `examples/web-server.sema` for manual testing

## What We're NOT Building

- No ORM (maps + raw SQL via future `db/query` is enough)
- No template engine (`prompt/render` exists)
- No session management (JWTs + KV store)
- No form parsing (JSON APIs only)
- ~~No static file serving~~ — added via `http/file` and `:static` routes
- No `defroute` special form (data-driven routing is sufficient and more composable)
