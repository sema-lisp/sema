---
outline: [2, 3]
---

# Standard Library

Sema ships with a **comprehensive standard library** of built-in functions across many modules, covering everything from string manipulation and file I/O to HTTP requests, regex, and cryptographic hashing.

## Naming Conventions

Sema's stdlib follows consistent naming patterns:

| Pattern           | Convention           | Example                                |
| ----------------- | -------------------- | -------------------------------------- |
| `module/function` | Slash-namespaced     | `string/trim`, `file/read`, `math/gcd` |
| `legacy-name`     | Scheme compat aliases | `string-append` → `string/append`      |
| `type->type`      | Arrow conversions    | `string/to-symbol`, `list->vector`      |
| `predicate?`      | Predicate suffix     | `null?`, `list?`, `even?`              |

### Naming aliases

Several functions are registered under both a legacy (Scheme-style) name and a canonical
slash-namespaced or `predicate?` name (Decision #24). Both forms are kept for backward
compatibility; new code should prefer the canonical form on the right.

| Legacy name           | Canonical alias        |
| --------------------- | ---------------------- |
| `any`                 | `any?`                 |
| `every`               | `every?`               |
| `time-ms`             | `time/now-ms`          |
| `hash-map`            | `map/new`              |
| `promise-forced?`     | `async/forced?`        |
| `tools->routes`       | `route/from-tools`     |
| `make-bytevector`     | `bytevector/make` (also `bytevector/new`) |
| `bytevector-length`   | `bytevector/length`    |
| `bytevector-u8-ref`   | `bytevector/u8-ref` (also `bytevector/ref`) |
| `bytevector-u8-set!`  | `bytevector/u8-set!` (also `bytevector/set!`) |
| `bytevector-copy`     | `bytevector/copy`      |
| `bytevector-append`   | `bytevector/append`    |
| `bytevector->list`    | `bytevector/to-list`   |
| `list->bytevector`    | `bytevector/from-list` (also `list/to-bytevector`) |

Predicates (`bytevector?` etc.) and the bare `bytevector` varargs constructor keep their
short canonical names — predicates always stay un-namespaced.

## Quick Reference

### [Math & Arithmetic](./math)

| Function                                                                       | Description               |
| ------------------------------------------------------------------------------ | ------------------------- |
| `+`, `-`, `*`, `/`, `mod`                                                      | Basic arithmetic          |
| `<`, `>`, `<=`, `>=`, `=`                                                      | Comparison                |
| `abs`, `min`, `max`, `pow`, `sqrt`, `log`                                      | Numeric utilities         |
| `floor`, `ceil`, `round`, `truncate`                                           | Rounding                  |
| `sin`, `cos`, `math/tan`                                                       | Trigonometry              |
| `math/asin`, `math/acos`, `math/atan`, `math/atan2`                            | Inverse trig              |
| `math/sinh`, `math/cosh`, `math/tanh`                                          | Hyperbolic                |
| `math/exp`, `math/log10`, `math/log2`                                          | Exponential & logarithmic |
| `math/gcd`, `math/lcm`, `math/quotient`, `math/remainder`                      | Integer math              |
| `math/random`, `math/random-int`                                               | Random numbers            |
| `math/clamp`, `math/sign`, `math/lerp`, `math/map-range`                       | Interpolation & clamping  |
| `math/degrees->radians`, `math/radians->degrees`                               | Angle conversion          |
| `even?`, `odd?`, `positive?`, `negative?`, `zero?`                             | Numeric predicates        |
| `math/nan?`, `math/infinite?`                                                  | Float predicates          |
| `pi`, `e`, `math/infinity`, `math/nan`                                         | Constants                 |
| `bit/and`, `bit/or`, `bit/xor`, `bit/not`, `bit/shift-left`, `bit/shift-right` | Bitwise operations        |

### [Strings & Characters](./strings)

| Function                                                                            | Description               |
| ----------------------------------------------------------------------------------- | ------------------------- |
| `string/append`, `string/length`, `string/ref`, `string/slice`                      | Core string ops           |
| `str`, `format`                                                                     | Conversion & formatting   |
| `string/split`, `string/join`, `string/trim`                                        | Split, join, trim         |
| `string/upper`, `string/lower`, `string/capitalize`, `string/title-case`            | Case conversion           |
| `string/contains?`, `string/starts-with?`, `string/ends-with?`                      | Search predicates         |
| `string/replace`, `string/index-of`, `string/last-index-of`, `string/reverse`       | Manipulation              |
| `string/chars`, `string/repeat`, `string/pad-left`, `string/pad-right`              | Utilities                 |
| `string/map`, `string/number?`, `string/empty?`                                     | Higher-order & predicates |
| `string/after`, `string/before`, `string/between`, `string/take`                    | Slicing & extraction      |
| `string/chop-start`, `string/chop-end`, `string/ensure-start`, `string/ensure-end`  | Prefix & suffix           |
| `string/wrap`, `string/unwrap`, `string/remove`                                     | Wrapping & removal        |
| `string/replace-first`, `string/replace-last`                                       | Targeted replacement      |
| `string/snake-case`, `string/kebab-case`, `string/camel-case`, `string/pascal-case` | Case conversion           |
| `string/headline`, `string/words`                                                   | Headline & word splitting |
| `char/to-integer`, `integer/to-char`, `char/alphabetic?`, ...                           | Character operations      |
| `string/to-number`, `number/to-string`, `string/to-symbol`, ...                           | Type conversions          |

### [Lists](./lists)

| Function                                                                | Description                   |
| ----------------------------------------------------------------------- | ----------------------------- |
| `list`, `cons`, `car`, `cdr`, `first`, `rest`                           | Construction & access         |
| `cadr`, `caddr`, `last`, `nth`                                          | Positional access             |
| `length`, `append`, `reverse`, `range`                                  | Basic operations              |
| `map`, `filter`, `foldl`, `foldr`, `reduce`, `flat-map`                 | Higher-order functions        |
| `sort`, `sort-by`, `apply`, `for-each`                                  | Ordering & application        |
| `take`, `drop`, `flatten`, `flatten-deep`, `zip`, `partition`           | Sublists                      |
| `member`, `any`, `every`, `list/index-of`, `list/unique`, `list/dedupe` | Searching                     |
| `list/group-by`, `list/interleave`, `list/chunk`, `frequencies`         | Grouping                      |
| `list/sum`, `list/min`, `list/max`                                      | Aggregation                   |
| `list/shuffle`, `list/pick`                                             | Random                        |
| `list/repeat`, `make-list`, `iota`                                      | Construction                  |
| `list/split-at`, `list/take-while`, `list/drop-while`                   | Splitting                     |
| `assoc`, `assq`, `assv`                                                 | Association lists             |
| `interpose`                                                             | Interleaving                  |
| `list/reject`, `list/find`, `list/sole`                                 | Filtering & searching         |
| `list/pluck`, `list/key-by`                                             | Map extraction                |
| `list/avg`, `list/median`, `list/mode`                                  | Statistics                    |
| `list/diff`, `list/intersect`, `list/duplicates`                        | Set operations                |
| `list/sliding`, `list/page`, `list/cross-join`                          | Windowing & pagination        |
| `list/pad`, `list/join`, `list/times`                                   | Padding, joining & generation |
| `tap`                                                                   | Utility                       |

### [Vectors](./vectors)

| Function                       | Description     |
| ------------------------------ | --------------- |
| `vector`                       | Create a vector |
| `vector->list`, `list->vector` | Conversion      |

### [Maps & HashMaps](./maps)

| Function                                           | Description                  |
| -------------------------------------------------- | ---------------------------- |
| `map/new`, `get`, `assoc`, `dissoc`, `merge`      | Core map ops                 |
| `keys`, `vals`, `contains?`, `count`               | Inspection                   |
| `map/entries`, `map/from-entries`                  | Entry conversion             |
| `map/map-vals`, `map/map-keys`, `map/filter`       | Higher-order                 |
| `map/select-keys`, `map/update`                    | Selection & update           |
| `map/sort-keys`, `map/except`, `map/zip`           | Sorting, exclusion & zipping |
| `hashmap/new`, `hashmap/get`, `hashmap/assoc`, ... | HashMap operations           |

### [Predicates & Type Checking](./predicates)

| Function                                                          | Description           |
| ----------------------------------------------------------------- | --------------------- |
| `null?`, `nil?`, `empty?`, `list?`, `pair?`                       | Collection predicates |
| `number?`, `integer?`, `float?`, `string?`, `symbol?`, `keyword?` | Type predicates       |
| `char?`, `record?`, `bytevector?`, `bool?`, `fn?`                 | More type predicates  |
| `map?`, `vector?`                                                 | Container predicates  |
| `promise?`, `promise-forced?`                                     | Promise predicates    |
| `eq?`, `=`, `zero?`, `even?`, `odd?`, `positive?`, `negative?`    | Equality & numeric    |
| `prompt?`, `message?`, `conversation?`, `tool?`, `agent?`         | LLM type predicates   |

### [File I/O & Paths](./file-io)

| Function                                                                                                      | Description                  |
| ------------------------------------------------------------------------------------------------------------- | ---------------------------- |
| `display`, `println`, `pprint`, `print`, `io/print-error`, `io/println-error`, `newline`, `io/read-line`, `io/read-stdin`, `io/eof?`, `io/flush` | Console I/O                  |
| `file/read`, `file/write`, `file/append`                                                                      | File read/write              |
| `file/read-bytes`, `file/write-bytes`                                                                         | Binary file I/O              |
| `file/read-lines`, `file/write-lines`                                                                         | Line-based I/O               |
| `file/for-each-line`, `file/fold-lines`, `file/fold-lines-bytes`                                              | Streaming line I/O           |
| `file/delete`, `file/rename`, `file/copy`                                                                     | File management              |
| `file/exists?`, `file/is-file?`, `file/is-directory?`, `file/is-symlink?`                                     | File predicates              |
| `file/list`, `file/mkdir`, `file/info`                                                                        | Directory operations         |
| `file/glob`                                                                                                   | File globbing                |
| `path/join`, `path/dirname`, `path/basename`, `path/extension`, `path/absolute`                               | Path manipulation            |
| `path/ext`, `path/stem`, `path/dir`, `path/filename`, `path/absolute?`                                        | Path predicates & components |

### [PDF Processing](./pdf)

| Function                 | Description                                           |
| ------------------------ | ----------------------------------------------------- |
| `pdf/extract-text`       | Extract all text from a PDF                           |
| `pdf/extract-text-pages` | Extract text per page (returns list)                  |
| `pdf/page-count`         | Get number of pages                                   |
| `pdf/metadata`           | Get metadata map (`:title`, `:author`, `:pages`, ...) |

### [HTTP & JSON](./http-json)

| Function                                                           | Description        |
| ------------------------------------------------------------------ | ------------------ |
| `http/get`, `http/post`, `http/put`, `http/delete`, `http/request` | HTTP methods       |
| `json/encode`, `json/encode-pretty`, `json/decode`                 | JSON serialization |

### [Web Server](./web-server)

| Function                                                                         | Description            |
| -------------------------------------------------------------------------------- | ---------------------- |
| `http/serve`                                                                     | Start an HTTP server   |
| `http/router`                                                                    | Data-driven routing    |
| `http/ok`, `http/created`, `http/no-content`, `http/not-found`, `http/error`     | JSON response helpers  |
| `http/redirect`                                                                  | HTTP redirect          |
| `http/html`, `http/text`                                                         | Content-type responses |
| `http/file`                                                                      | Serve a file from disk |
| `http/stream`                                                                    | SSE streaming          |
| `http/websocket`                                                                 | WebSocket connections  |

### [Regex](./regex)

| Function                                            | Description             |
| --------------------------------------------------- | ----------------------- |
| `regex/match?`, `regex/match`, `regex/find-all`     | Matching                |
| `regex/replace`, `regex/replace-all`, `regex/split` | Replacement & splitting |

### [CSV, Crypto & Encoding](./csv)

| Function                                      | Description     |
| --------------------------------------------- | --------------- |
| `csv/parse`, `csv/parse-maps`, `csv/encode`   | CSV operations  |
| `uuid/v4`                                     | UUID generation |
| `base64/encode`, `base64/decode`              | Base64 encoding |
| `base64/encode-bytes`, `base64/decode-bytes`  | Binary Base64   |
| `hash/sha256`, `hash/md5`, `hash/hmac-sha256` | Hashing         |

### [Date & Time](./datetime)

| Function                    | Description          |
| --------------------------- | -------------------- |
| `time/now`, `time-ms`       | Current time         |
| `time/format`, `time/parse` | Formatting & parsing |
| `time/date-parts`           | Date decomposition   |
| `time/add`, `time/diff`     | Arithmetic           |
| `sleep`                     | Delay execution      |

### [System](./system)

| Function                                                    | Description           |
| ----------------------------------------------------------- | --------------------- |
| `env`, `sys/env-all`, `sys/set-env`                         | Environment variables |
| `sys/args`, `sys/cwd`, `sys/platform`, `sys/os`, `sys/arch` | System info           |
| `sys/pid`, `sys/tty`, `sys/which`, `sys/elapsed`            | Process info          |
| `sys/interactive?`, `sys/hostname`, `sys/user`              | Session info          |
| `sys/home-dir`, `sys/temp-dir`                              | Directory paths       |
| `sys/term-size`                                             | Terminal size (Unix)  |
| `sys/on-signal`, `sys/check-signals`                        | Signal hooks (Unix)   |
| `shell`                                                     | Run shell commands    |
| `exit`                                                      | Exit process          |

### [Serial Ports](./serial)

| Function                                                   | Description                              |
| ---------------------------------------------------------- | ---------------------------------------- |
| `serial/list`                                              | List available device paths              |
| `serial/open`, `serial/close`                              | Open/close a port (returns int handle)   |
| `serial/write`, `serial/read-line`                         | Raw I/O                                  |
| `serial/send`                                              | Write line + read JSON response          |

### [Bytevectors](./bytevectors)

| Function                                                       | Description       |
| -------------------------------------------------------------- | ----------------- |
| `bytevector`, `bytevector/new`                                | Construction      |
| `bytevector/length`, `bytevector/ref`, `bytevector/set!` | Access & mutation |
| `bytevector/copy`, `bytevector/append`                         | Copy & append     |
| `bytevector/to-list`, `list/to-bytevector`                         | List conversion   |
| `utf8/to-string`, `string/to-utf8`                                 | String conversion |
| `bytes/length`, `bytes/ref`, `bytes/find`, `bytes/slice`           | Byte-oriented ops (hot loops) |
| `bytes/->string`, `bytes/parse-int10`                              | Byte decoding & parsing |

### [Streams](./streams)

| Function                                                              | Description              |
| --------------------------------------------------------------------- | ------------------------ |
| `stream/from-string`, `stream/from-bytes`, `stream/byte-buffer`       | In-memory streams        |
| `stream/open-input`, `stream/open-output`                             | File streams             |
| `stream/read`, `stream/read-byte`, `stream/read-line`, `stream/read-all` | Reading                  |
| `stream/write`, `stream/write-byte`, `stream/write-string`            | Writing                  |
| `stream/close`, `stream/flush`, `stream/copy`                         | Control                  |
| `stream?`, `stream/readable?`, `stream/writable?`, `stream/available?` | Predicates               |
| `stream/type`, `stream/to-bytes`, `stream/to-string`                  | Introspection & extraction |
| `*stdin*`, `*stdout*`, `*stderr*`                                     | Standard I/O globals     |
| `with-stream`                                                         | Resource management macro |

### [Concurrency](./concurrency)

| Function                                                              | Description              |
| --------------------------------------------------------------------- | ------------------------ |
| `async/spawn`, `async/await`, `async/all`, `async/race`               | Async task management    |
| `async/resolved`, `async/rejected`                                     | Pre-settled promises     |
| `async/run`, `async/sleep`, `async/timeout`                            | Scheduler control & deadlines |
| `async/cancel`, `async/cancelled?`                                     | Cancellation             |
| `async/promise?`, `async/resolved?`, `async/rejected?`, `async/pending?` | Promise predicates       |
| `channel/new`, `channel/send`, `channel/recv`, `channel/try-recv`      | Channel operations       |
| `channel/close`                                                        | Channel lifecycle        |
| `channel?`, `channel/closed?`, `channel/empty?`, `channel/full?`, `channel/count` | Channel predicates       |

### [Records](./records)

| Function             | Description          |
| -------------------- | -------------------- |
| `define-record-type` | Define a record type |
| `record?`            | Record predicate     |
| `type`               | Get record type tag  |

### [Terminal Styling](./terminal)

| Function                                                         | Description                         |
| ---------------------------------------------------------------- | ----------------------------------- |
| `term/bold`, `term/red`, `term/green`, ...                       | Individual style functions          |
| `term/style`                                                     | Apply multiple styles with keywords |
| `term/rgb`                                                       | 24-bit true color                   |
| `term/strip`                                                     | Remove ANSI escape codes            |
| `term/spinner-start`, `term/spinner-stop`, `term/spinner-update` | Animated spinners                   |
| `io/tty-raw!`, `io/tty-restore!`                                 | Raw-mode TTY (Unix)                 |
| `io/read-key`, `io/read-key-timeout`                             | Per-keystroke input (Unix)          |

### [Text Processing](./text-processing)

| Function                                                                  | Description                                |
| ------------------------------------------------------------------------- | ------------------------------------------ |
| `text/chunk`, `text/chunk-by-separator`, `text/split-sentences`           | Text chunking                              |
| `text/clean-whitespace`, `text/strip-html`                                | Text cleaning                              |
| `text/truncate`, `text/word-count`, `text/trim-indent`                    | Text utilities                             |
| `text/excerpt`, `text/normalize-newlines`                                 | Excerpt extraction & newline normalization |
| `prompt/template`, `prompt/render`                                        | Prompt templates                           |
| `document/create`, `document/text`, `document/metadata`, `document/chunk` | Document metadata                          |

### [SQLite](./sqlite)

| Function                         | Description                    |
| -------------------------------- | ------------------------------ |
| `db/open`, `db/open-memory`      | Open file or in-memory database |
| `db/exec`, `db/exec-batch`       | Execute statements             |
| `db/query`, `db/query-one`       | Query rows as maps             |
| `db/last-insert-id`              | Last inserted rowid            |
| `db/tables`                      | List tables                    |
| `db/close`                       | Close connection               |

### [Typed Arrays](./typed-arrays)

| Function                                                  | Description            |
| --------------------------------------------------------- | ---------------------- |
| `f64-array`, `i64-array`                                  | Create from values     |
| `f64-array/make`, `i64-array/make`                        | Create with fill       |
| `f64-array/range`, `i64-array/range`                      | Create from range      |
| `f64-array/from-list`, `i64-array/from-list`              | Convert from list      |
| `f64-array/ref`, `i64-array/ref`                          | Index access           |
| `f64-array/set!`, `i64-array/set!`                        | Set element (CoW)      |
| `f64-array/length`, `i64-array/length`                    | Length                 |
| `f64-array/sum`, `i64-array/sum`                          | Fast sum               |
| `f64-array/dot`                                           | Dot product            |
| `f64-array/map`, `i64-array/map`                          | Map over elements      |
| `f64-array/fold`, `i64-array/fold`                        | Fold over elements     |
| `f64-array?`, `i64-array?`                                | Type predicates        |

### [Mutable Containers](./mutable)

| Function                                                    | Description                        |
| ----------------------------------------------------------- | ---------------------------------- |
| `mutable-array/new`                                         | Create (empty, capacity, or n×fill) |
| `mutable-array/push!`, `mutable-array/set!`                 | In-place update (return the array) |
| `mutable-array/get`, `mutable-array/length`                 | Access                             |
| `mutable-array/->vector`                                    | Freeze to an immutable vector      |
| `mutable-cell/new`, `mutable-cell/get`, `mutable-cell/set!` | Single mutable slot                |

### [Context](./context)

| Function                                                          | Description                              |
| ----------------------------------------------------------------- | ---------------------------------------- |
| `context/set`, `context/get`, `context/has?`                      | Core key-value context                   |
| `context/remove`, `context/pull`, `context/all`                   | Retrieval & cleanup                      |
| `context/merge`, `context/clear`                                  | Bulk operations                          |
| `context/with`                                                    | Scoped overrides (auto-restores on exit) |
| `context/push`, `context/stack`, `context/pop`                    | Named stacks                             |
| `context/set-hidden`, `context/get-hidden`, `context/has-hidden?` | Hidden (non-logged) context              |

### [Key-Value Store](./kv-store)

| Function                        | Description                    |
| ------------------------------- | ------------------------------ |
| `kv/open`, `kv/close`           | Open/close a JSON-backed store |
| `kv/get`, `kv/set`, `kv/delete` | CRUD operations                |
| `kv/keys`                       | List all keys                  |

### [TOML](./toml)

| Function                        | Description          |
| ------------------------------- | -------------------- |
| `toml/decode`                   | Decode TOML to Sema  |
| `toml/encode`                   | Encode Sema to TOML  |

### [Playground & WASM](./playground)

| Function              | Description                                            |
| --------------------- | ------------------------------------------------------ |
| `web/user-agent`      | Browser user agent string (WASM only)                  |
| `web/user-agent-data` | Structured browser info map (Chromium only, WASM only) |
