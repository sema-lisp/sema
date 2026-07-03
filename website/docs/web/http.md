# HTTP & Streams

Sema Web uses the normal `http/*` request functions from the stdlib for fetch-style requests, and adds a browser-specific streaming API for Server-Sent Events.

## Regular HTTP

Normal `http/get`, `http/post`, `http/request`, and related functions are documented in the stdlib HTTP docs. In Sema Web they run through the browser fetch API.

Use `evalAsync()` from JavaScript when your top-level Sema code performs HTTP requests directly.

## Event Streams

### `(http/event-source url-or-opts)` -> signal

Open a streaming HTTP connection and return a reactive signal. Unlike the browser's native `EventSource`, Sema Web uses a fetch-based SSE client so you can send headers, credentials, and POST bodies.

The first argument may be either:

- a URL string
- an options map with `:url`, `:method`, `:headers`, `:body`, and `:with-credentials`

```scheme
(def stream
  (http/event-source "https://example.com/events"))

(def auth-stream
  (http/event-source
    {:url "/api/events"
     :method "POST"
     :headers {"authorization" "Bearer demo-token"}
     :body "{\"topic\":\"updates\"}"
     :with-credentials true}))
```

### Stream State

Dereferencing the returned signal gives a map with this shape:

```scheme
{:data "raw event payload"
 :event "message"
 :id nil
 :retry nil
 :done false
 :error nil
 :status 200
 :state "open"}
```

Fields:

- `:data` — raw event payload string
- `:event` — event name, or `"message"` when omitted
- `:id` — SSE event id if present
- `:retry` — SSE retry value if present
- `:done` — `true` once the stream is closed or errors
- `:error` — error message string, or `nil`
- `:status` — HTTP status once the connection opens
- `:state` — `"connecting"`, `"open"`, or `"closed"`

## Closing Streams

### `(http/close-event-source stream)` -> nil

Close a stream created by `http/event-source`.

```scheme
(http/close-event-source stream)
```

### `(http/close-stream stream)` -> nil

Alias for `http/close-event-source`.

```scheme
(http/close-stream stream)
```

## Components and Cleanup

Streams created during a component render or lifecycle hook are owned by that component and are closed automatically on unmount. You can still close them manually if you want earlier shutdown behavior.

```scheme
(defcomponent ticker ()
  (let ((stream (local "stream" nil)))
    (on-mount (fn ()
      (put! stream (http/event-source "/api/ticker"))
      (fn ()
        (when @stream
          (http/close-stream @stream)))))
    [:pre (:data (deref @stream))]))
```

## Relationship to `llm/chat-stream`

`llm/chat-stream` is built on the same streaming machinery, but consumes the normalized LLM proxy SSE protocol and returns a simpler signal shape:

```scheme
{:text "partial response" :done false :error nil}
```

Use `http/event-source` for arbitrary SSE endpoints. Use `llm/chat-stream` for proxy-backed LLM responses.
