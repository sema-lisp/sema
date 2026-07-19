---
outline: [2, 3]
---

# Streams

Streams are first-class byte-oriented I/O handles for reading and writing data incrementally. They provide a unified interface across files, in-memory buffers, strings, and standard I/O — the same `stream/read` and `stream/write` work regardless of the underlying source.

```sema
;; Read a file line by line
(with-stream (s (stream/open-input "data.txt"))
  (let loop ((line (stream/read-line s)))
    (when line
      (println line)
      (loop (stream/read-line s)))))

;; In-memory buffer
(let ((buf (stream/byte-buffer)))
  (stream/write-string buf "hello")
  (stream/to-string buf))  ;; => "hello"
```

## Creating Streams

### `stream/from-string`

Create a read-only stream from a string's UTF-8 bytes.

```sema
(define s (stream/from-string "hello world"))
(stream/read-byte s)    ;; => 104 (ASCII 'h')
(stream/read s 5)       ;; => #u8(101 108 108 111 32) ("ello ")
```

### `stream/from-bytes`

Create a readable stream from a bytevector.

```sema
(define s (stream/from-bytes (bytevector 1 2 3)))
(stream/read-byte s)    ;; => 1
(stream/read-byte s)    ;; => 2
```

### `stream/byte-buffer`

Create a read/write in-memory buffer. Writes append to the buffer; reads consume from the current position.

```sema
(define buf (stream/byte-buffer))
(stream/write buf (string->utf8 "hello"))
(stream/to-string buf)  ;; => "hello"
```

### `stream/open-input`

Open a file for reading. Returns a buffered input stream. Sandbox-gated (`FS_READ`).

```sema
(define s (stream/open-input "data.csv"))
(define contents (stream/read-all s))
(stream/close s)
```

### `stream/open-output`

Open (or create) a file for writing. Returns a buffered output stream. Sandbox-gated (`FS_WRITE`).

```sema
(define s (stream/open-output "output.txt"))
(stream/write-string s "hello world\n")
(stream/close s)
```

## Reading

### `stream/read`

Read up to `n` bytes, returning a bytevector. Returns fewer bytes at EOF.

```sema
(stream/read s 1024)   ;; => bytevector (up to 1024 bytes)
```

### `stream/read-byte`

Read a single byte. Returns an integer 0–255, or `nil` at EOF.

```sema
(stream/read-byte s)   ;; => 65 (or nil at EOF)
```

### `stream/read-line`

Read until newline (`\n`), returning a string without the newline. Strips trailing `\r` for Windows line endings. Returns `nil` at EOF.

```sema
(stream/read-line s)   ;; => "first line" (or nil)
```

### `stream/read-all`

Read the stream into a bytevector. An optional byte cap defaults to 256 MiB;
the call fails before growing its result beyond the cap.

```sema
(define data (stream/read-all s (* 8 1024 1024))) ; 8 MiB maximum
(utf8->string data)    ; convert to string if text
```

## Writing

### `stream/write`

Write a bytevector. Returns the number of bytes written.

```sema
(stream/write s (bytevector 72 101 108 108 111))  ;; => 5
```

### `stream/write-byte`

Write a single byte (integer 0–255).

```sema
(stream/write-byte s 10)   ; write a newline
```

### `stream/write-string`

Write a string as UTF-8 bytes. Returns the number of bytes written.

```sema
(stream/write-string s "hello")   ;; => 5
```

## Control

### `stream/close`

Close a stream, releasing the underlying resource. Double-close is a no-op.

```sema
(stream/close s)
(stream/close s)   ; safe, does nothing
```

### `stream/flush`

Flush any buffered output to the underlying sink.

```sema
(stream/flush s)
```

### `stream/copy`

Copy bytes from one stream to another. Returns total bytes copied. An optional
byte cap defaults to 256 MiB, and the first over-limit chunk is rejected before
it is written.

Inside the cooperative runtime, stdin remains cancellable and a copy with one
file-backed side is offloaded. File-to-file copy requires two resource gates and
therefore fails promptly; use bounded `stream/read`/`stream/write` chunks for
that case.

