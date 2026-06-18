# Architecture Overview

Sema is a Lisp with first-class LLM primitives, implemented in Rust. All code runs on a single evaluator: a [bytecode VM](./bytecode-vm.md). The runtime is single-threaded (`Rc`, not `Arc`), with deterministic destruction via reference counting instead of a garbage collector.

The entire implementation is ~125k lines of Rust spread across 12 crates, each with a clear responsibility and strict dependency ordering.

## Crate Map

```
                ┌──────────────────────────────────────┐
                │              sema                    │
                │  (binary: CLI, REPL, embedding API)  │
                └──┬─────────────────┬─────────────────┘
                   │                 │
     ┌─────────────▼──┐         ┌────▼─────┐
     │ sema-notebook  │         │ sema-eval│
     │ notebook UI +  ├────────►│trampoline│
     │ server         │         │evaluator │
     └────────────────┘         └─┬───┬──┬─┘
                                  │   │  │
                 ┌────────────────▼┐  │ ┌▼──────────────┐
                 │  sema-stdlib    │  │ │   sema-llm    │
                 │  native fns     │  │ │ LLM providers │
                 └────────┬────────┘  │ │ + embeddings  │
                          │           │ └───────┬───────┘
                          │      ┌────▼─────┐   │
                          │      │ sema-vm  │   │
                          │      │ bytecode │   │
                          │      │ VM       │   │
                          │      │(evaluator)│  │
                          │      └────┬─────┘   │
                          │           │         │
                     ┌────▼───────────▼──┐      │
                     │   sema-reader     │      │
                     │   lexer/parser    │      │
                     └────────┬──────────┘      │
                              │                 │
                     ┌────────▼───────┐         │
                     │   sema-core    │◄────────┘
                     │  Value, Env,   │
                     │  SemaError     │
                     └────────────────┘
```

**Dependency flow:** `sema-core ← sema-reader ← sema-vm ← sema-eval ← sema` — with `sema-eval` also pulling in `sema-stdlib` (to register builtins) and `sema-llm`, both of which depend only on `sema-core` (plus `sema-reader` for stdlib).

The critical constraint: **sema-stdlib and sema-llm depend on sema-core, not on sema-eval.** This avoids circular dependencies but creates a problem — both crates sometimes need to evaluate user code. They solve it via dependency inversion:

- **sema-stdlib** invokes the real evaluator via callbacks (`call_callback`/`eval_callback`) registered by `sema-eval` at startup — stored on the `EvalContext` and a shared thread-local stdlib context
- **sema-llm** uses its own eval callback for legacy reasons, plus the core callbacks

