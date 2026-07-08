---
outline: [2, 3]
---

# HTTP & JSON

## HTTP

HTTP functions make synchronous requests and return a response map. All HTTP functions require the **network** sandbox capability.

::: tip Sandbox
HTTP functions are gated behind the `NETWORK` capability. They are available by default when running scripts with `sema`, but disabled in sandboxed environments (e.g., the WASM playground). A sandboxed script that attempts to use HTTP will receive an error.
:::

### Response Map

All HTTP functions return a map with three keys:

| Key        | Type   | Description                                          |
|------------|--------|------------------------------------------------------|
| `:status`  | int    | HTTP status code (e.g., `200`, `404`, `500`)         |
| `:headers` | map    | Response headers as keyword-keyed map                |
| `:body`    | string \| bytevector | Response body — a string by default, or a bytevector with `{:as :bytes}` |

```sema
(define resp (http/get "https://httpbin.org/get"))

(:status resp)    ; => 200
(:headers resp)   ; => {:content-type "application/json" :server "..." ...}
(:body resp)      ; => "{\"args\": {}, ...}"
```

Headers are returned with keyword keys derived from the header name (e.g., `Content-Type` becomes `:content-type`). The body is a raw string by default — use `json/decode` to parse JSON responses, or `{:as :bytes}` to get a bytevector for binary downloads (see [Binary bodies & downloads](#binary-bodies-downloads)).

### Options Map

The `http/get`, `http/post`, `http/put`, `http/delete`, and `http/request` functions accept an optional **options map** with the following keys:

| Key          | Type | Description                                                              |
|--------------|------|--------------------------------------------------------------------------|
| `:headers`   | map  | Request headers (string or keyword keys both work)                       |
| `:timeout`   | int  | Request timeout in milliseconds                                          |
| `:as`        | keyword | Response body decoding: `:text` (default) or `:bytes` (a bytevector)  |
| `:multipart` | list | Send a `multipart/form-data` body (file uploads) — see [Multipart & file uploads](#multipart-file-uploads) |

```sema
;; Custom headers and timeout
(http/get "https://api.example.com/data"
  {:headers {"Authorization" "Bearer tok_abc123"
             "Accept" "application/json"}
   :timeout 5000})
```

### Binary bodies & downloads

The request **body** may be a **bytevector** to send raw bytes (a binary
upload); it's sent verbatim with no JSON encoding. Set your own
`Content-Type` header if the server needs one.

Pass `{:as :bytes}` to receive the response `:body` as a **bytevector** instead
of a string — required for binary payloads (audio, images, PDFs) that would be
corrupted by UTF-8 text decoding. Pair with `file/write-bytes` to save a
download.

```sema
;; Download binary data and save it to disk
(let ((resp (http/get "https://api.example.com/audio.mp3" {:as :bytes})))
  (file/write-bytes "out.mp3" (:body resp)))

;; Upload raw bytes (e.g. an image read from disk)
(http/post "https://api.example.com/upload"
  (file/read-bytes "photo.jpg")
  {:headers {"Content-Type" "image/jpeg"}})
```

### Multipart & file uploads

Set `:multipart` in the options map to a **list of part maps** to send a
`multipart/form-data` body. Each part is `{:name "..." :content ...}` plus
optional `:filename` and `:content-type`. A `:filename` (or bytevector content)
marks the part as an uploaded file. When `:multipart` is present the positional
`body` is ignored.

| Part key        | Type                  | Description                                    |
|-----------------|-----------------------|------------------------------------------------|
| `:name`         | string (required)     | The form field name                            |
| `:content`      | string \| bytevector  | The field value or file bytes (required)       |
| `:filename`     | string (optional)     | Upload as a file with this name                |
| `:content-type` | string (optional)     | MIME type for the part                         |

```sema
;; Upload a file alongside a text field
(http/post "https://api.example.com/documents"
  {}    ; positional body ignored when :multipart is set
  {:headers {"Authorization" "Bearer tok_abc123"}
   :multipart (list
     {:name "purpose"  :content "rag-ingest"}
     {:name "file"     :filename "report.pdf"
      :content (file/read-bytes "report.pdf")
      :content-type "application/pdf"})})
```

### `http/get`

```
(http/get url)
(http/get url opts)
```

Make an HTTP GET request.

- **url** — string, the request URL
- **opts** — optional [options map](#options-map): `:headers`, `:timeout`, `:as` (`:text`/`:bytes`), `:multipart`

```sema
;; Simple GET
(http/get "https://httpbin.org/get")

;; GET with custom headers
(http/get "https://api.example.com/users"
  {:headers {:authorization "Bearer my-token"}})
```

### `http/post`

```
(http/post url body)
(http/post url body opts)
```

Make an HTTP POST request.

- **url** — string, the request URL
- **body** — request body: a map (auto-encoded as JSON with `Content-Type: application/json`), a string (sent as-is), or a bytevector (sent as raw bytes). Ignored when `:multipart` is set.
- **opts** — optional [options map](#options-map): `:headers`, `:timeout`, `:as` (`:text`/`:bytes`), `:multipart`

```sema
;; POST with a map body (auto-JSON-encoded)
(http/post "https://httpbin.org/post"
  {:name "Ada" :age 36})

;; POST with string body and custom headers
(http/post "https://api.example.com/webhook"
  "raw payload"
  {:headers {"Content-Type" "text/plain"}})

;; POST with JSON body and auth
(http/post "https://api.example.com/users"
  {:name "Ada" :role "admin"}
  {:headers {"Authorization" "Bearer tok_abc123"}
   :timeout 10000})
```

### `http/put`

```
(http/put url body)
(http/put url body opts)
```

Make an HTTP PUT request. Behaves identically to `http/post` — map bodies are auto-JSON-encoded.

- **url** — string, the request URL
- **body** — request body (string or map)
- **opts** — optional [options map](#options-map): `:headers`, `:timeout`, `:as` (`:text`/`:bytes`), `:multipart`

```sema
(http/put "https://api.example.com/users/42"
  {:name "Ada Lovelace" :role "admin"})
```

### `http/delete`

```
(http/delete url)
(http/delete url opts)
```

Make an HTTP DELETE request.

- **url** — string, the request URL
- **opts** — optional [options map](#options-map): `:headers`, `:timeout`, `:as` (`:text`/`:bytes`), `:multipart`

```sema
(http/delete "https://api.example.com/users/42"
  {:headers {"Authorization" "Bearer tok_abc123"}})
```

### `http/request`

```
(http/request method url)
(http/request method url opts)
(http/request method url opts body)
```

Make an HTTP request with any method. Use this for methods not covered by the convenience functions (e.g., `PATCH`, `HEAD`).

- **method** — string, HTTP method (case-insensitive, converted to uppercase). Supported: `GET`, `POST`, `PUT`, `DELETE`, `PATCH`, `HEAD`
- **url** — string, the request URL
- **opts** — optional [options map](#options-map): `:headers`, `:timeout`, `:as` (`:text`/`:bytes`), `:multipart`
- **body** — optional request body (string or map)

```sema
;; PATCH request
(http/request "PATCH" "https://api.example.com/users/42"
  {:headers {"Content-Type" "application/json"}}
  {:name "Updated Name"})

;; HEAD request (body will be empty)
(define resp (http/request "HEAD" "https://example.com"))
(:status resp)    ; => 200
(:body resp)      ; => ""
```

### Error Handling

Network errors (DNS failure, connection refused, timeout) throw a `SemaError::Io` error. Use `try`/`catch` to handle them:

```sema
;; Handle network errors
(try
  (http/get "https://unreachable.invalid")
  (catch e
    (println "Request failed:" e)))

;; Check status codes
(define resp (http/get "https://api.example.com/data"))
(cond
  ((= (:status resp) 200) (json/decode (:body resp)))
  ((= (:status resp) 404) (error "Not found"))
  ((>= (:status resp) 500) (error "Server error"))
  (else (error (format "Unexpected status: ~a" (:status resp)))))

;; Timeout handling
(try
  (http/get "https://slow-api.example.com/data"
    {:timeout 3000})
  (catch e
    (println "Request timed out or failed:" e)))
```

### Common Patterns

#### GET + JSON Decode Pipeline

```sema
;; Fetch JSON data and extract fields
(define data
  (-> (http/get "https://api.example.com/users/1")
      (:body)
      (json/decode)))

(:name data)   ; => "Ada"
(:email data)  ; => "ada@example.com"
```

#### POST with JSON Body and Auth Headers

```sema
(define resp
  (http/post "https://api.example.com/posts"
    {:title "Hello World" :body "Content here"}
    {:headers {"Authorization" "Bearer tok_abc123"
               "X-Request-Id" "req-001"}}))

(when (= (:status resp) 201)
  (println "Created:" (:body resp)))
```

#### Paginated API Requests

```sema
(define (fetch-all-pages base-url)
  (let loop ((page 1) (results '()))
    (define resp (http/get (format "~a?page=~a" base-url page)))
    (define data (json/decode (:body resp)))
    (define items (:items data))
    (if (empty? items)
      results
      (loop (+ page 1) (append results items)))))
```

---

## JSON

Functions for encoding Sema values to JSON strings and decoding JSON strings back into Sema values.

### Type Mapping

#### Encoding (Sema → JSON)

| Sema Type   | JSON Type | Notes                                      |
|-------------|-----------|--------------------------------------------|
| `int`       | number    | `42` → `42`                                |
| `float`     | number    | `3.14` → `3.14`. NaN/Infinity cause errors |
| `string`    | string    | `"hello"` → `"hello"`                      |
| `keyword`   | string    | `:name` → `"name"`                         |
| `symbol`    | string    | `'foo` → `"foo"`                           |
| `#t` / `#f` | boolean   | `#t` → `true`, `#f` → `false`             |
| `nil`       | null      | `nil` → `null`                             |
| list        | array     | `'(1 2 3)` → `[1, 2, 3]`                  |
| vector      | array     | `[1 2 3]` → `[1, 2, 3]`                   |
| map         | object    | `{:a 1}` → `{"a": 1}`                     |
| hashmap     | object    | Same as map                                |
| function    | *error*   | Cannot encode functions as JSON            |
| record      | *error*   | Cannot encode records as JSON              |

#### Decoding (JSON → Sema)

| JSON Type | Sema Type | Notes                                           |
|-----------|-----------|-------------------------------------------------|
| number    | int/float | Integers decode as `int`, decimals as `float`   |
| string    | string    | `"hello"` → `"hello"`                           |
| boolean   | bool      | `true` → `#t`, `false` → `#f`                  |
| null      | nil       | `null` → `nil`                                  |
| array     | list      | `[1, 2]` → `(1 2)`                             |
| object    | map       | Keys become keywords: `{"a": 1}` → `{:a 1}`   |

### `json/encode`

```
(json/encode value) → string
```

Encode a Sema value as a compact JSON string. Uses **strict** conversion — errors on values that cannot be represented in JSON (functions, records, NaN, Infinity).

- **value** — any JSON-encodable Sema value

```sema
(json/encode 42)                    ; => "42"
(json/encode "hello")               ; => "\"hello\""
(json/encode #t)                    ; => "true"
(json/encode nil)                   ; => "null"
(json/encode '(1 2 3))             ; => "[1,2,3]"
(json/encode [1 2 3])              ; => "[1,2,3]"
(json/encode {:name "Ada" :age 36}) ; => "{\"age\":36,\"name\":\"Ada\"}"
```

Encoding errors:

```sema
;; NaN and Infinity cannot be represented in JSON
(json/encode (/ 0.0 0.0))   ; Error: cannot encode NaN/Infinity as JSON

;; Functions cannot be encoded
(json/encode println)        ; Error: cannot encode native-fn as JSON
```

### `json/encode-pretty`

```
(json/encode-pretty value) → string
```

Encode a Sema value as a pretty-printed JSON string with 2-space indentation. Same strict conversion rules as `json/encode`.

- **value** — any JSON-encodable Sema value

```sema
(json/encode-pretty {:name "Ada" :scores [95 87 92]})
;; =>
;; {
;;   "name": "Ada",
;;   "scores": [
;;     95,
;;     87,
;;     92
;;   ]
;; }
```

### `json/decode`

```
(json/decode json-string) → value
```

Decode a JSON string into a Sema value. JSON objects become maps with keyword keys, arrays become lists. See the [type mapping table](#decoding-json-sema) for full details.

- **json-string** — a string containing valid JSON

```sema
(json/decode "42")                          ; => 42
(json/decode "3.14")                        ; => 3.14
(json/decode "\"hello\"")                   ; => "hello"
(json/decode "true")                        ; => #t
(json/decode "null")                        ; => nil
(json/decode "[1, 2, 3]")                   ; => (1 2 3)
(json/decode "{\"name\": \"Ada\"}")         ; => {:name "Ada"}
```

Decoding errors:

```sema
;; Invalid JSON throws an error
(json/decode "not json")    ; Error: json/decode: expected value at line 1 column 1

;; Argument must be a string
(json/decode 42)            ; Error: type error: expected string, got int
```

### JSON Roundtrips

Values that survive an encode → decode roundtrip preserve their structure, though some types are normalized:

```sema
;; Vectors become lists after roundtrip
(json/decode (json/encode [1 2 3]))   ; => (1 2 3)

;; Keywords in maps are preserved
(json/decode (json/encode {:a 1 :b 2}))   ; => {:a 1 :b 2}

;; Nested structures work
(define data {:users [{:name "Ada"} {:name "Bob"}]
              :count 2
              :active #t})
(define roundtripped (json/decode (json/encode data)))
(:count roundtripped)   ; => 2
(:active roundtripped)  ; => #t
```

### Error Handling

JSON encoding and decoding errors can be caught with `try`/`catch`:

```sema
;; Catch encoding errors
(try
  (json/encode (/ 0.0 0.0))
  (catch e
    (println "Encode failed:" e)))

;; Catch decoding errors
(try
  (json/decode "invalid json {{{")
  (catch e
    (println "Decode failed:" e)))
```