```sema
(let ((in (stream/from-string "hello"))
      (out (stream/byte-buffer)))
  (stream/copy in out 1024)) ;; => 5
```

## Introspection

### `stream?`

Type predicate — returns `#t` if the value is a stream.

```sema
(stream? (stream/byte-buffer))    ;; => #t
(stream? 42)                      ;; => #f
```

### `stream/readable?`, `stream/writable?`

Check the direction of a stream.

```sema
(stream/readable? (stream/from-string "x"))   ;; => #t
(stream/writable? (stream/from-string "x"))   ;; => #f
(stream/writable? (stream/byte-buffer))       ;; => #t
```

### `stream/available?`

Returns `#t` if data is ready to read without blocking.

```sema
(stream/available? (stream/from-string "x"))  ;; => #t
(stream/available? (stream/from-string ""))   ;; => #f
```

### `stream/type`

Returns a string describing the stream implementation.

```sema
(stream/type (stream/byte-buffer))         ;; => "byte-buffer"
(stream/type (stream/from-string "x"))     ;; => "string"
(stream/type (stream/open-input "f.txt"))  ;; => "file-input"
(stream/type *stdout*)                     ;; => "stdout"
```

## Extraction (Byte Buffers)

### `stream/to-bytes`

Extract the accumulated contents of a byte-buffer stream as a bytevector.

```sema
(let ((s (stream/byte-buffer)))
  (stream/write s (bytevector 1 2 3))
  (stream/to-bytes s))   ;; => #u8(1 2 3)
```

### `stream/to-string`

Extract the contents of a byte-buffer stream as a UTF-8 string.

```sema
(let ((s (stream/byte-buffer)))
  (stream/write-string s "hello")
  (stream/to-string s))   ;; => "hello"
```

## Standard I/O

Three global streams are available for console I/O:

| Stream | Direction | Description |
|--------|-----------|-------------|
| `*stdin*` | Readable | Standard input |
| `*stdout*` | Writable | Standard output |
| `*stderr*` | Writable | Standard error |

```sema
(stream/write-string *stdout* "prompt> ")
(stream/flush *stdout*)
(stream/write-string *stderr* "warning: something happened\n")
```

## Resource Management

### `with-stream`

Macro that binds a stream, executes the body, and automatically closes the stream on exit — even if an error is thrown.

```sema
(with-stream (s (stream/open-input "data.txt"))
  (stream/read-all s))
;; s is closed here, even if read-all threw an error

;; Write to a file
(with-stream (out (stream/open-output "output.txt"))
  (stream/write-string out "line 1\n")
  (stream/write-string out "line 2\n"))
;; file is flushed and closed
```

## Patterns

### Line-by-Line Processing

```sema
(with-stream (s (stream/open-input "log.txt"))
  (let loop ((line (stream/read-line s))
             (count 0))
    (if (nil? line)
      count
      (loop (stream/read-line s) (+ count 1)))))
```

### Building a String Incrementally

```sema
(let ((buf (stream/byte-buffer)))
  (stream/write-string buf "{")
  (stream/write-string buf "\"key\": \"value\"")
  (stream/write-string buf "}")
  (stream/to-string buf))   ;; => "{\"key\": \"value\"}"
```

### File Copy

```sema
(with-stream (in (stream/open-input "photo.jpg"))
  (with-stream (out (stream/open-output "backup.jpg"))
    (let loop ((total 0))
      (let ((chunk (stream/read in 8192)))
        (if (= (bytes/length chunk) 0)
          total
          (begin
            (stream/write out chunk)
            (loop (+ total (bytes/length chunk)))))))))
```

## Error Handling

Reading a closed stream or writing to a read-only stream throws an error caught with `try`/`catch`:

```sema
(try
  (let ((s (stream/from-string "x")))
    (stream/close s)
    (stream/read s 1))    ; throws "stream is closed"
  (catch e
    (println (str "Error: " e))))
```