This is discussed in detail in [The Circular Dependency Problem](#the-circular-dependency-problem).

### Crate Responsibilities

| Crate           | Role                            | Key types                                                                                                                                 |
| --------------- | ------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------- |
| **sema-core**   | Shared types                    | `Value` (NaN-boxed 8-byte), `Env`, `SemaError`, string interner, `NativeFn`, `Lambda`, `Macro`, `Record`, LLM types                       |
| **sema-reader** | Parsing                         | `Lexer` (24 token types) + recursive descent `Parser` → `Value` AST + `SpanMap`                                                           |
| **sema-vm**     | Bytecode VM (the evaluator)     | `CoreExpr`, `ResolvedExpr`, `Op`, `Chunk`, `Emitter` — lowering, resolution, compilation, VM dispatch                                     |
| **sema-eval**   | Evaluation                      | Trampoline-based evaluator, special forms, module system, call stack + span table                                                         |
| **sema-stdlib** | Standard library                | Native functions across a comprehensive standard library                                                                                  |
| **sema-llm**    | LLM integration                 | `LlmProvider` trait, native providers (Anthropic, OpenAI, Gemini, Ollama), OpenAI-compatible shim, embedding providers, cost tracking     |
| **sema-lsp**    | Language Server              | LSP via tower-lsp: completions, hover, go-to-definition, references, rename, semantic tokens, diagnostics                                  |
| **sema-dap**    | Debug Adapter                | DAP server: breakpoints, stepping, stack traces, variable inspection via VM debug hooks                                                    |
| **sema-fmt**    | Formatter                       | Code formatter for `.sema` files (`sema fmt`)                                                                                             |
| **sema-notebook** | Notebook interface       | `.sema-nb` JSON format, evaluation engine, HTTP server with REST API, embedded browser UI, Markdown export                                 |
| **sema-wasm**  | WASM bindings             | Browser playground bindings, JS interop via `wasm-bindgen`                                                                                |
| **sema**        | Binary                          | clap CLI, reedline REPL (highlighter / hinter / inspector live in `crates/sema/src/repl/`), `InterpreterBuilder` embedding API             |

## The Value Type

All Sema data is represented by a single NaN-boxed `Value` — an 8-byte `struct Value(u64)` that encodes every type in IEEE 754 quiet NaN payload space:

```rust
// crates/sema-core/src/value.rs
#[repr(transparent)]
pub struct Value(u64);

// Encoding: floats stored as raw f64 bits.
// All other types packed into quiet NaN payloads:
//   sign=1 | exponent=0x7FF | quiet=1 | TAG(6 bits) | PAYLOAD(45 bits)
//
// Immediate types (no heap allocation):
//   Nil, Bool, Char, Symbol(Spur), Keyword(Spur), IntSmall(±2^44)
//
// Heap types (Rc pointer in 45-bit payload):
//   IntBig, String, List, Vector, Map, HashMap, Lambda, Macro, NativeFn,
//   Prompt, Message, Conversation, ToolDef, Agent, Thunk, Record, Bytevector,
//   MultiMethod, Stream, F64Array, I64Array, AsyncPromise, Channel
//
// Pattern matching via val.view() → ValueView enum
```

::: details The IBM 704 connection (1955)
The idea of packing type information and data into a single machine word goes back to the [IBM 704](http://bitsavers.informatik.uni-stuttgart.de/pdf/ibm/704/24-6661-2_704_Manual_1955.pdf) — the machine Lisp was born on. The 704's 36-bit word was divided into sub-fields: a 3-bit **prefix** (opcode), a 15-bit **decrement**, a 3-bit **tag** (register selector), and a 15-bit **address**. The same word could be an instruction, a fixed-point number, a floating-point number, or six BCD characters — depending entirely on context. Sema's NaN-boxing is the same fundamental idea scaled to 64 bits: 6 tag bits + 45 payload bits, where the tag determines how to interpret the payload. The 704 also pioneered the biased-exponent floating-point format (sign + 8-bit characteristic biased by +128 + 27-bit fraction) that would eventually become IEEE 754 thirty years later — the very standard whose NaN space we now exploit for type tagging.
:::

Several design choices here are worth examining.

### Why `Rc`, Not `Arc`

Sema is single-threaded. `Arc` adds an atomic increment/decrement on every clone/drop — unnecessary overhead when there's no cross-thread sharing. `Rc` uses ordinary (non-atomic) reference counting, which is cheaper and also means the compiler can catch accidental `Send`/`Sync` usage at compile time.

The trade-off versus a tracing garbage collector: reference counting gives deterministic destruction (values are freed the instant their last reference drops), but cannot collect cycles. In practice this is rarely a problem — Lisp closures tend to create tree-shaped reference graphs, not cycles. A lambda captures its enclosing environment, which may capture its own enclosing environment, forming a chain. Cycles are theoretically possible (e.g., named lambdas bind themselves in their own environment, and `Thunk` uses `RefCell` which could close over itself), but they don't arise in typical Sema programs. If they did, the leaked memory would be bounded by the closure's captured environment — not a growing leak.

Sema uses NaN-boxing — encoding values in the unused bits of IEEE 754 NaN representations to fit a tagged value in 8 bytes, the same technique used by Janet. This makes `Value` the same size as a `f64` or a pointer, meaning the value stack and constant pool have excellent cache locality. Heap types like `List`, `Map`, and `Lambda` add one level of `Rc` pointer indirection, with the pointer stored in the 45-bit payload field (using the 8-byte alignment guarantee to shift the pointer right by 3 bits). Small integers (±17.5 trillion), symbols, keywords, characters, booleans, and nil are all stored entirely within the 8-byte NaN-box with zero heap allocation.

### Why Vector-Backed Lists

`Value::List(Rc<Vec<Value>>)` stores list elements in a contiguous `Vec`, not a linked list of cons cells. This is a deliberate departure from traditional Lisp.

::: details Why `car` and `cdr` have those names
McCarthy's original Lisp (1958) ran on the [IBM 704](http://bitsavers.informatik.uni-stuttgart.de/pdf/ibm/704/24-6661-2_704_Manual_1955.pdf), which packed cons cells into a single 36-bit machine word. The **address** field (bits 21-35) held a pointer to the first element; the **decrement** field (bits 3-17) held a pointer to the rest of the list. The 704 had hardware instructions to extract these fields directly — `car` is literally "Contents of the Address Register" and `cdr` is "Contents of the Decrement Register." They were single machine instructions, not function calls. Sema keeps the names for Scheme compatibility but the implementation is completely different — `car` is a Vec index (`v[0]`) and `cdr` is a slice copy (`v[1..]`).
:::

The performance trade-offs:

| Operation                      | Vec-backed     | Cons cells      |
| ------------------------------ | -------------- | --------------- |
| Random access (`nth`)          | O(1)           | O(n)            |
| `length`                       | O(1)           | O(n)            |
| Cache locality                 | Contiguous     | Pointer-chasing |
| `cons` (prepend)               | O(n) copy      | O(1)            |
| `append`                       | O(n) copy      | O(n)            |
| Pattern matching (`car`/`cdr`) | Slice indexing | Natural         |

The performance win comes from cache locality — modern CPUs prefetch sequential memory, so iterating a `Vec` is dramatically faster than chasing pointers through a cons list. Random access and length are constant-time bonuses.

The cost is O(n) `cons` and `append`. Sema mitigates this with copy-on-write optimization (see [Performance Internals](./performance.md#_1-copy-on-write-map-mutation)): when the `Rc` refcount is 1, mutations happen in place instead of copying. In practice, most list construction uses `list`, `map`, `filter`, or `fold` — which build a new `Vec` directly — rather than repeated `cons`.

Clojure takes a third approach: persistent vectors backed by wide (32-way branching) array-mapped tries, giving effectively O(1) indexed access (O(log₃₂ n), which is ≤ 7 for any practical size) with structural sharing. Sema's approach is simpler and faster for small to medium lists, at the cost of no structural sharing.

### Why `BTreeMap` for Maps, `hashbrown` Opt-In

`Value::Map` uses `BTreeMap` (sorted, deterministic iteration order) rather than `HashMap`. This matters for:

- **Deterministic equality:** Two maps with the same entries compare identically via `PartialEq`, and iteration order is independent of insertion order — important for consistent hashing and display
- **Printing:** `{:a 1 :b 2}` always prints in the same order, making test assertions reliable
- **Usable as keys:** Maps can be keys in other `BTreeMap`s because `Value` implements `Ord`. Since `Map` variants compare by sorted content, two maps with the same entries are always equal under `Ord`, regardless of construction order

For performance-critical code, `Value::HashMap` wraps `hashbrown::HashMap` (the SwissTable implementation used inside Rust's standard library). It's opt-in via `(hashmap/new)` — see the [Performance Internals](./performance.md#_5-hashbrown-hashmap) for benchmarks.

### Why `Spur` for Symbols and Keywords

`Symbol(Spur)` and `Keyword(Spur)` store interned `u32` handles rather than strings. A thread-local `lasso::Rodeo` interner maps strings to `Spur` values and back:

```rust
thread_local! {
    static INTERNER: RefCell<Rodeo> = RefCell::new(Rodeo::default());
}

pub fn intern(s: &str) -> Spur {
    INTERNER.with(|r| r.borrow_mut().get_or_intern(s))
}

pub fn with_resolved<F, R>(spur: Spur, f: F) -> R
where
    F: FnOnce(&str) -> R,
{
    INTERNER.with(|r| {
        let interner = r.borrow();
        f(interner.resolve(&spur))
    })
}
```

This makes symbol equality O(1) (integer comparison instead of string comparison) and environment lookup faster (integer keys in the env's hash map). It also means special form dispatch — the hottest path in the evaluator — compares `u32` values against pre-cached constants rather than resolving strings.

String interning is as old as Lisp itself. McCarthy's original LISP 1.5 (1962) interned atoms in the "object list" (oblist). The key difference: Sema uses a separate interner rather than pointer identity, so interning is explicit via `intern()` rather than implicit.

### LLM Types as First-Class Values

`Prompt`, `Message`, `Conversation`, `ToolDef`, and `Agent` sit in the `Value` type at the same level as `List` and `Map`. They're not encoded as maps-with-conventions — they're distinct types with their own constructors, pattern matching, and display representations:

```sema
;; These are values, not strings or maps
(define msg (message :user "Hello"))    ; => <message user "Hello">
(define p (prompt msg))                 ; => <prompt 1 messages>
(define conv (conversation p :model "claude-sonnet-4-6")) ; => <conversation 1 messages>
```

This means the type system catches errors like passing a string where a message is expected, and tools like `complete` can dispatch on the actual type rather than checking for the presence of magic keys in a map.

## Environment Model

The environment is a linked list of scopes, each holding a `SpurMap<Spur, Value>` (a `hashbrown::HashMap`):

```rust
pub struct Env {
    pub bindings: Rc<RefCell<SpurMap<Spur, Value>>>,
    pub parent: Option<Rc<Env>>,
    pub version: Cell<u64>,
}
```

The `version` counter is bumped on every mutation; the bytecode VM's per-instruction inline caches use it to detect stale global lookups. Variable lookup walks the parent chain until it finds a binding or reaches the root. This is the standard lexical scoping model — a closure captures a reference to its defining environment, and lookups resolve outward through enclosing scopes.

### Operations

| Operation                 | Behavior                                      | Used by                     |
| ------------------------- | --------------------------------------------- | --------------------------- |
| `get(spur)`               | Walk parent chain, return first match         | Variable lookup             |
| `set(spur, val)`          | Insert in current scope                       | `define`, parameter binding |
| `set_existing(spur, val)` | Walk chain, update where found                | `set!` (mutation)           |
| `update(spur, val)`       | Overwrite in current scope                    | Hot-path env reuse          |
| `take(spur)`              | Remove from current scope, return value       | COW optimization            |
| `take_anywhere(spur)`     | Remove from any scope in chain                | COW optimization            |

`take` and `take_anywhere` exist for the copy-on-write optimization: by _removing_ a value from the environment before passing it to a function, the `Rc` refcount drops to 1, enabling in-place mutation. See [Performance Internals](./performance.md#_1-copy-on-write-map-mutation).

`update` exists for the lambda environment reuse optimization: when reusing an environment across iterations of a hot loop, `update` overwrites an existing binding in place instead of going through the full insert path. See [Performance Internals](./performance.md#_2-lambda-environment-reuse).

## Error Handling

`SemaError` is a `thiserror`-derived enum with 12 variants including `WithTrace` and `WithContext` wrappers:

```rust
#[derive(Debug, Clone, thiserror::Error)]
pub enum SemaError {
    Reader { message: String, span: Span },
    Eval(String),
    Type { expected: String, got: String, got_value: Option<String> },
    Arity { name: String, expected: String, got: usize },
    Unbound(String),
    Llm(String),
    Io(String),
    PermissionDenied { function: String, capability: String },
    PathDenied { function: String, path: String },
    UserException(Value),

    WithTrace { inner: Box<SemaError>, trace: StackTrace },
    WithContext { inner: Box<SemaError>, ... },
}
```

### Constructor Helpers

Errors are created via constructor methods, never raw enum variants:

```rust
SemaError::eval("division by zero")
SemaError::type_error("int", val.type_name())
SemaError::arity("map", "2", args.len())
```

This keeps error construction concise across all native functions and special forms.

### Lazy Stack Traces

Stack traces are not captured at error creation time. Instead, the `WithTrace` wrapper is attached during error _propagation_ — when the trampoline loop unwinds through a function call, it wraps the error with the current call stack:

```rust
pub fn with_stack_trace(self, trace: StackTrace) -> Self {
    if trace.0.is_empty() {
        return self;
    }
    match self {
        SemaError::WithTrace { .. } => self,  // already wrapped, don't double-wrap
        SemaError::WithContext { inner, hint, note } => SemaError::WithContext {
            inner: Box::new(inner.with_stack_trace(trace)),  // wrap inside the context
            hint,
            note,
        },
        other => SemaError::WithTrace {
            inner: Box::new(other),
            trace,
        },
    }
}
```

This avoids the cost of capturing a stack trace for errors that are caught by `try`/`catch` — only errors that propagate to the top level pay the trace cost. The idempotence check (`WithTrace { .. } => self`) prevents double-wrapping when an error passes through multiple call frames.

## Interpreter State

Sema's evaluator state is held in an explicit `EvalContext` struct, defined in `sema-core/src/context.rs` and threaded through the evaluator as `ctx: &EvalContext`. Each `Interpreter` instance owns its own `EvalContext`, enabling multiple independent interpreters per thread with fully isolated state.

### EvalContext Fields

| Field               | Type                                | Purpose                                      |
| ------------------- | ----------------------------------- | -------------------------------------------- |
| `module_cache`      | `RefCell<BTreeMap<PathBuf, ...>>`   | Loaded modules (path → exports)              |
| `current_file`      | `RefCell<Vec<PathBuf>>`             | Stack of file paths being executed           |
| `module_exports`    | `RefCell<Vec<Option<Vec<String>>>>` | Exports declared by currently-loading module |
| `module_load_stack` | `RefCell<Vec<PathBuf>>`             | Cycle detection during module loading        |
| `call_stack`        | `RefCell<Vec<CallFrame>>`           | Call frames for error traces                 |
| `span_table`        | `RefCell<HashMap<usize, Span>>`     | Rc pointer address → source span             |
| `eval_depth`        | `Cell<usize>`                       | Recursion depth counter                      |
| `max_eval_depth`    | `Cell<usize>`                       | High-water mark of eval depth                |
| `eval_step_limit`   | `Cell<usize>`                       | Step limit for fuzz targets                  |
| `eval_steps`        | `Cell<usize>`                       | Current step counter                         |
| `eval_deadline`     | `Cell<Option<Instant>>`             | Wall-clock budget (used by the notebook)     |
| `sandbox`           | `Sandbox`                           | Capability sandbox                           |
| `user_context` / `hidden_context` | `RefCell<Vec<BTreeMap<Value, Value>>>` | Dynamic context frames        |
| `context_stacks`    | `RefCell<BTreeMap<Value, Vec<Value>>>` | Named context stacks                      |
| `eval_fn` / `call_fn` | `Cell<Option<fn(...)>>`           | Registered evaluator callbacks               |
| `interactive`       | `Cell<bool>`                        | REPL/interactive mode flag                   |

### Remaining Thread-Locals

Some state remains in thread-local storage — either because it's a pure performance cache or because it belongs to a subsystem that hasn't been refactored yet:

| Location                     | Thread-local        | Purpose                                       |
| ---------------------------- | ------------------- | --------------------------------------------- |
| `sema-core/value.rs`         | `INTERNER`          | String interner (`lasso::Rodeo`)              |
| `sema-core/context.rs`       | `STDLIB_CTX`        | Shared `EvalContext` for stdlib callbacks     |
| `sema-eval/special_forms.rs` | `SF`                | Cached `SpecialFormSpurs` (performance cache) |
| `sema-llm/builtins.rs`       | `PROVIDER_REGISTRY` | Registered LLM providers                      |
| `sema-llm/builtins.rs`       | `SESSION_USAGE`     | Cumulative token usage                        |
| `sema-llm/builtins.rs`       | `LAST_USAGE`        | Most recent completion's usage                |
| `sema-llm/builtins.rs`       | `EVAL_FN`           | Full evaluator callback                       |
| `sema-llm/builtins.rs`       | `SESSION_COST`      | Cumulative dollar cost                        |
| `sema-llm/builtins.rs`       | `BUDGET_LIMIT`      | Spending cap                                  |
| `sema-llm/builtins.rs`       | `BUDGET_SPENT`      | Spending against cap                          |
| `sema-llm/pricing.rs`        | `CUSTOM_PRICING`    | User-defined model pricing                    |

### Implications for Embedding

Multiple `Interpreter` instances can coexist on the same thread with fully isolated evaluator state — each has its own module cache, call stack, span table, and depth counters. The string interner (`INTERNER`) remains shared per-thread, which is correct since `Spur` handles must be consistent within a thread. LLM state (provider registry, usage tracking, budgets) is also per-thread, meaning all interpreters on the same thread share provider configuration and cost tracking.

`Value` instances are not `Send` or `Sync` (they use `Rc`, not `Arc`), so interpreters cannot be moved across threads.

## WASM Support

Sema compiles to WebAssembly with conditional compilation gates. The `#[cfg(not(target_arch = "wasm32"))]` attribute excludes modules that depend on OS-level capabilities:

**From sema-stdlib:**

- `io` — file system access (`file/read`, `file/write`, `file/fold-lines`, etc.)
- `system` — process execution, environment variables, exit
- `http` — HTTP client (`http/get`, `http/post`, etc.)
- `terminal` — terminal control (colors, cursor, raw mode)
- `kv`, `pdf`, `serial`, `server`, `sqlite` — other OS-dependent modules

**From sema-eval:**

- Module `import`/`load` (depends on file system)

**sema-llm** is excluded entirely — LLM providers require network access.

The pure-computation core (arithmetic, strings, lists, maps, JSON, regex, crypto, datetime, CSV, bytevectors, predicates, math, comparison, bitwise, meta) remains available in WASM, making Sema usable as an embedded scripting language in browser-based applications.

## The LLM Subsystem

### Provider Trait

All LLM providers implement a single trait:

```rust
pub trait LlmProvider: Send + Sync {
    fn name(&self) -> &str;
    fn complete(&self, request: ChatRequest) -> Result<ChatResponse, LlmError>;
    fn default_model(&self) -> &str;

    // Optional — defaults provided
    fn stream_complete(&self, request: ChatRequest,
        on_chunk: &mut dyn FnMut(&str) -> Result<(), LlmError>,
    ) -> Result<ChatResponse, LlmError> { /* non-streaming fallback */ }

    fn batch_complete(&self, requests: Vec<ChatRequest>)
        -> Vec<Result<ChatResponse, LlmError>> { /* sequential fallback */ }

    fn embed(&self, request: EmbedRequest)
        -> Result<EmbedResponse, LlmError> { /* unsupported error */ }
}
```

Note the `Send + Sync` bound — despite the single-threaded runtime, provider implementations use `tokio::runtime::Runtime::block_on` internally to run async HTTP clients. The trait itself is synchronous; async is hidden behind the provider boundary.

### Provider Registry

The `ProviderRegistry` holds registered providers by name with a default provider slot and a separate embedding provider slot:

```rust
pub struct ProviderRegistry {
    providers: HashMap<String, Box<dyn LlmProvider>>,
    default: Option<String>,
    embedding_provider: Option<String>,
}
```

At startup, the binary crate detects available API keys and registers providers:

- `ANTHROPIC_API_KEY` → Anthropic (Claude)
- `OPENAI_API_KEY` → OpenAI (GPT)
- `GOOGLE_API_KEY` → Gemini
- `GROQ_API_KEY`, `XAI_API_KEY`, `MISTRAL_API_KEY`, `MOONSHOT_API_KEY` → OpenAI-compatible shim
- Ollama (local) is always registered — no auth needed; `OLLAMA_HOST` overrides the default `http://localhost:11434`

Embedding providers (Jina, Voyage, Cohere) are registered separately and selected via `(llm/set-embedding-provider)`.

### Cost Tracking

Every completion records token usage in `SESSION_USAGE` and computes dollar cost via a built-in pricing table (`pricing.rs`). The `llm/with-budget` function sets a scoped spending cap:

```sema
(llm/with-budget {:max-cost-usd 0.50 :max-tokens 10000} (lambda ()
  (llm/complete "Summarize this document...")))
;; Raises an error if cumulative cost exceeds $0.50 or 10000 tokens
```

## The Circular Dependency Problem

The most architecturally significant constraint in Sema is the dependency direction between the evaluator and the library crates.

### The Problem

Both `sema-stdlib` and `sema-llm` sometimes need to evaluate user code:

- **sema-stdlib:** `file/fold-lines` invokes a user-provided lambda on each line. `map`, `filter`, `fold`, `for-each`, `sort` all take lambda arguments.
- **sema-llm:** Tool handlers defined via `deftool` are Sema expressions that must be evaluated when an LLM invokes the tool.

But `sema-stdlib` and `sema-llm` depend on `sema-core`, not `sema-eval`. They can't call `eval_value()` because that function lives in `sema-eval`, which is upstream.

```
sema-eval ──depends-on──► sema-core
sema-stdlib ──depends-on──► sema-core
sema-stdlib ──CANNOT depend on──► sema-eval  (would create a cycle)
```

### Solution 1: Callback Architecture (sema-core + sema-stdlib)

`sema-core` defines callback storage in `context.rs` that bridges the dependency gap using dependency inversion — function-pointer slots on `EvalContext`, plus a shared thread-local context for stdlib functions that don't receive a `ctx` parameter:

```rust
pub type EvalCallbackFn = fn(&EvalContext, &Value, &Env) -> Result<Value, SemaError>;
pub type CallCallbackFn = fn(&EvalContext, &Value, &[Value]) -> Result<Value, SemaError>;

pub struct EvalContext {
    // ...
    pub eval_fn: Cell<Option<EvalCallbackFn>>,
    pub call_fn: Cell<Option<CallCallbackFn>>,
}

thread_local! {
    static STDLIB_CTX: EvalContext = EvalContext::new();
}
```

At startup, `sema-eval` registers the real evaluator and call dispatch functions (into both the interpreter's context and the shared `STDLIB_CTX`):

```rust
sema_core::set_eval_callback(&ctx, eval_value);
sema_core::set_call_callback(&ctx, call_value);
```

All stdlib higher-order functions (`map`, `filter`, `fold`, `sort-by`, `for-each`, `file/fold-lines`, etc.) invoke user-provided lambdas through `sema_core::call_callback`, which dispatches to the real evaluator:

```rust
// In sema-stdlib, e.g. map implementation
let result = sema_core::call_callback(ctx, &func, &[elem])?;
```

The `with_stdlib_ctx` function provides a shared `EvalContext` for stdlib callbacks, avoiding per-call allocation of a new context.

This is a clean dependency inversion — `sema-stdlib` depends only on the callback signature defined in `sema-core`, not on `sema-eval`. The runtime cost is one `Cell::get()` + function pointer dispatch per call, which is negligible. Unlike the previous mini-evaluator approach, this architecture uses the _same_ evaluator everywhere — all special forms, builtins, and features are available inside higher-order functions like `map` and `file/fold-lines`.

### Solution 2: Eval Callback (sema-llm)

`sema-llm` predates the core callback architecture and maintains its own eval callback in a thread-local. It stores a function pointer that bridges the dependency gap:

```rust
pub type EvalCallback = Box<dyn Fn(&EvalContext, &Value, &Env) -> Result<Value, SemaError>>;

thread_local! {
    static EVAL_FN: RefCell<Option<EvalCallback>> = RefCell::new(None);
}

pub fn set_eval_callback(f: impl Fn(&EvalContext, &Value, &Env) -> Result<Value, SemaError> + 'static) {
    EVAL_FN.with(|eval| {
        *eval.borrow_mut() = Some(Box::new(f));
    });
}
```

At startup, the binary crate registers the full evaluator:

```rust
sema_llm::builtins::set_eval_callback(sema_eval::eval_value);
```

When a tool handler needs to evaluate Sema code, it calls through this indirection:

```rust
fn full_eval(ctx: &EvalContext, expr: &Value, env: &Env) -> Result<Value, SemaError> {
    EVAL_FN.with(|eval_fn| {
        let eval_fn = eval_fn.borrow();
        match &*eval_fn {
            Some(f) => f(ctx, expr, env),
            None => simple_eval(expr, env),  // fallback if no callback registered
        }
    })
}
```

This is the classic dependency inversion pattern — `sema-llm` depends on an _interface_ (the callback signature) rather than the concrete implementation. The runtime cost is one `RefCell::borrow()` + dynamic dispatch per eval call, which is negligible for LLM tool invocations (the LLM API call dominates by orders of magnitude).

### Why Not a Trait?

An alternative would be to define an `Evaluator` trait in `sema-core` and have `sema-eval` implement it. This would work but adds complexity for little benefit — the callback is simpler, there's only one implementation, and it avoids threading a trait object through every function that might need evaluation. The callback approach also makes it easy to test `sema-llm` in isolation (register a mock evaluator).

### Architectural Lesson

The circular dependency constraint forced a callback architecture that turned out to be a better design than having direct access to the evaluator would have been. The dependency inversion through `sema-core` callbacks gives a single, canonical evaluator used everywhere — stdlib HOFs, LLM tool handlers, and the main interpreter all run the same code paths with full feature support. This also provides a clean seam for future work: when the bytecode VM became the default backend, only the callback registrations needed to change — all call sites in stdlib and llm remained untouched, validating this design. Sometimes constraints lead to better designs than unconstrained freedom would have.
