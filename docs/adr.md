# Sema Design Decisions

## Implementation Decisions Made During Build

### 1. `prompt` special form: syntax-directed, not evaluation-first

- `(prompt (system "text") (user "text"))` — the role keywords (`system`, `user`, `assistant`, `tool`) are checked in the raw syntax BEFORE evaluation
- This prevents the evaluator from trying to call `system` as a function
- Other expression forms inside `prompt` ARE evaluated normally
- This is the same pattern as Clojure's `(defn name [args] body)` — the `name` and `[args]` are syntax, not evaluated

### 2. `message` form uses keywords, `prompt` uses bare symbols

- `(message :user "Hello")` — keyword role, fully evaluated
- `(prompt (user "Hello"))` — bare symbol role, syntax-directed
- This gives two entry points: `prompt` for ergonomic multi-message construction, `message` for dynamic single-message creation

### 3. Environment: `Rc<Env>` with parent chain, `RefCell<BTreeMap>`

> **Superseded** (see #43/#44): bindings are now `Rc<RefCell<hashbrown::HashMap<Spur, Value>>>` — Spur-keyed after string interning, hashbrown for speed. The BTreeMap-for-ordering rationale no longer applies.

- Single-threaded: `Rc` not `Arc`
- `BTreeMap` over `HashMap` for deterministic ordering (matters for printing, testing)
- `set_existing` walks the chain for `set!` semantics

### 4. Trampoline-based TCO

- `eval_step` returns `Trampoline::Value` or `Trampoline::Eval(expr, env)`
- The trampoline loop in `eval_value` drives tail calls without growing the stack
- Special forms return `Trampoline::Eval` for the last expression in bodies (`begin`, `if`, `let`, etc.)

### 5. Lambda self-reference

- Named lambdas and `define`-d functions automatically bind their own name in the closure env
- This enables recursion without `letrec` or Y-combinator
- Simple approach: inject the binding when applying the lambda

### 6. stdlib `call_function` duplication

> **Superseded** (see #61): the mini-eval was deleted. `call_function` is now a thin dispatcher to `sema_core::call_callback`, which routes to the real evaluator registered at startup. The "complex expressions may not work" trade-off no longer exists.

- `sema-stdlib/src/list.rs` has its own `call_function` and mini-eval for HOF support (`map`, `filter`, `foldl`)
- This avoids a circular dependency (sema-stdlib can't depend on sema-eval)
- The mini-eval handles: symbol lookup, function application, self-evaluating literals
- Trade-off: complex expressions inside HOF callbacks may not work with the mini-eval, but lambdas work fine

### 7. LLM provider: thread_local registry

- Uses `thread_local!` for the provider registry and usage tracking
- Avoids `Arc<Mutex>` complexity since the Lisp is single-threaded
- Provider auto-configures from env vars (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`)

### 8. tokio runtime per provider

- Each provider creates its own `tokio::runtime::Runtime`
- Uses `block_on` to present a sync interface
- This keeps the Lisp evaluator synchronous while allowing async HTTP

### 9. Keywords as functions

- `:keyword` in function position works like `(get map :keyword)`: `(:name person)`
- Implemented in the evaluator's `eval_step` for `Value::Keyword` in head position

### 10. Value Ord implementation

- `Value` implements `Ord` by comparing within type, then by type_order between types
- Floats compared by bit representation (not mathematically correct but gives consistent ordering)
- Required for `BTreeMap<Value, Value>` (map keys)

## File Layout Summary

```
crates/sema-core/src/
  lib.rs            # re-exports
  value.rs          # Value enum, Env, Lambda, Macro, Message, Prompt, Conversation, etc.
  error.rs          # SemaError enum

crates/sema-reader/src/
  lib.rs            # pub fn read, read_many
  lexer.rs          # tokenize → Vec<SpannedToken>
  reader.rs         # Parser: tokens → Value (+ tests)

crates/sema-eval/src/
  lib.rs            # pub fn eval, eval_string, Interpreter + module system re-exports
  eval.rs           # core eval loop with trampoline + module system thread-local state
  special_forms.rs  # define, if, let, let*, letrec, lambda, begin, defmacro, quasiquote,
                    # prompt, message, deftool, defagent, load, try, catch, throw,
                    # module, import, case, eval, macroexpand

crates/sema-stdlib/src/
  lib.rs            # register_stdlib
  arithmetic.rs     # +, -, *, /, mod
  comparison.rs     # <, >, <=, >=, =, eq?, not, zero?, even?, odd?
  list.rs           # car, cdr, cons, map (multi-list), filter, foldl, foldr, reduce,
                    # range, sort, apply, take, drop, last, zip, flatten, member,
                    # any, every, partition
  string.rs         # string-append, string/split, format, str, type conversions
  predicates.rs     # null?, list?, number?, string?, etc.
  map.rs            # get, assoc, dissoc, keys, vals, merge, contains?
  io.rs             # display, println, file/*, path/*, read, read-many, error
  math.rs           # abs, min, max, floor, ceil, sqrt, pow, pi, trig, log, random, clamp, sign, gcd, lcm
  bitwise.rs        # bit/and, bit/or, bit/xor, bit/not, bit/shift-left, bit/shift-right
  crypto.rs         # uuid/v4, base64/encode, base64/decode, hash/sha256
  datetime.rs       # time/now, time/format, time/parse, time/date-parts
  csv_ops.rs        # csv/parse, csv/parse-maps, csv/encode
  system.rs         # env, shell, exit, time-ms, sleep, sys/args, sys/cwd, sys/platform, sys/env-all
  json.rs           # json/encode, json/decode, json/encode-pretty
  meta.rs           # gensym
  regex_ops.rs      # regex/match?, regex/match, regex/find-all, regex/replace, regex/split
  http.rs           # http/get, http/post, http/put, http/delete, http/request

crates/sema-llm/src/
  lib.rs            # module declarations
  types.rs          # ChatRequest, ChatResponse, ToolCall, Usage, LlmError
  provider.rs       # LlmProvider trait, ProviderRegistry
  anthropic.rs      # AnthropicProvider (Messages API)
  openai.rs         # OpenAiProvider (chat/completions API)
  builtins.rs       # llm/configure, llm/complete, llm/chat (with tool loop), llm/extract,
                    # llm/classify, llm/pmap, agent/run, conversation/*, prompt-*,
                    # message-*, llm/last-usage, llm/session-usage, llm/reset-usage

crates/sema/src/
  main.rs           # CLI (clap) + file runner (REPL lives under crates/sema/src/repl/, reedline-based)
```

## Phase 4 Decisions

### 11. Eval callback for tool execution

- Tool handlers (lambdas in deftool) need the full evaluator (for `let`, `if`, `cond`, etc.)
- sema-llm can't depend on sema-eval (circular dependency)
- Solution: thread_local `EvalCallback` registered by the Interpreter on init
- `set_eval_callback(eval_value)` gives the LLM builtins access to the full evaluator
- Falls back to `simple_eval` if no callback registered

### 12. Tool execution loop

- `llm/chat` with `:tools` option runs an automatic loop:
  send → check tool_calls → execute handlers → send results → repeat
- Max rounds configurable via `:max-tool-rounds` (default 10)
- Tool results sent back as user messages: `[Tool result for name]: content`
- Parameters converted from JSON to Sema values in schema-defined order

### 13. deftool / defagent as special forms

- `deftool` is a special form (not a macro) — evaluates description, params, and handler
- Creates a `ToolDef` value and binds it in the current env
- `defagent` similarly creates an `Agent` value with system, tools, max-turns, model
- Agent type added to core Value enum

### 14. Macro expansion environment

- Fixed bug: macros now expand in a child of the caller's env (not a bare env)
- This gives macros access to `list`, `cons`, and other builtins during expansion
- Macro params shadow any same-named bindings from the caller env

### 15. load as special form

- `(load "file.sema")` reads and evaluates a file in the CURRENT environment
- This means loaded definitions are available in the caller's scope
- Different from the stdlib `load` function (which just parsed and returned forms)

## Phase 6 Decisions

### 16. `try`/`catch`/`throw` over R7RS `guard`

- R7RS defines `(guard (exn (clause ...)) body)` with `cond`-style clauses
- We chose `(try body... (catch e handler...))` for several reasons:
  - Familiar to users of Java, Python, JavaScript, Clojure
  - Simpler to implement — single catch variable bound to error map
  - Error maps with `:type` keyword enable pattern matching via `cond` in the handler
  - `throw` takes any value (not just strings), stored in `:value` key
- All `SemaError` variants are catchable and converted to Sema maps:
  - `:type` — keyword like `:eval`, `:type-error`, `:unbound`, `:user`, `:llm`, `:io`, `:arity`, `:reader`
  - `:message` — human-readable string
  - Variant-specific keys: `:value` (UserException), `:expected`/`:got` (Type), `:name` (Unbound)
- `try` body is fully evaluated (loses TCO — standard behavior for exception-protected code)
- `catch` handler gets TCO on last expression

### 17. Named `let` with TCO

- Detected by checking if `args[0]` is a symbol (vs. a list of bindings)
- Creates a lambda with the loop name and binds it in the new environment
- Recursive calls resolve to the lambda, `apply_lambda` returns `Trampoline::Eval` → full TCO
- No new AST node or special dispatch — reuses existing `eval_let` function

### 18. `letrec` two-pass binding

- Pass 1: bind all names to `Nil` in new env (placeholders)
- Pass 2: evaluate init exprs in the new env, update bindings
- This allows init exprs to close over each other (mutual recursion via lambdas)
- Simpler than R7RS "locations" semantics — `Nil` placeholder is observable if read before assignment

### 19. Module system: file-path-based, not name-based

- Modules are identified by canonical file path, not by module name
- `(module name (export sym1 ...) body...)` — name is documentation only
- `(import "path.sema")` — always uses file paths
- Design rationale:
  - No module registry or search path configuration needed
  - Relative paths resolve from the importing file's directory
  - Absolute paths work too
  - Simple to understand and debug

### 20. Module isolation

- `create_module_env` walks the env parent chain to the root (global/stdlib) env
- Module env is a child of root — gets builtins but not caller's bindings
- This prevents accidental coupling between modules and callers
- Module cache stores exports by canonical path — each module loaded only once

### 21. Thread-local module state

- `MODULE_CACHE`, `CURRENT_FILE`, `MODULE_EXPORTS` are `thread_local!`
- Consistent with existing pattern (LLM provider registry uses thread_local)
- `CURRENT_FILE` is a stack — supports nested `load`/`import`
- `MODULE_EXPORTS` is `Option<Vec<String>>` — `None` means "no module form, export everything"

### 22. `load` updated for relative path resolution

- `load` now resolves relative to `current_file_dir()` (from `CURRENT_FILE` stack)
- Falls back to current working directory if no file context
- `load` also pushes/pops file path for nested resolution
- Breaking change: previously always resolved from cwd

### 23. Extended list operations in stdlib mini-eval

- All new list ops (`take`, `drop`, `zip`, etc.) are NativeFn — no evaluator needed
- Multi-list `map` reuses existing `call_function` from stdlib
- HOF-based ops (`any`, `every`, `partition`, `reduce`, `foldr`) also use `call_function`
- No changes to the mini-eval were needed

## Phase 7 Decisions

### 24. Slash-namespaced naming convention

- All new function groups use `namespace/function` naming: `file/`, `path/`, `regex/`, `http/`, `json/`, `string/`
- Legacy Scheme names (`read-file`, `write-file`, etc.) renamed to `file/read`, `file/write`, etc. for consistency
- Rationale: the slash acts as a logical namespace (like Clojure) — groups related functions into discoverable families
- Traditional Scheme names kept only for: `string-append`, `string-length`, `string-ref`, `substring` (too deeply entrenched in Scheme)
- Predicates like `null?`, `list?`, `map?` remain un-namespaced — they're universal
- Arrow conversions remain: `string->symbol`, `keyword->string`, etc. — standard Scheme convention

### 25. `case` uses PartialEq on unevaluated datums

- Datum lists are NOT evaluated — `(case x ((1 2) "match"))` compares x against literal `1` and `2`
- This matches R5RS semantics and works naturally with keywords: `(case :b ((:a :b) "match"))`
- TCO on last body expression of matching clause

### 26. `eval` as a special form (not builtin)

- `eval` evaluates its argument, then returns `Trampoline::Eval(result, env)` for TCO
- Must be a special form (not NativeFn) because it needs access to the current environment
- The evaluated expression runs in the caller's environment (not a fresh one)

### 27. HTTP client: thread-local runtime + client

- `http.rs` uses `thread_local!` for both `tokio::Runtime` and `reqwest::Client`
- Client reuse enables connection pooling across multiple requests
- Map bodies auto-serialized as JSON via `crate::json::value_to_json`
- Response is always a map: `{:status N :headers {...} :body "string"}`
- Tests marked `#[ignore]` since they require network access

### 28. `macroexpand` expands once

- `macroexpand` does a single expansion step (not recursive)
- Evaluates its argument (so you pass `'(macro-call args...)`)
- If the form starts with a macro name, expands it; otherwise returns as-is
- Uses the existing `apply_macro` function (made `pub` for this purpose)

## Phase 8 Decisions

### 29. Duplicated `call_function` in map.rs

> **Superseded** (see #61): the duplication was removed — `map.rs` now does `use crate::list::call_function;` and both route through `sema_core::call_callback`.

- Map HOFs (`map/map-vals`, `map/filter`, `map/update`) need `call_function` like list.rs
- Duplicated ~60 lines of `call_function` + `sema_eval_value` rather than refactoring to shared module
- Same pattern as list.rs: handles NativeFn and Lambda, mini-eval for lambda bodies
- Rationale: avoids refactoring existing working code; both copies are stable

### 30. Bitwise ops renamed from `bit-*` to `bit/*`

- Follows the slash-namespaced convention (Decision #24)
- Old `bit-and`, `bit-or` etc. renamed to `bit/and`, `bit/or` etc.

### 31. `time/now` returns f64 seconds (not milliseconds)

- Unix timestamp as float seconds (e.g., `1707955200.123`)
- Subsecond precision via fractional part
- Different from `time-ms` which returns integer milliseconds
- Rationale: float seconds is the standard unix timestamp format, works naturally with chrono

### 32. CSV values are always strings

- `csv/parse` and `csv/parse-maps` return all fields as strings
- No automatic type coercion (CSV has no type information)
- Users can convert with `string->number`, `int`, `float` as needed

### 33. `map/filter` takes `(fn (k v) ...)` — two-argument predicate

- Unlike list `filter` which takes `(fn (item) ...)`
- Map filter needs both key and value for meaningful filtering
- Consistent with Clojure's `(filter (fn [[k v]] ...) map)` pattern

## CLI Design Decisions

### 34. CLI flag design follows Chez Scheme / Chicken Scheme conventions

- Surveyed Racket, Chez Scheme, Chicken Scheme, Clojure, Janet, Fennel, and Hy
- Core flags follow widespread Lisp conventions: `-e` (eval), `-l` (load), `-q` (quiet), `-i` (interactive), `-p` (print)
- `-p` always prints (even Nil) — useful for shell pipelines; `-e` skips Nil (standard REPL behavior)
- `-l` is repeatable: `sema -l a.sema -l b.sema` loads both before main execution
- `-i` keeps interpreter state after file/eval, then enters REPL — essential for debugging scripts
- `--no-init` / `--no-llm` skip `(llm/auto-configure)` — faster startup for scripts that don't need LLM
- `--chat-model` and `--chat-provider` set env vars (`SEMA_CHAT_MODEL`, `SEMA_CHAT_PROVIDER`) rather than reconfiguring the provider registry
  - Rationale: provider may not be configured yet; scripts can check `(env "SEMA_CHAT_MODEL")` explicitly
  - This avoids coupling CLI args to provider internals
- `sys/args` returns raw `std::env::args()` — standard behavior, user filters as needed
- `--version` uses `env!("CARGO_PKG_VERSION")` from Cargo.toml — single source of truth for version string

### 35. Multi-provider architecture: reuse OpenAiProvider for compatible APIs

- Groq, xAI, Mistral, and Moonshot all use the OpenAI chat/completions API format
- Rather than creating separate provider structs, `OpenAiProvider` was extended with `name` and `send_stream_options` fields
- Factory method `OpenAiProvider::named(name, api_key, base_url, model, send_stream_options)` creates named instances
- Mistral requires `send_stream_options=false` (rejects the `stream_options` field)
- Google Gemini and Ollama required new provider structs due to completely different APIs:
  - Gemini: auth via query param, `contents` format, `systemInstruction`, `generationConfig`, SSE streaming with `?alt=sse`
  - Ollama: no auth, NDJSON streaming (not SSE), `num_predict` instead of `max_tokens`, custom usage fields
- NDJSON parser (`ndjson.rs`) kept separate from SSE parser (`sse.rs`) — different wire protocols
- Embedding-only providers (Jina, Voyage, Cohere) implement `LlmProvider` but return errors for `complete()`
- `llm/auto-configure` registers ALL available providers, sets the first found as default (priority: Anthropic → OpenAI → Groq → xAI → Mistral → Moonshot → Gemini → Ollama)
- `llm/configure :ollama` does not require `:api-key` — the key extraction was made optional with per-arm validation

## Phase 9 Decisions

### 36. Slash-namespaced LLM accessors (legacy aliases removed)

- Renamed all LLM type accessors to use `/` namespace per Decision #24
- `tool-name` → `tool/name`, `agent-system` → `agent/system`, `prompt-messages` → `prompt/messages`, `message-role` → `message/role`, etc.
- Legacy hyphenated aliases were initially kept but later removed to avoid maintenance burden
- Only the slash-namespaced forms exist now: `tool/name`, `agent/system`, `prompt/messages`, `message/role`, etc.

### 37. Auto-retry on rate limiting

- `do_complete` now retries up to 3 times on `LlmError::RateLimited`
- Waits `min(retry_after_ms, 30000)` between retries using `std::thread::sleep`
- After 3 retries, returns a clear error: "rate limited after 3 retries"
- Only applies to `do_complete` (single completions); streaming is not retried

### 38. Gemini and Ollama tool-call support

- Gemini: sends `tools[].function_declarations` in the request, parses `functionCall` parts from response
- Ollama: sends OpenAI-compatible `tools` array, parses `message.tool_calls` from response
- Both generate synthetic IDs (`gemini-call-N`, `ollama-call-N`) since their APIs don't provide tool call IDs
- All providers now support the full tool loop via `llm/chat` with `:tools`

### 39. Provider introspection builtins

- `llm/set-default` — switch active provider at runtime (validates provider exists)
- `llm/list-providers` — returns sorted list of configured provider names as keywords
- `llm/current-provider` — returns map with `:name` and `:model` of active provider
- `llm/set-budget` / `llm/clear-budget` / `llm/budget-remaining` — expose budget control to Sema
- All provider management uses the existing `PROVIDER_REGISTRY` thread-local

### 40. HTTP timeouts on all providers

- All providers now use 120s HTTP timeout (matching Ollama's existing timeout)
- Prevents indefinite hangs on slow or unresponsive API endpoints
- Applied to: Anthropic, OpenAI, Gemini, Jina, Voyage, Cohere embedding providers

### 41. `pi` and `e` as constants, not functions

- Changed from zero-arg `NativeFn` registrations to direct `env.set()` bindings
- `pi` and `e` now evaluate as bare symbols to their float values (no parens needed)
- Rationale: mathematical constants should be values, not function calls — `(* 2 pi)` not `(* 2 (pi))`

### 42. Scheme-compat predicate aliases

- Added `pair?` (non-empty list), `boolean?` (= `bool?`), `procedure?` (= `fn?`), `equal?` (= `eq?`)
- Primary names remain `bool?`, `fn?`, `eq?` — aliases exist for Scheme compatibility
- `pair?` is new functionality: returns `#t` for non-empty lists (Sema has no dotted pairs/improper lists, so `pair?` ≡ non-empty `list?`)

## Performance Optimization Decisions

### 43. String interning for symbols and keywords (lasso)

- `Value::Symbol` and `Value::Keyword` store `Spur` (u32) instead of `Rc<String>`
- `Env::bindings` changed from `BTreeMap<String, Value>` to Spur-keyed lookups (today: `hashbrown::HashMap<Spur, Value>`, see #44)
- Thread-local `Rodeo` interner, accessed via `intern()/resolve()/with_resolved()`
- `Value::String` remains `Rc<String>` — arbitrary user strings are NOT interned
- Eq comparison of symbols/keywords is now O(1) integer comparison
- Ord comparison still resolves to lexicographic for deterministic BTreeMap ordering
- Mini-eval special form dispatch uses pre-interned Spur constants for O(1) matching (no string comparison)
- Consistent with existing `thread_local!` pattern (LLM provider, module cache)

### 44. HashMap variant for performance-critical accumulation (hashbrown)

- Added `Value::HashMap(Rc<hashbrown::HashMap<Value, Value>>)` as opt-in fast map
- `hashmap/new`, `hashmap/get`, `hashmap/assoc`, `hashmap/to-map`, `hashmap/keys`, `hashmap/contains?` builtins
- Existing `get`, `assoc`, `keys`, `vals`, `contains?`, `count`, `empty?` also work on HashMap
- `Value::Map` (BTreeMap) remains the default for deterministic ordered output
- HashMap used where O(1) lookup matters more than key ordering (e.g., 1BRC accumulator with ~400 entries)
- `Hash` impl added for `Value`: hashes discriminant + inner value; functions/maps hash by discriminant only
- COW optimization (Rc::make_mut) applies to HashMap assoc just like BTreeMap assoc
- HashMap Display sorts entries for deterministic output

### 45. SIMD byte search with memchr

- `memchr` crate used in inlined `string/split` for single-byte separator search
- Replaces `bytes.iter().position()` with `memchr::memchr()` (SIMD-optimized)
- Minimal impact on short strings but beneficial for longer string processing

## WASM Playground Decisions

### 46. WASM `sys/*` returns `"web"` not host OS detection

- `sys/platform` → `"web"`, `sys/arch` → `"wasm32"`, `sys/os` → `"web"`
- Rejected parsing `navigator.userAgent` — UA strings increasingly unreliable (reduction, masquerading, privacy)
- Rejected `navigator.platform` — deprecated API
- Rationale: code runs in WASM sandbox, not natively. Reporting `"macos"` would be misleading since OS-specific APIs (filesystem paths, processes, signals) don't exist
- Matches Go (`GOOS=js`), Rust (`wasm32-unknown-unknown`), Pyodide (`sys.platform="emscripten"`)
- Future: add `web/user-agent` as a separate WASM-only function for host hints

### 47. In-memory VFS for WASM playground (session-only)

- `thread_local! BTreeMap<String, String>` for files, `BTreeSet<String>` for directories
- Enables file I/O examples (turtle-svg, modules-demo, streaming-io) without async bridges
- Session-only — data lost on reload; acceptable for a playground
- Evaluated alternatives: IndexedDB (async sync overhead), OPFS (requires Web Worker for sync access)
- OPFS identified as the ideal future upgrade path — 10-100x faster than IDB, persistent, sync access via `FileSystemSyncAccessHandle` in Workers
- See `docs/plans/2026-02-14-wasm-shims-design.md` for full comparison and roadmap

### 48. HTTP stubs over async bridge for WASM MVP

- `http/*` functions return clear error messages instead of implementing async fetch
- Fundamental constraint: `NativeFn` is synchronous, browser `fetch()` is async (Promise-based)
- Cannot synchronously wait for a Promise on the main thread without deadlocking the event loop
- Future path: `eval_async` entry point + `Value::Promise` variant or suspend/resume effect system
- Worker + Atomics.wait approach rejected for MVP due to cross-origin isolation header requirement
- See `docs/plans/2026-02-14-wasm-shims-design.md` for detailed HTTP roadmap

### 49. Terminal styling as pass-through in WASM

- All `term/*` functions return text unchanged (ANSI codes useless in browser)
- 15 color/modifier functions + `term/style`, `term/strip`, `term/rgb`
- Enables examples using terminal colors to run without error, just without visual styling
- Future: could map to HTML `<span>` elements with CSS classes if playground supports rich output

### 50. Same-VM closure execution via NativeFn payload

- VM closures are wrapped as `Value::NativeFn` with an opaque `payload: Option<Rc<dyn Any>>` field on `NativeFn`
- `VmClosurePayload` stores `Rc<Closure>` + `Vec<Rc<Function>>` (function table from compilation context)
- Inside the VM, `call_value` checks `native.payload`, downcasts to `VmClosurePayload`, and calls `call_vm_closure` which pushes a `CallFrame` on the **same VM** — zero Rust stack growth
- Outside the VM (stdlib HOFs like `map`, `filter`), the `NativeFn::func` fallback creates a fresh VM — this is the interop bridge
- This approach avoids adding a new `Value::VmClosure` variant, keeping the `Value` enum unchanged
- Trade-off: the NativeFn fallback still recurses in Rust for stdlib HOF calls, but this is bounded (stdlib doesn't do deep recursion)

### 51. True TCO for VM closures via frame reuse

- `tail_call_vm_closure` reuses the current `CallFrame`'s stack base instead of pushing a new frame
- Truncates stack to current frame's base, writes new params, replaces `closure` and resets `pc` to 0
- Enables constant-stack-space tail recursion: tested at 100,000+ depth
- `Op::TailCall` bytecode instruction emitted by compiler for calls in tail position
- Mutual recursion at 1,000+ depth also works (each call pushes a frame, but no Rust recursion)

### 52. Named-let desugared to letrec+lambda in lowering

- `(let loop ((n init) ...) body...)` lowered to `(letrec ((loop (lambda (n ...) body...))) (loop init ...))`
- Eliminates `compile_named_let` in the compiler — reuses existing `compile_letrec` + `compile_lambda` paths
- Fixed two classes of bugs: self-reference slot corruption (Bug 1) and missing upvalue/func_id support (Bug 3)
- The `NamedLet` variant, `resolve_named_let`, and `compile_named_let` have been fully removed from the codebase
- Tail position flag propagated correctly to the initial `(loop init ...)` call

### 53. VM-per-Task cooperative async

- Each `async/spawn` creates a new VM instance sharing `Rc<Env>` globals and `Rc<Vec<Rc<Function>>>` with the parent
- A cooperative scheduler in `sema-vm/src/scheduler.rs` manages tasks with round-robin execution
- Yield is signaled via thread-local `YIELD_SIGNAL` in `sema-core/src/async_signal.rs`, not via error variants
- The VM checks the yield signal after every native function call (CALL_NATIVE, CALL_GLOBAL)
- On yield, the VM leaves a nil placeholder on the stack and advances PC past the call. On resume, the scheduler replaces the placeholder with the wake value via `replace_stack_top()`
- Replaces the replay model from PR #29 which re-executed entire task bodies, corrupting side effects

### 54. Async is VM-only, VM is default backend

- The bytecode VM is the default execution backend (CLI, REPL, notebook, playground)
- The tree-walker is available via `--tw` flag for backwards compatibility
- Async features (async/await, channels, task scheduler) require the VM backend
- The tree-walker returns a clear error: "async requires the VM backend (do not use --tw)"
- This acknowledges the tree-walker's deprecation path and avoids maintaining two async implementations

### 55. Move VM upvalues to open-close-on-popframe model (IMPLEMENTED, modified design — C1 NOT resolved)

Status: **implemented 2026-03-11** (commits `f691a55`, `3869228`, `346f46d`) — `UpvalueState::{Open,Closed}` in `crates/sema-vm/src/vm.rs`, `has_open_upvalues` flag removed, Load/StoreLocal stay branch-free. **Deviation from point 5 below:** the shipped variant calls `close_open_upvalues` *before every non-VM call* (vm.rs `call_callback` sites), instead of keeping cells open across the HOF bridge. Consequence: in-VM closure mutation works, but **audit bug C1 still reproduces** — `set!` inside a closure invoked via a stdlib HOF (`map`, `filter`, …) mutates a closed snapshot and is lost. See `docs/bugs/vm-set-lost-through-hof-callbacks.md` and `docs/limitations.md` #31. Fixing C1 requires either keeping cells open across the cross-VM bridge (original point 5) or routing HOF callbacks in-VM.

Context: the current VM eagerly closes upvalues at `MakeClosure` time and dual-writes mutations to both the parent's local slot and the closure's upvalue cell. This breaks down when a closure is called *outside* the parent VM (stdlib HOFs like `map`, `filter`, `for-each`, `sort-by`, `retry` route through `NativeFn::func` on a fresh VM — Decision #50). The fresh VM has its own copy of the upvalue cell; mutations there never propagate back to the parent's slot, and `set!` is silently lost.

Decision: switch to a Lua/Crafting-Interpreters-style **open upvalue runtime**:

1. **Heap-allocated upvalue cells.** Each upvalue is `Rc<RefCell<UpvalueCell>>` where `UpvalueCell` is either `Open { stack_addr: *mut Value /* logical slot id */ }` or `Closed { value: Value }`. While open, reads/writes go through the parent's stack slot; closed upvalues own their value.
2. **`open_upvalues` per `CallFrame`.** An intrusive list (sorted by stack slot, descending) of open upvalues pointing into this frame. Created lazily on `MakeClosure` — if a captured slot already has an open upvalue, reuse it.
3. **Close on frame exit.** `Return`, `Throw`-unwind, and `tail_call_vm_closure` must walk `open_upvalues` and mutate each cell from `Open` to `Closed`, copying the current slot value. **Critically:** `tail_call_vm_closure` currently sets `open_upvalues = None` before truncating the stack — this must become "close, then replace" (see MEMORY.md "Tail call frame replacement … must close upvalues before replacing frame").
4. **Affected opcodes.** All Load/Store local variants must branch on `has_open_upvalues`:
   - `LoadLocal`, `LoadLocal0..3`, `StoreLocal`, `StoreLocal0..3` (10 ops total — already enumerated in MEMORY.md)
   - `MakeClosure` — capture path changes from "copy value" to "find-or-create open upvalue for slot"
   - `LoadUpvalue` / `StoreUpvalue` — go through the cell (open: read parent slot; closed: read cell)
5. **Cross-VM closures.** When a VM closure is wrapped as `NativeFn` for stdlib HOF interop (Decision #50), captured upvalues that are still open in the parent VM stay open. The fresh VM created by the HOF fallback reads/writes through the shared `Rc<RefCell<UpvalueCell>>`, so `set!` mutations land in the parent's slot via the open cell. This is the property that fixes C1.

Trade-offs:

- One extra branch on the hot Load/StoreLocal path (`has_open_upvalues`). MEMORY.md notes this is already considered; the inline-cache benchmarks should be the regression gate.
- `MakeClosure` becomes O(captures) with one heap allocation per *new* upvalue (deduped per slot per frame).
- Removes the dual-write fast path entirely — single source of truth simplifies reasoning.

Out of scope for this ADR: also unifies the fix for `(type (fn (x) x))` returning `:native-fn` from VM (because closures will no longer need the NativeFn-wrapping fallback in many cases) and missing `:stack-trace` in VM error maps (separate ADR).

References: MEMORY.md (Upvalue model section), `crates/sema-vm/src/resolve.rs` (Lua-style resolution, already done), `crates/sema-vm/src/vm.rs` (Load/Store sites, `tail_call_vm_closure`), `docs/limitations.md` #31.

### 56. Bytecode stack-depth verifier for .semac loading (PROPOSED)

Status: **proposed** — fixes audit bug C11 (see `docs/limitations.md` #32). Not yet implemented.

Context: the VM uses `pop_unchecked` at 90+ call sites in `crates/sema-vm/src/vm.rs`. This relies on the in-process compiler emitting stack-balanced bytecode. `.semac` files loaded via `crates/sema-vm/src/serialize.rs::validate_bytecode` are *not* verified for stack balance — only structural checks (magic, version, table bounds, jump targets). A crafted/corrupted `.semac` can cause UB in release: `set_len(usize::MAX)` after underflow, then OOB reads.

Decision: add an **abstract-interpretation pass** over every `Chunk` (main chunk + every `Function`) during `deserialize_compile_result`, before returning. The pass tracks min/max stack depth per opcode and rejects any chunk that:

- can reach an opcode while `depth < pops_required`, or
- exits a function with `depth != 1` (must leave a single return value), or
- reaches `Return` with `depth < 1`, or
- has any reachable code path leading to negative depth at any join point.

**Algorithm sketch:**

1. Build a CFG by scanning opcodes: linear flow + edges from `Jump`, `JumpIfFalse`, `JumpIfTrue`, fallthrough past conditional jumps. Block boundaries at jump targets and after `Return` / `Throw` / unconditional `Jump`.
2. Fixed-point iterate: for each basic block, track entry depth (joined as `min` from all predecessors — using a single `i64` "entry depth" since well-formed code converges). Use a worklist algorithm.
3. For each opcode, compute `pops` and `pushes` from a static table (`Op::stack_effect()` — to be added to `opcodes.rs`).
4. Reject with `SemaError::eval("bytecode validation failed: stack underflow at op N (depth D, needs P)")`.

**Static stack-effect table** (the source of truth — see `crates/sema-vm/src/opcodes.rs`):

| Op | pops | pushes | notes |
| --- | --- | --- | --- |
| `Const`, `Nil`, `True`, `False` | 0 | 1 | |
| `Pop` | 1 | 0 | |
| `Dup` | 0 | 1 | (reads but doesn't pop TOS) |
| `LoadLocal`, `LoadLocal0..3` | 0 | 1 | |
| `StoreLocal`, `StoreLocal0..3` | 1 | 0 | |
| `LoadUpvalue` | 0 | 1 | |
| `StoreUpvalue` | 1 | 0 | |
| `LoadGlobal` | 0 | 1 | |
| `StoreGlobal`, `DefineGlobal` | 1 | 0 | |
| `Jump` | 0 | 0 | unconditional; depth flows to target |
| `JumpIfFalse`, `JumpIfTrue` | 1 | 0 | pop happens before branch |
| `Call argc` | `argc + 1` | 1 | callee + args → result |
| `TailCall argc` | `argc + 1` | 0 | exits frame |
| `Return` | 1 | 0 | exits frame |
| `MakeClosure func_id n_up` | 0 | 1 | upvalue descriptors are inline operands, not stack |
| `CallNative argc` | `argc` | 1 | |
| `CallGlobal argc` | `argc` | 1 | |
| `MakeList n`, `MakeVector n` | `n` | 1 | |
| `MakeMap n_pairs`, `MakeHashMap n_pairs` | `2 * n_pairs` | 1 | |
| `Throw` | 1 | 0 | exits frame |
| `Add`, `Sub`, `Mul`, `Div`, `Eq`, `Lt`, `Gt`, `Le`, `Ge`, `AddInt`, `SubInt`, `MulInt`, `LtInt`, `EqInt` | 2 | 1 | binary |
| `Negate`, `Not` | 1 | 1 | unary |
| `Car`, `Cdr`, `Length`, `IsNull`, `IsPair`, `IsList`, `IsNumber`, `IsString`, `IsSymbol` | 1 | 1 | |
| `Cons`, `Append`, `Get`, `ContainsQ`, `Mod`, `Nth` | 2 | 1 | |

Trade-offs:

- Adds load-time CPU cost roughly proportional to bytecode size. Acceptable: `.semac` loading is rare relative to runtime opcode dispatch.
- Verifier must agree with `vm.rs` dispatch exactly. Mismatches are bugs in *either* direction; adding `Op::stack_effect()` as a single source of truth (used by both verifier and any future fuzzer) reduces drift.
- Does not catch type errors (e.g. `Add` on non-numbers) — those remain runtime checks. Only catches arithmetic-on-stack-depth violations.

Once this lands, `.semac` files from untrusted sources can be loaded safely. Until it does, see `docs/limitations.md` #32 for the trust-model caveat.

References: `crates/sema-vm/src/vm.rs::pop_unchecked` (the unsafe site), `crates/sema-vm/src/serialize.rs::validate_bytecode` (where the new pass plugs in), `crates/sema-vm/src/opcodes.rs` (canonical opcode list), `docs/limitations.md` #32.

### 57. Propagate source spans through runtime errors (PARTIALLY IMPLEMENTED — tree-walker done, VM pending)

Status update 2026-06-09: the **tree-walker side is done** — runtime errors print `--> file:line:col`, a source snippet with caret, and spanned `at name (...)` frames (span plumbing in `crates/sema-eval/src/eval.rs`). The **VM side is not**: commit `1a83c2b` propagated spans into `ChunkDebugInfo`, but CallNative/CallGlobal/binary-op error sites still return bare messages with no location. The backends also emit different message text for arithmetic type errors (e.g. `(+ 1 "a")`). Remaining work is exactly the "VM side" section below.

Tracks LIMITATIONS.md #H13. Today the **reader** has perfect span info (used in syntax-error diagnostics like `--> path:line:col`), but **eval/VM** runtime errors emit bare messages: `type error: + expected number, got string` with no location. For anything beyond a one-liner this makes debugging needlessly hard — the user sees the error but has to grep the file to find the offending call site.

Plumbing-heavy but well-localized; both backends already carry the information internally and just don't surface it on errors.

**Tree-walker side (eval).** `EvalContext` already maintains a `current_file` stack and a `call_stack` used to build the trace printed by `print_error`. What's missing is per-call-site span propagation into `NativeFn` dispatch:

- Thread the current expression's `Span` through `eval_step`'s recursive dispatch. The reader's `SpanMap` already maps each `Value` AST node to a `Span`; the evaluator just doesn't read it on the hot path.
- When `eval_step` calls a `NativeFn`, attach the call-site span to any `SemaError` raised from inside (`SemaError::with_location(file, span)` or similar). Today native fns construct `SemaError::type_error()` / `::arity()` with no location attached.
- For user lambdas, the existing `call_stack` push already captures `(name, file, span)` — keep that, but extend so that arity/destructuring failures attach the *call site* rather than the lambda's *definition site*.

**VM side.** `ChunkDebugInfo` (in `crates/sema-vm/src/debug.rs`) already stores per-instruction spans — used today to build stack traces in `crates/sema/src/main.rs::print_error`. What's missing is using those spans when a `CallNative` / `CallGlobal` / arithmetic op raises an error:

- At the `CALL_NATIVE` / `CALL_GLOBAL` dispatch sites in `vm.rs`, look up the current `pc`'s span via the frame's `Chunk::debug_info` and attach it to the returned `SemaError` before propagating.
- Same for the binary-op opcodes (`Add`, `Sub`, …) when they raise `type_error` — these are the worst offenders for "where did this happen?" pain.

**Formatting.** Reuse the existing `--> path:line:col` formatter from `sema-reader` so runtime errors look like syntax errors. Stack-frame rendering in `print_error` already does this for frames that carry `(file, span)`; the gap is at the *innermost* error site, which today only has a message.

Trade-offs:

- Spreads `Span` through every native-fn signature or every `SemaError::*` constructor. Likely cleaner to store an optional `Span` on `SemaError` itself and have the *caller* (eval/VM dispatch) attach it on the way out, rather than every native fn doing it.
- Hot path: tree-walker already reads `SpanMap` for some operations; VM already touches `debug_info` for stack-trace construction. Adding the lookup on the *error path* only is essentially free.
- Behaviour change for downstream tools that parse error strings — keep the message stable, add the location as a separate formatted line (matches what `print_error` already does for stack frames).

References: `crates/sema-eval/src/eval.rs` (eval_step, NativeFn dispatch), `crates/sema-vm/src/vm.rs` (CALL_NATIVE / CALL_GLOBAL, binary-op error sites), `crates/sema-vm/src/debug.rs::ChunkDebugInfo`, `crates/sema-core/src/error.rs` (SemaError + location plumbing), `crates/sema-reader/src/span.rs` (span formatter reused for `--> path:line:col`), `docs/limitations.md` #H13.

### 58. Thread-local writer hook for stdout capture (replaces gag::BufferRedirect) (PARTIAL — hook shipped for DAP, notebook not migrated)

Status update 2026-06-09: the hook landed in **`crates/sema-core/src/output_hook.rs`** (`set_stdout_hook`/`set_stderr_hook`/`write_stdout`/`write_stderr`) — note: in sema-core, not sema-stdlib as proposed below — and `sema-stdlib/src/io.rs` print fns route through it. Current consumer is the **DAP server** (`crates/sema-dap/src/server.rs`). The notebook still uses `gag::BufferRedirect` (`crates/sema-notebook/src/engine.rs`, `Cargo.toml`). Remaining work: `docs/plans/2026-06-09-notebook-output-hook-migration.md`.

Tracks LIMITATIONS.md #H17. The notebook engine currently captures cell stdout with `gag::BufferRedirect::stdout()` — a process-wide file-descriptor swap. This works for the common case but composes poorly:

- A `cargo test` run that exercises notebook code also redirects test-harness output.
- Concurrent evaluations (e.g. two notebook server requests, or a future parallel eval-all) race on the single global fd.
- Certain consoles / Windows / non-tty environments mishandle the dup2 dance.
- The WASM build cannot do this at all and already uses an in-process buffer via `OUTPUT.with(...)` in stdlib `io.rs`.

**Plan.** Move stdout capture out of the OS layer and into the interpreter:

- Add a thread-local writer hook in `crates/sema-stdlib/src/io.rs` — something like `thread_local! { static OUTPUT_WRITER: RefCell<Option<Box<dyn Write>>> = RefCell::new(None) }`.
- `println`, `display`, `print`, `newline`, `print-string` (anything that today writes to `stdout`) goes through this hook: if `Some`, write to the user-supplied sink; otherwise fall through to real `std::io::stdout()`.
- Notebook engine (`crates/sema-notebook/src/engine.rs`) sets the hook to a `Vec<u8>` buffer for the duration of each cell eval and reads it back. No more `gag`.
- The WASM build already does the equivalent via a separate code path; this consolidates both backends behind one mechanism.

Trade-offs:

- Captures stdout from **Sema code** only. Native functions that write directly to `std::io::stdout()` (e.g. an LLM streaming print) bypass the hook. That's actually correct — the notebook only cares about user-program output, not interpreter chatter — but documented behaviour change vs. today's "everything inside `BufferRedirect` is captured".
- Per-thread, so concurrent evals naturally isolate without locking.
- Removes the `gag` dependency from `sema-notebook`.
- Cooperates with the WASM `OUTPUT.with(...)` pattern (currently a parallel-but-divergent capture mechanism) — long term, both can use the same hook.

References: `crates/sema-stdlib/src/io.rs` (println/display/print sites, and the WASM `OUTPUT` thread-local), `crates/sema-notebook/src/engine.rs` (current `BufferRedirect` use), `docs/limitations.md` #H17.

### 59. Canonical naming refinement (Wave 4 alias migration)

Reaffirms Decision #24: new function groups are slash-namespaced (`file/`, `path/`, `regex/`, `http/`, `json/`, `string/`, …), predicates end in `?` (`null?`, `list?`, `pair?`), arrow conversions are reserved for type↔type coercions (`string->symbol`, `keyword->string`), and the small set of deeply-entrenched R7RS Scheme primitives (`string-append`, `string-length`, `string-ref`, `substring`) stays as-is.

This pass closed several gaps where a canonical slash-namespaced form was missing or the legacy name was the only spelling. The canonical-vs-legacy pairs introduced (or formalized) in this wave:

- `any?` (canonical) / `any` (legacy alias)
- `every?` / `every`
- `time/now-ms` / `time-ms`
- `map/new` / `hash-map`
- `async/forced?` / `promise-forced?`
- `route/from-tools` / `tools->routes`
- `bytevector/{make,length,u8-ref,u8-set!,copy,append,to-list,from-list}` / `make-bytevector` family
- `path/{dir,filename,extension}` (canonical) / `path/{dirname,basename,ext}` (alias)

**Alias policy:**

- Legacy names remain registered indefinitely for back-compat — no breakage of existing scripts, notebooks, or playground examples.
- New code (stdlib examples, docs, prelude, tests) should prefer the canonical form.
- Documentation lists the canonical name as primary; aliases are noted but not promoted in tutorials.
- No deprecation warnings emitted at compile or load time — revisit at the 2.0 boundary, where alias removal becomes a real option.

**Items intentionally NOT consolidated this pass:**

- `lambda` / `fn`, `defun` / `defn`, `begin` / `progn`: three spellings per concept, but each is short, idiomatic, and present in real code in the wild. Aliases stay; consolidation can be revisited at 2.0.
- `async` (special form) and `async/spawn` (native function): semantically distinct (sugar form vs. explicit callback-with-options). Both kept.
- `read-line` / `read-many` / `read-stdin`: already aliased to `io/*` at the bottom of `crates/sema-stdlib/src/io.rs::register`. Leaving as-is — the aliases there already satisfy the slash-namespace convention.

This decision is informed by the agent quality-sweep audit, which catalogued the alias gaps and motivated the canonical names chosen above. The audit's full list lives separately; this entry records only the policy outcome.

## Rehomed from docs/decisions.md (archived 2026-06-09)

The following entries were moved here from the legacy `docs/decisions.md` (now in `docs/archived/`), with factual corrections applied during the move.

### 60. NaN-boxed Value representation

Replaced the 24-byte `enum Value` with an 8-byte NaN-boxed `struct Value(u64)`. All values encoded in IEEE 754 quiet-NaN payload space.

**Encoding scheme:**

- **Floats:** stored directly as `f64` bits; canonical quiet NaN (`0x7FF8...`) used for NaN float values to avoid collision with boxed values.
- **Boxed values:** sign=1, exponent=all 1s, quiet bit=1. Bits 50-45 = TAG (6 bits, up to 64 types), bits 44-0 = PAYLOAD (45 bits).
- **Small integers:** 45-bit two's complement in the payload, range ±17.5 trillion. No heap allocation.
- **Symbols/keywords:** `Spur` (interned string key, 32 bits) directly in the payload. **Chars:** Unicode codepoint in the payload. **Booleans/nil:** tag-only (`Value::NIL`, `Value::TRUE`, `Value::FALSE`).
- **Heap types** (String, List, Vector, Map, Lambda, …): Rc pointer in the 45-bit payload (pointer >> 3, using 8-byte alignment). 23 heap-allocated tags.

**API:** Value is no longer an enum — pattern matching uses `val.view()` → `ValueView`, or accessors (`as_int()`, `as_str()`, `as_list()`, `is_nil()`); constructors are lowercase fns (`Value::int(n)`, `Value::string(s)`).

**Benchmark results at migration time (Apple M-series, release):** VM mode +8-12% (tak 9.09s→8.04s, deriv 1.99s→1.84s) from better cache locality; tree-walker −9-16% from `view()`/accessor overhead on the hot match path; RSS −5-10%. Kept despite the TW regression because the VM is the default/future path and the TW's role is shrinking (macro expansion, `--tw` compat).

**Migration scope:** ~1,800 compile errors across 34 files in 8 crates, purely mechanical. **Safety fix found during migration:** `as_bytevector()`/`as_record()` had dangling-pointer UB via `borrow_rc()` returning a reference into a stack-local `ManuallyDrop<Rc<T>>`; fixed to `borrow_ref()`.

### 61. Mini-eval removal — callback architecture

The 620-line mini-evaluator (`sema_eval_value` + hand-rolled `call_function`) that lived in `sema-stdlib/src/list.rs` (see #6, #29) was **deleted** and replaced with thread-local `eval_callback`/`call_callback` in `sema-core`, registered by `sema-eval` at interpreter init. All stdlib HOFs now call through the real evaluator.

- **Why:** the mini-eval diverged from the real evaluator (no `try/catch`, `do`, macros, modules) and blocked the bytecode-VM transition.
- **Cost:** 1BRC regressed ~960ms → ~3050ms on 1M rows; fast-path work recovered ~14% (shared `with_stdlib_ctx` EvalContext, inline NativeFn dispatch, self-evaluating fast path, deferred cloning). The remaining gap is fundamental tree-walker overhead — closed by the bytecode VM, not by reviving the mini-eval.
- Residue: a small `simple_eval` fallback survives in `sema-llm/src/builtins.rs` only.

### 62. Runtime sandbox: capability bitset, not a process sandbox

- `--sandbox` restricts dangerous natives at runtime via a `Caps` bitset (`sema-core/src/sandbox.rs`). **Nine** capability groups: `fs-read`, `fs-write`, `shell`, `network`, `env-read`, `env-write`, `process`, `llm`, `serial`.
- Sandboxed functions stay registered (discoverable, tab-completable) but return `PermissionDenied` when invoked. `register_fn_gated()` wraps closures with a `Sandbox::check()` guard at registration; unrestricted default = zero overhead.
- Presets: `--sandbox=strict` (deny shell, fs-write, network, env-write, process, llm, serial) and `--sandbox=all`. Path restriction via `--allowed-paths` / `Sandbox::with_allowed_paths`.
- Embedders: `InterpreterBuilder::with_sandbox(Sandbox::deny(...))`.
- The WASM playground uses compile-time `#[cfg]` shims instead — complementary.
- **Not a process sandbox** — in-language permission checks only; no OS-level isolation.

### 63. Package system: git + registry sources, lockfile

- `sema pkg` CLI: `init`, `add`, `install`, `update`, `remove`, `list`, plus registry commands (`search`, `info`, `publish`, `yank`, `login`).
- Two sources: **git repos** (`sema pkg add github.com/user/repo@ref` → `~/.sema/packages/`) and the **registry** (self-hostable single Rust binary in `pkg/` — SQLite/SeaORM, REST API, web UI; `DEFAULT_REGISTRY = pkg.sema-lang.com`, currently not serving — see `docs/plans/2026-06-09-pkg-registry-predeploy-hardening.md`).
- Manifest: `sema.toml` (`[package]` + `[deps]`; short names = registry, URL paths = git). Default entrypoint `package.sema`, overridable via `entrypoint`.
- **Lockfile is implemented** (`sema.lock`: exact commit SHAs + registry checksums, `--locked` enforcement in `crates/sema/src/pkg.rs`).
