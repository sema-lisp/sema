# Glossary

This page defines the technical vocabulary used across Sema's documentation — from Lisp fundamentals and VM internals to the LLM, observability, and tooling layers. Many words are overloaded (the same word means different things in different subsystems); those entries enumerate each meaning explicitly so you can tell them apart.

## Lisp & Language Core

**Arity** — the number of arguments a function expects. Calling with the wrong count raises an arity error (error `:type` `:arity`), e.g. `f expects 1 args, got 3`.

**Association list (alist)** — a list of pairs used as a simple key-value mapping, queried with `assoc` (uses `equal?`), `assq` (uses `eq?`, pointer/symbol equality), or `assv` (uses `eqv?`, numeric value). Each lookup returns the matching pair or `#f`. Distinct from the `map`/`hashmap` data types, and the alist `assoc` is a different function from the map `assoc` that adds a key.

**Atom** — a single, non-list Sema value such as a number, string, symbol, or keyword, as opposed to a list of expressions. Note: in Clojure-family Lisps "atom" also means a mutable reference cell — Sema has no such type; use `define` + `set!` for mutable state (see *Mutable state* under Concurrency).

**Begin / progn** — a sequencing form that evaluates its expressions in order and returns the last result. `progn` is an accepted Common Lisp alias.

**Binding** — (1) *value binding*: an association of a name to a value (`define` for globals; `let`/`let*`/`letrec` for locals; `set!` mutates an existing one; modules expose *exported bindings*). (2) *binding form*: a syntactic construct that introduces bindings (`let`, `when-let`, `if-let`).

**Car / cdr** — classic Lisp accessors: `car` (alias `first`) returns the first element of a list, `cdr` (alias `rest`) returns the remainder. Names derive from IBM 704 hardware registers (Contents of the Address/Decrement Register). Compositions like `cadr`, `caddr` chain them.

**Closure** — a function paired with the variables it captures from its enclosing lexical scope, retaining access to them even after the defining function returns. See also the VM-level implementation under *Closure* in Reader, Compiler & VM Internals.

**Cons pair** — the two-field cell ("cons") from which lists are built: `car` holds the head, `cdr` holds the tail. `cons` prepends an element to a list. See also *Dotted pair*.

**Delay / force** — `delay` creates a promise that defers evaluation of its expression; `force` evaluates it and memoizes the result (non-promise values pass through unchanged). Classic Scheme lazy evaluation; `promise-forced?` tests whether it has been forced. See *Promise*.

**Destructuring** — binding-position patterns that pull apart a value into named variables: positional list/vector patterns (`[a b c]`, with `&` for rest), nested patterns, and map patterns (`{:keys [name age]}`). Works in `let`, `let*`, `define`, lambda params, and `match`. `_` is a wildcard.

**Do loop** — a Scheme `do` iteration form with variable bindings, per-iteration step expressions, a termination test, and an optional body, e.g. `(do ((i 0 (+ i 1))) ((= i 10) result) body)`. Relies on tail-call optimization.

**Dotted pair** — Scheme cons-cell notation `(a . b)`. Because Sema lists are `Vec`-backed (not linked cons cells), the parser represents the dot as a marker symbol `"."` inserted into the element list (`(a . b)` parses as `[a, ".", b]`) — a pragmatic escape hatch for improper lists and Scheme compatibility.

**Equality** — Sema's equality family: `=` is numeric equality (`(= 1 1.0)` is true); `eq?`/`equal?` test structural equality and are aliases. Alist lookups use `assq` (`eq?`, pointer/symbol) and `assv` (`eqv?`, numeric value), so the `eq?`/`eqv?` distinction is real there. Records compare by type plus pairwise `equal?`.

**Form** — (1) *expression*: a single Sema expression considered as a unit of code — a function call, special form, or literal (`io/read-many` parses a string of multiple forms). (2) *special form*: see *Special form*. (3) *formatter category*: the formatter's notion of "body forms", "binding forms", "clause forms" — syntactic categories that drive indentation.

**Gensym** — a function generating a guaranteed-unique symbol (e.g. `tmp__42`), used to avoid variable capture in macros. Auto-gensym (`foo#`) is the ergonomic form, with its uniqueness magic active only inside a quasiquote template.

**Guard** — an extra boolean condition attached to a `match` clause via `when`, so the clause fires only when both the pattern matches and the guard is truthy, e.g. `(x when (> x 100) "big")`.

**Homoiconicity** — the property that code is represented in the language's own data structures (a Sema program is just an ordinary `Value`). Underpins the reader producing `Value` directly (no separate AST), macros operating on `Value` AST, and the grammar fuzzer's near-free round-trip/value oracles. Also called "code is data".

**Hygiene / variable capture** — variable capture is the bug where a binding a macro introduces accidentally shadows a user's same-named variable. Auto-gensym (`foo#`) and `gensym` produce unique symbols to prevent it, giving hygienic macros.

**IEEE 754 float policy** — Sema's numeric rule split by type: integer division/modulo by zero raises an error, while floating point follows IEEE 754 (overflow and undefined results yield `inf`, `-inf`, or `NaN`). Integer overflow wraps two's-complement (no bignums). `math/nan?`/`math/infinite?` test the special floats; JSON/TOML cannot encode `NaN`/`Infinity`.

**Keyword** — a colon-prefixed, self-evaluating identifier (e.g. `:name`, `:ok`), commonly a map key. Keywords double as accessor functions: `(:name m)` is equivalent to `(get m :name)`. Clojure-style; interned as a `Spur`. Also used as error `:type` tags and as the result of `(type x)`.

**Keyword-as-function** — the convenience where a keyword in head position acts as a getter: `(:name person)` works like `(get person :name)`. Handled in the evaluator when a `Value::Keyword` appears as the call head.

**Lambda** — a special form creating an anonymous function, e.g. `(lambda (x y) (+ x y))`. `fn` is an alias; variadic params use dot notation (`x . rest`). `defun`/`defn` are sugar over `lambda`. See also *Short lambda*.

**Let / let\* / letrec** — local-binding special forms: `let` binds in parallel (all inits evaluated before any binding), `let*` binds sequentially (each visible to later inits), `letrec` makes all bindings visible to all inits (for mutual recursion). See also *Named let*.

**Lexical scope** — Sema's scoping rule: a function accesses variables from the textually enclosing scopes where it was defined, not from where it is called. The basis of closures.

**List** — the fundamental Sema data structure: a parenthesized, ordered sequence with `car`/`first` as head and `cdr`/`rest` as tail, created via `list` or quoting. Conceptually a cons list (queried as alists with `assoc`/`assq`/`assv`), but represented internally as a `Vec` — see *Vector-backed list* for the performance trade-offs. Contrast with `vector` (bracketed, O(1) indexed).

**Macro** — a `defmacro`-defined transformer that rewrites code at expansion time (before evaluation), typically built with quasiquote/unquote. Some (threading, `when-let`) are auto-loaded built-ins. Contrast with a special form (built into the evaluator, not user-definable). See also *Macro expansion*.

**Match** — a pattern-matching special form testing a value against patterns (literals, binding symbols, vector/map structures) with optional `when` guards. `match` raises an error if no clause matches; `match*` returns `nil` instead. Add a catch-all `(_ ...)` for exhaustiveness.

**Module system** — Sema's `import`/`load` mechanism (in `sema-eval`): modules are identified by canonical file path, cached in `EvalContext.module_cache`, and a module's env is a child of the root env (it gets builtins, not caller bindings). Meanings of "module": (1) a source module via `(module ...)` with selective `export`; (2) preloaded *virtual modules* injected into the cache by host code (`preload_module`); (3) packages, whose entrypoint file is loaded on import. Architecture docs also use "module" loosely for stdlib sub-modules (io, http) and Rust modules. Contrast with `load`, which does not use the module system.

**Multimethod** — Clojure-style polymorphic dispatch: `defmulti` declares a method with a dispatch function applied to the arguments; `defmethod` registers an implementation for a specific dispatch value (`:default` for fallback).

**Named let** — a `let` with a loop name that creates a local recursive function used as a tail-call-optimized loop, e.g. `(let loop ((i 0)) ...)`. Standard Scheme idiom.

**Nil** — the empty/null value, returned by `when`/`while`/`unless` on a failed condition, by `some->` on a nil step, by `match*` on no match, and by `channel/recv` on a closed empty channel. `null?`/`nil?` test for it; distinct from `#f` though both are non-truthy.

**Pair** — `(pair? x)` is `#t` for a non-empty list (a Scheme-compatibility predicate); the underlying cons-cell pair holds a head (`car`) and tail (`cdr`). See *Cons pair*.

**Predicate** — a function (conventionally `?`-suffixed) returning a boolean, e.g. `null?`, `list?`, `even?`, `agent?`. The docs separate overlapping ones precisely: `null?` (empty list OR nil), `nil?` (only nil), `empty?` (any empty collection/string/nil).

**Prefix notation** — the convention where the operator or function comes first in a list, followed by its arguments, e.g. `(+ 1 2)` instead of `1 + 2`. Also called Polish notation.

**Quasiquote** — a templating form (backtick `` ` ``) returning a structure mostly unevaluated but allowing selective evaluation via unquote and unquote-splicing. Essential for macros; auto-gensym (`foo#`) only has its uniqueness magic inside a quasiquote template.

**Quote** — a special form returning its argument unevaluated, turning code into data; reader shorthand `'x` desugars to `(quote x)`. `'(+ 1 2)` yields the list, `'foo` the symbol.

**Recursion** — a function calling itself (or mutually) for repetitive work — the standard looping mechanism. Tail recursion enables TCO (see *Tail-call optimization*); infinite recursion triggers a max-eval-depth error.

**S-expression** — the uniform parenthesized syntax for both code and data: an expression is either a single value (atom) or a parenthesized list of expressions. Foundational Lisp concept; Sema's pitch is that even LLM prompts are ordinary s-expression data. Also "sexp", "symbolic expression".

**Special form** — a construct built into the evaluator that controls evaluation order and cannot be redefined (`define`, `if`, `quote`, `lambda`, `let`, `cond`, `try`, `import`, `async`, …). Unlike functions, special forms may evaluate their arguments selectively or not at all. Sema has ~40 surface special forms; dispatch compares pre-cached `Spur` constants. Some that can't compile to pure bytecode are delegated to `__vm-*` globals (see *Runtime-delegated form*).

**Symbol** — a bare identifier used as a variable name and as quoted data, e.g. `foo`, `my-var`, `+`. Symbols evaluate to their bound value unless quoted; `(type 'foo)` is `:symbol`. Interned to a `Spur`; `gensym`/auto-gensym produce fresh ones.

**Thunk** — a zero-argument function used to defer execution. It is the unit of scoped behavior for `with-*` combinators (`llm/with-cache`, `llm/with-budget`, `llm/with-fallback`, `retry`, `context/with`), the body of an async task (`async/spawn`), and the wrapped body of a lazy `delay`. In notebooks, thunks are opaque values that cannot be round-tripped and must be re-evaluated on reload.

**Threading macro** — pipeline macros that thread a value through a sequence of forms: `->` (thread-first, inserts as first arg), `->>` (thread-last), `as->` (bind to a name for arbitrary placement), `some->` (nil-safe thread-first that short-circuits on nil). Auto-loaded; the formatter indents each step.

**Truthiness** — the rule determining which values count as true in conditionals. `and` returns the last truthy value or `#f`; `or` returns the first truthy value; `while`/`when` loop/run on truthy conditions. Only `#f` (and `nil`) are non-truthy.

**Try / catch / throw** — error-handling forms: `try` evaluates a body, `catch` binds any raised error (a structured map with `:type`, `:message`, and `:value` for user-thrown values; plus `:stack-trace` — a list of frame maps) for handling, and `throw` raises any value. `catch` catches ALL error types (including internal `:unbound`, `:arity`, `:permission-denied`), so re-throw what you don't handle.

**Unquote / unquote-splicing** — inside a quasiquote, unquote (`,expr`) evaluates `expr` and inserts its value; unquote-splicing (`,@expr`) evaluates a list and splices each element into the template. E.g. `` `(a ,@(list 1 2 3) b) `` yields `(a 1 2 3 b)`.

**Variadic / rest parameters** — functions accepting a variable number of arguments, captured via dot notation (`x . rest`) in lambda params or `&` in destructuring patterns.

## Reader, Compiler & VM Internals

**.semac** — Sema's compiled bytecode file format: a 24-byte header (magic `\x00SEM` + format version + flags + Sema version) followed by length-prefixed sections (string table, function table, main chunk, optional debug sections). Versioned (currently 4); the loader requires an exact version match. Produced by `sema compile`, consumed by run/disasm/build. A build artifact tied to the producing Sema version, not a portable interchange format. Auto-detected via the null-byte magic.

**AST** — abstract syntax tree. Sema has no dedicated AST type: the parser produces ordinary `Value` nodes, so the same `Value` type that exists at runtime represents parsed code (the "code is data" tradition). `sema ast` prints it; macros expand `Value` AST into more `Value` AST, which lowering converts into `CoreExpr`.

**Bytecode VM** — Sema's stack-based bytecode virtual machine, the sole evaluator and default backend (since v1.13). Source compiles to bytecode through four passes (Lower → Optimize → Resolve → Compile) and runs on the VM; `.semac` files store the compiled bytecode.

**Call frame** — (1) *VM CallFrame*: the per-call VM record holding the active closure, program counter, stack-base offset, open-upvalue cells, and cache base; pushed on call, reused on tail call, popped on return. (2) *sema_core::CallFrame*: a record used in `StackTrace`s attached to errors — carries `name`, `file`, and `span`. The VM's `capture_vm_stack_trace` walks VM frames and produces these for error maps. The DAP debugger renders VM frames with names, line numbers, and source paths.

**Callback architecture** — Sema's dependency-inversion design where `sema-stdlib`/`sema-llm` (which depend on `sema-core`, not `sema-eval`) invoke the real evaluator through function-pointer callbacks (`call_callback`/`eval_callback`) registered by `sema-eval` at startup. Solves the circular-dependency problem so higher-order functions and LLM tool handlers run the single canonical evaluator. Replaced the removed *mini-eval*.

**Chunk** — the unit of compiled bytecode: raw code bytes plus its constant pool, source spans, max stack depth, local count, inline-cache slot count, and exception table. The main program and each function each compile to a `Chunk`. Not to be confused with an LLM streaming chunk (see *Chunk* under LLM & GenAI) or `list/chunk`/`text/chunk`.

**Closure** — at the VM level, a function value paired with the captured upvalues it references, created by `MakeClosure` from a compiled `Function` template plus upvalue descriptors. VM closures are wrapped as `Value::NativeFn` (carrying a `VmClosurePayload`) so non-VM code can call them: in-VM calls run in the same VM, calls from outside spin up a fresh VM (the "fallback path"). See also *Closure* under Lisp & Language Core.

**Constant pool** — the per-chunk table of literal values that `Const` opcodes index into. In `.semac` each entry is a serialized type-tag-plus-payload `SerializedValue`; runtime-only types (Lambda, NativeFn, Prompt, Channel, Agent, ToolDef, Thunk, Record) must never appear here. Nesting depth is capped at 128 (`MAX_VALUE_DEPTH`).

**Copy-on-write (COW)** — an optimization where a shared `Rc`-wrapped collection is mutated in place when its refcount is 1 (via `Rc::try_unwrap`/`Rc::make_mut`) and cloned only when actually shared, so callers never observe an aliased mutation. Used by `bytevector/set!`, typed-array `set!`, and BTreeMap updates; `Env::take` exists to drop refcounts to 1 first. ~30% of the 1BRC speedup. Sema chose COW over persistent collections.

**CoreExpr** — the desugared intermediate representation produced by lowering, with variables still represented as names. The Optimize pass runs on it before resolution; paired with `ResolvedExpr` so the compiler can only receive resolved expressions.

**Cross-compilation** — `sema build --target <triple>` producing executables for other platforms by downloading and caching a runtime binary for the target (verified against a published SHA256), then doing magic-byte-detected, format-aware injection. `libsui` does Mach-O ad-hoc signing in pure Rust, so macOS ARM64 binaries can be built from Linux. `SEMA_RUNTIME_BASE_URL` overrides the download location.

**Debug hook** — VM instrumentation points (`debug.rs`, `execute_debug`) the DAP server uses: on every instruction step the hook checks for a hit breakpoint, a completed step, or a requested pause, updating `DebugState` and notifying the frontend. Source line numbers map to bytecode instructions for breakpoint verification.

**Disassembly** — human-readable rendering of a chunk's bytecode (`disasm.rs` / `sema disasm`), showing each instruction's offset, opcode mnemonic, and operands (e.g. `0000 CONST 0 ; 3`). Exposed via the CLI (optional `--json`) and the MCP `disasm` tool.

**Dispatch loop** — the VM's central loop that reads one opcode at a time and executes it — "the literal heart of every bytecode interpreter." Sema's is a two-level loop: an outer loop caches frame-local state (code/constants/base pointers) and an inner loop dispatches without re-fetching frame data, reloading only when control flow changes frames. PGO lays out the `match op` hot blocks by measured opcode frequency.

**Emitter** — the bytecode builder (`emit.rs`) wrapped by the Compiler; it writes opcodes/operands and handles jump backpatching (filling in jump offsets once branch lengths are known).

**Env** — Sema's runtime environment: a chain of scopes, each an `Rc<RefCell<HashMap<Spur, Value>>>` plus an optional parent and a `version` counter, with lookup walking the parent chain (lexical scoping). In the real VM most variable access is resolved to integer slots/upvalues at compile time and the Env is consulted mainly for globals; the `version` counter drives inline-cache invalidation. WASM `eval` uses a non-persistent child env vs `evalGlobal`'s persistent global env; a notebook's "shared cell environment" is one persistent Env across cells.

**EvalContext** — an explicit struct (`sema-core/context.rs`) holding all per-interpreter evaluator state — module cache, call stack, span table, depth counters, sandbox, eval/call callbacks, eval deadline — threaded through evaluation as `ctx: &EvalContext`. Each `Interpreter` owns one, so multiple isolated interpreters can run on one thread. A shared thread-local `STDLIB_CTX` serves stdlib callbacks that don't receive a ctx parameter.

**Exception table** — a per-chunk table of entries (`try_start`, `try_end`, `handler_pc`, `stack_depth`, `catch_slot`) implementing `try`/`catch`. The `Throw` opcode searches it for a matching handler, restores the stack to the saved depth, pushes the error value, and jumps to the handler — no inline branching opcodes.

**F-string** — an interpolating string literal `f"...${expr}..."` that the reader desugars into a `(str "literal" expr …)` call, parsing each `${...}` interpolation recursively. `\$` suppresses interpolation. Distinct from `prompt/template`'s Mustache-style `{{key}}` slots.

**Format version** — the `.semac` binary-format version field (currently 4) in the 24-byte header; the loader requires an exact match and otherwise rejects ("Recompile from source"). Distinct from the recorded compiler version. v2 added inline-cache operands, v3 added upvalue names, v4 added `local_scopes`.

**Function table** — the required `.semac` section (0x02) of compiled function templates (name, arity, `has_rest`, upvalue descriptors and names, the function's chunk, debug metadata). `MakeClosure` references entries by `func_id`. Empty for programs with no inner lambdas; distinct from the runtime native-function table.

**Function template** — a compiled `Function` (`chunk.rs`) describing a lambda — its chunk, arity, rest flag, and upvalue descriptors — collected by the Compiler and stored in the `.semac` function table. `MakeClosure` instantiates a `Closure` from a template plus captured upvalues; one template can produce many closures.

**Fused CallGlobal** — an opcode combining `LoadGlobal` + `Call` into one instruction for non-tail calls to global functions, carrying `(u32 spur, u16 argc, u16 cache_slot)` operands and inline-cached via `cache_slot`; sets up the frame without the function value on the stack.

**Inline cache** — a per-instruction cache for global-variable lookups: each `LoadGlobal`/`CallGlobal` carries a `u16` cache-slot operand indexing a per-VM `Vec` of `(spur, env_version, value)` tuples; a matching spur and env version skips the `Env` lookup entirely. Biggest wins on global-call-heavy workloads (higher-order-fold 2.34x); entries invalidate on `env_version` mismatch when a global is redefined.

**Intrinsic** — a common builtin the compiler recognizes at a call site and compiles to a dedicated inline opcode (e.g. `+` → `AddInt`, `car` → `Car`, `length` → `Length`) instead of a global lookup plus call, eliminating the call overhead. Fires only when the call references the canonical global with matching arity and that global hasn't been redefined in the compilation unit.

**Lexer** — the single-pass tokenizer (`lexer.rs`) that walks a `Vec<char>` and emits `SpannedToken`s of 24 token types (brackets, quote forms, numbers, strings, f-strings, regex, keywords, symbols, etc.). The only place source positions enter the system; emits trivia tokens (Newline, Comment) the parser skips but the formatter and LSP use.

**Lowering** — the first compiler pass (`lower.rs`): converts the `Value` AST into `CoreExpr`, a desugared IR. The ~40 surface special forms collapse to ~35 `CoreExpr` node kinds (e.g. `cond` → nested `If`, `case` → `Let` + `If`). Tail-position analysis happens here.

**Macro expansion** — the step (in `sema-eval`) where `defmacro`-defined macros are expanded to more `Value` AST before compilation; expansion is performed VM-natively, and the result feeds the same Lower→Optimize→Resolve→Compile pipeline. Auto-gensym names like `x#` lex as plain symbols.

**Magic number** — identifying leading bytes of a binary format. `.semac` files start with `\x00SEM` (used to auto-detect bytecode vs source, since source never starts with a null byte); bundled executables use the `SEMAEXEC` archive/trailer magic. Two distinct formats sharing the concept; also used for corruption detection.

**Mini-eval** — a removed minimal evaluator once inlined in the stdlib to bypass the full trampoline (inlining `+`, `=`, `assoc`, etc.). Deleted because it caused semantic drift from the real evaluator and blocked the bytecode VM. Replaced by the callback architecture; its removal regressed the tree-walker ~3x, which the VM more than recovered.

**NaN-boxing** — a technique that packs every Sema value into a single 8-byte `u64` by encoding non-float types in the unused payload bits of an IEEE 754 quiet NaN. Floats are stored as raw `f64` bits; all other types use a tag plus payload. Immediate types (nil, bool, char, small int, symbol, keyword) need no heap allocation; heap types store an `Rc` pointer in the payload. It is why typed arrays (raw `f64`/`i64`) are faster than NaN-boxing every list element. Same technique used by Janet.

**NaN-boxed int fast path** — specialized opcodes (`AddInt`/`SubInt`/`MulInt`/`LtInt`/`EqInt`) that operate directly on raw NaN-boxed `u64` bits — sign-extending the 45-bit payload, doing the arithmetic, re-boxing — without ever constructing a `Value`, avoiding Clone/Drop overhead. An unchecked-overflow bug here once silently truncated large adds/subs crossing the small-int boundary (caught by the metamorphic fuzzer); the fix made `+`/`-` promote on overflow like `*`.

**NativeFn** — a Rust-implemented builtin function value. Signature `(&EvalContext, &[Value]) -> Result<Value, SemaError>`; `NativeFn::simple()` for context-free fns, `NativeFn::with_ctx()` for those needing the context. VM closures are also wrapped as `Value::NativeFn` so external code can call them. `CallNative` dispatches by index when `known_natives` is supplied at compile time. Also exposed to embedders via `register_fn` (Rust) / `registerFunction` (JS); a "yielding native" can suspend an async task.

**Opcode** — a single-byte VM instruction code (the `Op` enum, 69 opcodes in `sema-vm`). Most are one byte; some carry inline operands (`u16`/`u32`/`i32`). Categories: constants/stack, variable access, control flow, functions, data constructors, arithmetic/comparison, inline intrinsics, exceptions. `opcodes.rs` (`Op` + `Op::from_u8`) is the single source of truth.

**Optimize pass** — the compiler pass (`optimize.rs`) running on `CoreExpr` between lowering and resolution: constant folding (`(+ 1 2)` → `3`), comparison/boolean folding, control-flow simplification (`(if #t a b)` → `a`), and dead-code elimination. Why `sema compile` of `(+ 1 2)` yields a single `CONST 3`.

**Parser** — the recursive-descent parser (`reader.rs`) that consumes `SpannedToken`s and produces `Value` nodes plus a `SpanMap`, dispatching on token type (LParen→list, LBracket→vector, LBrace→map). Produces `Value` directly (no intermediate AST); handles dotted pairs via a `"."` marker symbol.

**Peephole optimization** — a local instruction-pattern rewrite by the compiler — notably `(if (not X) A B)` compiled to `JumpIfTrue` instead of `Not` + `JumpIfFalse`, eliminating one instruction and the `not` call.

**PGO** — Profile-Guided Optimization: the distributed binaries are instrumented, trained on the benchmark suite plus a 1BRC sample, the profile merged with `llvm-profdata`, and the binary rebuilt so LLVM lays out the dispatch loop's hot blocks by measured opcode frequency. Applied to cargo-dist releases and the Homebrew bottle (v1.19.2+), ~26–39% wins; a PGO failure ships fat-LTO instead. `cargo install` gets LTO but not PGO.

**Pop_unchecked** — the VM's unsafe unchecked stack-pop used on the hot dispatch path for speed. Sound only because in-process bytecode is balanced by construction and deserialized bytecode is proven balanced by the verifier; debug builds retain bounds checks via `debug_assert!`. Part of the VM's unsafe optimizations alongside raw-pointer bytecode reads.

**Prelude** — Sema source bundled in `sema-eval` (`prelude.rs`) and evaluated at interpreter startup to define library macros and functions (threading macros, `when-let`, and friends), expanded VM-natively through the same bytecode pipeline as user code. Distinct from a user file preloaded on the command line with `-l`/`--load`.

**Program counter (pc)** — the index of the current instruction in a chunk's bytecode. A jump simply sets `pc` to a different value. Used throughout: jump offsets are relative `pc` deltas, source maps map `pc`→line, exception tables specify `pc` ranges, breakpoints resolve source lines to `pc`s.

**Quote desugaring** — the reader's rewriting of quote syntax into real lists before evaluation: `'x` → `(quote x)`, `` `x `` → `(quasiquote x)`, `,x` → `(unquote x)`, `,@x` → `(unquote-splicing x)`. The syntax is reader-level; the semantics are evaluator-level. Sema has no user-extensible reader macros/readtables.

**Rc reference counting** — Sema's single-threaded memory model: every `Value` is `Rc` (non-atomic reference counting), giving deterministic destruction with no garbage collector. `Rc` (not `Arc`) avoids atomic increments and makes `Value`s non-`Send`/`Sync`. Cannot collect cycles, but Lisp closures tend to be tree-shaped; a future tracing GC is the named next runtime step.

**Reader** — Sema's front end (`sema-reader`): a two-phase pipeline where a lexer tokenizes source into `SpannedToken`s and a recursive-descent parser produces `Value` nodes directly — no separate AST type, since code is data. Quote sugar, f-strings, regex literals, short lambdas, and dotted pairs are desugared here. Reader errors are syntax/parse errors (`:reader`). Also `(read "...")` parses a string into a `Value`.

**Regex literal** — a raw-string literal `#"..."` whose contents are taken verbatim with no escape processing (only `\"` is special), letting you avoid double-escaping. The reader desugars it to a plain string `Value`; backed by the Rust `regex` engine (linear-time, no lookaround/backreferences).

**Resolution** — the compiler pass (`resolve.rs`) that walks `CoreExpr` and classifies every variable reference as `Local{slot}`, `Upvalue{index}`, or `Global{spur}`, producing `ResolvedExpr` — replacing runtime hash-based env lookup with direct slot indexing. Also marks captured locals and emits `UpvalueDesc`s. Described as "most of the gap between a teaching interpreter and a fast one."

**Runtime-delegated form** — a special form the compiler cannot lower to pure bytecode (`eval`, `import`, `load`, `defmacro`, `define-record-type`, `delay`/`force`, `prompt`/`message`/`deftool`/`defagent`, `macroexpand`), so it is compiled as a call to a corresponding `__vm-*` global function registered by `sema-eval`.

**SemaError** — Sema's `thiserror`-derived error enum (variants incl. Reader, Eval, Type, Arity, Unbound, Llm, UserException, plus `WithTrace`/`WithContext` wrappers), constructed via helper methods (`eval`, `type_error`, `arity`), never raw variants. Surfaced to Sema code as a structured error map with `:type`, `:message`, and `:value` (for user exceptions). Caught errors also include `:stack-trace` — a list of `{:name :file :line :col}` frame maps, innermost first.

**Short lambda** — a terse anonymous-function literal `#(...)` whose body is scanned for positional placeholders `%`, `%1`, `%2`…; bare `%` rewrites to `%1`, producing `(lambda (%1 … %N) body)`. Clojure-style; read/desugared by the reader. E.g. `#(* % %)` squares its argument.

**Slot** — a fixed integer index into the current function's stack frame where a local variable lives. The Resolve pass replaces variable names with slots so a runtime read is an array index, not a hash lookup. Slots 0–3 have dedicated zero-operand opcodes (`LoadLocal0..3`). Contrast with upvalue indices and global Spurs.

**Source map** — an as-yet-unimplemented `.semac` debug section (0x10) linking bytecode PCs back to source file/line/column via delta-encoded LEB128 entries, to enable file/line error messages when running compiled bytecode. At runtime, in-process source positions come from the `EvalContext` span table instead.

**Span (source)** — a source-location range (line, col, end_line, end_col) recorded per token by the lexer and attached to compound values for error reporting. Stored in a side table (`SpanMap`) keyed by `Rc`-pointer address rather than inside the NaN-boxed `Value`, so `Value` stays 8 bytes; only list/vector values get spans (atoms don't). Distinct from a tracing/telemetry span — see *Span* under Observability.

**Spur** — a `u32` interned-string handle from the `lasso::Rodeo` interner. Symbols, keywords, and global variable names are stored as Spurs so equality and env lookups are O(1) integer comparisons. Process-local (per-thread) and not stable across processes, which is why `.semac` files remap them via a string table. `intern(s)` interns; `resolve(spur)` maps back.

**Stack-depth verifier** — an abstract-interpretation pass (ADR #56) inside `validate_bytecode` that proves a deserialized chunk's operand stack never underflows or exceeds its declared maximum, making the VM's unchecked `pop_unchecked` sound for untrusted `.semac` files. Uses a worklist over reachable instructions with a strict-equality lattice at join points; `Op::stack_effect()` is the shared source of truth. Sound but conservative.

**Stack machine** — a VM design with a single operand stack: operands are pushed and operators pop them, evaluating nested expressions without runtime recursion. Sema's value stack is a contiguous `Vec` of NaN-boxed `Value`s (good cache locality); the compiler emits operands before operators. Sema, CPython, and the JVM all use this model.

**String interning** — replacing repeated strings (symbol/keyword names) with shared integer handles (Spurs) in a global table, so identity checks become integer comparisons. In Sema done via an explicit `intern()` into a thread-local `lasso` Rodeo; goes back to McCarthy's LISP 1.5 "object list" (oblist).

**String table** — the required `.semac` section (0x01) holding every unique string the bytecode references (symbol/keyword names, string constants, paths). On load each is interned to a fresh Spur and a remap table maps file-local indices to process-local Spurs. String index 0 is reserved and must be the empty string. This is how Sema makes process-local Spurs portable into a file.

**Tail-call optimization (TCO)** — reusing the current call frame for a call in tail position (the function's last action) so deep/recursive calls don't grow the native stack. The compiler tags tail-position calls during the Lower pass (the `Call` node carries `tail: bool`) and emits `TailCall`, which reuses the frame. Tail positions include the last body expression, if-branches, cond clauses, and the last `and`/`or` operand. Named `let`, `do`, and tail-recursive functions all rely on it.

**Trampoline** — an evaluation technique where a step returns either a final value or an instruction to continue evaluating another expression, looped without growing the native stack — used to implement TCO in the now-retired tree-walker. Distinct from CPS-style "Cheney on the MTA" trampolining cited (in the Lisp comparison) for Chicken Scheme; the VM does TCO via `TailCall` instead.

**Tree-walker** — the original recursive AST-interpreting evaluator (now retired). It evaluated `Value` AST directly via the trampoline; the bytecode VM replaced it as the sole evaluator, yielding 2–17x speedups. Docs keep its benchmark numbers for comparison.

**Two-level dispatch loop** — see *Dispatch loop*.

**Upvalue** — a variable captured by a closure from an enclosing function. Sema uses the Lua/Steel "open upvalue" model: an `UpvalueCell` is `Open { frame_base, slot }` (pointing into the live VM stack) while the defining frame is alive, then `Closed(Value)` once it exits. Resolved at compile time: `UpvalueDesc::ParentLocal(slot)` captures from the immediate parent, `ParentUpvalue(index)` through an intermediate. Known limitation: `set!` to a captured local is lost when the closure runs through a stdlib HOF.

**Value** — Sema's single universal data type: a `#[repr(transparent)] struct Value(u64)` that NaN-boxes every Sema datum (numbers, lists, maps, lambdas, LLM types, …), pattern-matched via `val.view()` returning a `ValueView` enum. It is both the runtime value and the parsed-code representation (code is data). Defined in `sema-core`; not `Send`/`Sync` (uses `Rc`).

**Vector-backed list** — Sema's representation of `Value::List` as `Rc<Vec<Value>>` (contiguous array) rather than linked cons cells, giving O(1) `nth`/`length` and cache-friendly iteration at the cost of O(n) cons/append (`car` is `v[0]`, `cdr` a slice copy). A deliberate departure from traditional Lisp; contrast with Clojure's persistent vectors.

**VFS (Virtual File System)** — an in-memory file archive. Meanings: (1) a thread-local archive in `sema-core` of compiled bytecode plus bundled assets embedded into a standalone executable by `sema build` — file/import ops check it first, then fall back to the real filesystem; (2) the WASM/browser in-memory filesystem replacing real disk (quotas: 1 MB/file, 16 MB total, 256 files; pluggable persistence backends); (3) the notebook server's sandboxed file API over HTTP, scoped to the notebook's directory. Writes always target the real filesystem in case (1).

## LLM & GenAI

**Agent** — a bundle of a system prompt, tools, model, and turn limit (`defagent`) that runs a multi-turn loop, automatically handling the back-and-forth of tool calls until a final answer or `:max-turns`; run with `agent/run`. A first-class `Value` type (predicate `agent?`). Meanings to disambiguate: (1) the Sema LLM agent (this entry); (2) in telemetry, every `agent/run` emits an `invoke_agent` span (typed `AGENT`/`agent`/`chain` in compat tools).

**Auto-configuration** — Sema's startup behavior of detecting available providers from environment variables (API keys) and configuring them with no manual setup; triggerable with `llm/auto-configure`, skippable with `--no-llm`. Embedding providers are auto-configured separately from chat providers.

**Automatic retry** — built-in, config-free retrying of transient LLM failures (HTTP 429, 5xx, network/timeout) with capped exponential backoff and full jitter (base 500 ms, cap 30 s, up to 3 retries), honoring a 429 `retry-after` hint. 4xx-non-429 and parse errors fail fast. Distinct from `llm/with-fallback` (switches providers) and the stdlib generic `retry`; each retry emits an `llm.retry_attempt` span.

**Batch** — `llm/batch` sends multiple prompts concurrently and collects all results; `llm/pmap` maps a function over items and sends the resulting prompts in parallel. Distinct from the OTel batch span processor (telemetry export).

**Budget** — a spending limit enforced on LLM calls: a cost cap in dollars (`llm/set-budget`, `:max-cost-usd`) and/or a token cap (`:max-tokens`); calls that would exceed it fail. Scoped form `llm/with-budget`. Best-effort (warn-once) when model pricing is unknown; state is thread-local. Disambiguate: this spend budget vs. the Anthropic *thinking budget* (`budget_tokens`, see *Reasoning effort*) vs. the `EvalContext` `eval_deadline` (a wall-clock time budget).

**Cache hit** — when an LLM call is served from Sema's response cache instead of the provider. A cache hit makes no provider call, so it reports zero usage (must not recharge cost or burn budget) and is flagged in telemetry with `sema.gen_ai.cache.hit`. Consequently token metrics undercount real spend when caching is in play.

**Cache key** — the SHA-256 hash identifying a cached response (`llm/cache-key`), computed from a prompt and options; the response cache is keyed on prompt + model + temperature. Distinct from provider prompt-cache keys.

**Chat** — sending a list of messages (system/user/assistant) to an LLM and getting a reply, via `llm/chat`. Disambiguate: (1) the Sema operation `llm/chat`; (2) a provider-capability column in the support table; (3) the auto-generated OTel span `chat {model}` emitted for every non-streaming completion (typed `LLM`/`generation`/`task`/`llm` in compat tools).

**Chunk** — (1) *streaming chunk*: an incremental piece of a streamed LLM response, passed to the stream callback as it arrives (for Lisp-defined providers without streaming, the whole response is sent as a single chunk); (2) *text chunk*: a slice of text from `text/chunk` recursive splitting (`:size`/`:overlap`) for LLM/RAG pipelines. Unrelated: `list/chunk` (list partitioning) and the VM bytecode *Chunk*.

**Classification** — assigning text to one of a fixed set of keyword labels via `llm/classify`, which returns the best-matching keyword. A constrained form of extraction.

**Completion** — a single model response generated from a prompt. `llm/complete` sends one prompt and returns the generated text; the term also refers to the chat-completions API shape. In usage/cost reporting, *completion tokens* (`:completion-tokens`) are the output tokens, vs. prompt/input tokens.

**Conversation** — an immutable data structure holding chat history (and an optional model); every operation returns a new conversation value. Created with `conversation/new`, advanced with `conversation/say` (which makes an LLM round-trip); `conversation/last-reply` returns the latest reply. A first-class `Value` type (predicate `conversation?`). Immutability enables `conversation/fork`. Distinct from the telemetry `gen_ai.conversation.id` (see *Conversation id*).

**Cost** — the computed dollar cost of an LLM call/session, derived from token usage and pricing, reported as `:cost-usd` in usage maps and `gen_ai.usage.cost` / `gen_ai.usage.cost_usd` in telemetry. Cached reads are reported but not yet discounted. Some backends (e.g. LangSmith) recompute cost from token counts, so their number may differ.

**Default model** — the model id a provider uses when no `:model` is pinned and no `:default-model` was configured (e.g. `:anthropic` → `claude-sonnet-4-6`); also what `llm/with-fallback` substitutes per provider when the body leaves the model unpinned. Set per provider, globally via `SEMA_CHAT_MODEL`, or per call. Model ids are provider-specific (a Claude id sent to OpenAI returns a 404).

**Deftool** — a Sema special form defining an LLM-callable tool: a name, description, parameter schema, and handler lambda evaluated when the LLM invokes it. `ToolDef` is a first-class `Value` type. The MCP server auto-exposes `deftool` tools in filepath mode (underscore-prefixed or `:mcp/expose #f` are private). See *Tool*.

**Defagent** — see *Agent*. The Sema construct (and first-class `Agent` `Value` type) for defining an LLM agent, alongside `deftool`, `llm/extract`, and conversations.

**Finish reason** — the reason the model stopped generating (e.g. `end_turn`, `length`), reported as `:stop-reason` from providers (default `"end_turn"` for Lisp-defined providers) and traced as `gen_ai.response.finish_reasons` on the `chat` span.

**Fork** — `conversation/fork` creates an independent copy of a conversation so you can explore divergent directions from the same point; because conversations are immutable, the original and each fork stay independent. Used to run parallel "what about X?" branches.

**Fallback chain** — an ordered list of providers passed to `llm/with-fallback`; if a call fails on one provider it automatically retries on the next. Entries can be bare provider keywords or `[provider model]` / `{:provider :model}` pairs for per-provider model overrides. Each provider does its own transient-error retry first. Streaming bypasses the fallback chain.

**First-class LLM types** — Prompt, Message, Conversation, ToolDef, and Agent are distinct NaN-boxed `Value` types (not maps-with-conventions), with their own constructors, pattern matching, and display forms. Constructed at runtime via `__vm-prompt`/`__vm-message`/etc., so they are runtime-only and can't be serialized into a `.semac` constant pool. Sema's primary differentiator vs other Lisps.

**LlmProvider** — the single Rust trait all LLM backends implement (`name`, `complete`, `default_model`, plus optional `stream_complete`/`batch_complete`/`embed`), registered in a `ProviderRegistry`. The trait is `Send`+`Sync` (providers use tokio `block_on` internally) even though the runtime is single-threaded. Concrete providers: Anthropic, OpenAI, Gemini, Ollama. See *Provider*.

**Lisp-defined provider** — a provider implemented entirely in Sema via `llm/define-provider`, whose `:complete` function receives a request map and returns a string or response map. Enables echo/mock/proxy/routing providers and deterministic testing. Streaming falls back to a single chunk. Integrates with `llm/set-default`, `llm/list-providers`, etc. like any other provider.

**Max-tokens** — disambiguate two meanings: (1) the per-response generation cap (`:max-tokens`) limiting how many tokens the model may generate; (2) the budget `:max-tokens`, a session/scoped *spend* cap counting input+output. Anthropic extended thinking raises the generation cap above the thinking budget.

**Max-turns** — the upper bound on how many back-and-forth iterations an agent's tool loop may run, set in `defagent` and read with `agent/max-turns`. A turn-limit safety bound distinct from token/cost budgets; the loop also aborts after 5 consecutive tool errors.

**MCP** — Model Context Protocol — Sema's `sema mcp` server (`sema-mcp`) lets LLM clients (Claude Desktop, Cursor, Claude Code) inspect/compile/format/eval/build Sema code and call user-defined `deftool` tools, over stdio JSON-RPC 2.0. Default tools: `run_file`, `compile`, `eval`, `docs`, `fmt`, `disasm`, `build`, `info`, plus stateful notebook tools. Bundled executables can embed an MCP server via `--mcp`.

**Message** — a role-content pair where the role is a keyword (`:system`, `:user`, `:assistant`) and the content is text (and optionally an image), the atomic unit prompts and conversations are made of, created with `(message :role content)`. A first-class `Value` type (predicate `message?`); accessed via `message/role`/`message/content`; `message/with-image` attaches a bytevector image. In Lisp-defined providers the request `:messages` is a list of `{:role :content}` maps.

**Model** — the specific LLM variant a call targets, named by a provider-specific id string (e.g. `claude-haiku-4-5-20251001`, `gpt-5.4-mini`). Selected via `:model`, a provider default, or `SEMA_CHAT_MODEL`. Ids are not portable across providers (a mismatched id returns 404). Recorded in telemetry as `gen_ai.request.model`/`gen_ai.response.model`. "Reasoning/thinking models" are a subclass supporting `:reasoning-effort`.

**Multi-modal** — LLM input combining text with images, created via `message/with-image` (image as a bytevector) and consumed by vision-capable models. Media type (PNG/JPEG/GIF/WebP/PDF) is auto-detected from magic bytes. Vision support is provider/model-dependent.

**OpenAI-compatible provider** — any service implementing the OpenAI chat-completions API, registered with `llm/configure` by passing `:api-key` and `:base-url` with any provider name — no custom code. Covers Together, Fireworks, Perplexity, Azure OpenAI, Groq, vLLM, LiteLLM, etc. Contrasted with native providers (bespoke serializers) and Lisp-defined providers.

**On-tool-call callback** — an `agent/run` option (`:on-tool-call (fn (event) ...)`) that observes each tool call as `:start` and `:end` events during the agent loop. A runtime observability hook distinct from OTel spans/events.

**Parameter schema** — the map describing a tool's expected arguments (field name → `{:type ... :description ...}`), shown to the LLM so it knows how to call the tool; retrieved with `tool/parameters`. Calling with mismatched arguments doesn't abort an agent run — the mismatch is fed back as the tool result. Distinct from the extraction schema (see *Schema*).

**Pricing** — per-million-token input/output rates used to compute cost, resolved in order: custom (`llm/set-pricing`) > a bundled models.dev snapshot (2,400+ models, offline) > unknown (cost returns nil). Checked via `llm/pricing-status`. When unknown, budget enforcement degrades to best-effort.

**Prompt** — a first-class, composable, immutable data structure (not a string template) built from message expressions, which can be inspected, transformed, filled with template slots, and sent to an LLM. Built with the `prompt` macro using `(system ...)`, `(user ...)`, `(assistant ...)` shorthands; introspected with `prompt?`, `prompt/messages`, `prompt/slots`. Distinct from a *system prompt* (the instruction message) and a *prompt cache* (provider-side input caching).

**Prompt cache** — a provider-side cache of input tokens for a stable prompt prefix, yielding large savings when a prefix repeats; surfaced as `:cache-read-tokens` and `:cache-creation-tokens` in usage. OpenAI and Gemini 2.5+ cache implicitly; Anthropic caching is opt-in via `cache_control`. Distinct from Sema's own in-memory *response cache*. Cached reads are reported but not yet discounted in `:cost-usd`.

**Prompt slot** — a `{{key}}` placeholder in a prompt's message contents, filled from a map by `prompt/fill`; `prompt/slots` returns the still-unfilled slot names as keywords. Partial fills leave unfilled slots intact.

**Prompt template** — a string with `{{key}}` Mustache-style placeholders created by `prompt/template` and filled by `prompt/render` from a map; missing keys are left as-is and non-string values are stringified. Distinct from reader-level f-strings (`${...}`).

**Provider** — a backend LLM service (Anthropic, OpenAI, Gemini, Ollama, Groq, xAI, Mistral, Moonshot, etc.) that Sema auto-configures from environment variables and dispatches calls to; can be native, OpenAI-compatible, embedding-only, or Lisp-defined. `--chat-provider`/`SEMA_CHAT_PROVIDER` select one; embeddings have separate providers (Jina, Voyage, Cohere). Disambiguate from: a *fallback provider chain*; the OTel *tracer provider* (the telemetry SDK object — see Observability). See also *LlmProvider*.

**Proxy / gateway** — an LLM-observability tool that captures data by routing your model calls through it (sitting in front of the API) rather than receiving OTLP traces — e.g. Helicone, LiteLLM, Portkey, Pezzo. Sema's OTLP export cannot feed these; use the tool's own gateway integration.

**RAG** — Retrieval-Augmented Generation: a workflow that embeds documents, stores them, retrieves semantically relevant ones, and uses them to ground LLM responses. The canonical example for embeddings + vector store. See *Embedding*, *Vector store*, *Semantic search* under Data Structures & Standard Library.

**Re-ask** — on extraction validation failure, feeding the validation errors back to the LLM on the next retry so it can correct its response (`:reask?`, default true). A field validator's `:message` is surfaced in the re-ask prompt; bounded by `:retries` (default 2).

**Reasoning effort** — a single portable option (`:reasoning-effort`, taking `:minimal`/`:low`/`:medium`/`:high`/`:none`/`:xhigh`) controlling how much a reasoning/thinking model deliberates before answering. Sema maps it to each provider's native control: OpenAI `reasoning_effort`, Anthropic extended thinking `budget_tokens` (the "thinking budget"), Gemini `thinkingConfig.thinkingBudget`. No-op where unsupported. Accepted by `llm/complete`, `llm/chat`, and per-run on `agent/run`.

**Response cache** — Sema's in-memory, per-session cache of LLM responses keyed on prompt + model + temperature, enabled for a thunk with `llm/with-cache` (optional `:ttl`). A cache hit makes no provider call and reports zero usage by design. Inspected via `llm/cache-key`, `llm/cache-stats`, `llm/cache-clear`. Distinct from the provider-side *prompt cache*.

**Role** — the speaker designation of a message, expressed as a keyword: `:system` (instructions), `:user` (human input), or `:assistant` (model reply). Used in `message`, `conversation/add-message`, and filtered via `message/role`.

**Schema** — a map declaring expected fields and their types/constraints. Disambiguate: (1) *extraction schema* — fields → `{:type ...}` with `:optional`/`:validate`/`:message`, used by `llm/extract`; (2) *tool parameter schema* — a tool's callable-function shape passed to the LLM (see *Parameter schema*). Both are validated against but serve different roles.

**Semantic conventions (GenAI)** — the OpenTelemetry-agreed standard attribute names for LLM telemetry (e.g. `gen_ai.request.model`, `gen_ai.usage.input_tokens`, `gen_ai.response.finish_reasons`) that Sema emits so backends understand the data without per-tool glue. Tools that use their own names need `SEMA_OTEL_COMPAT`; Sema-specific extras use the `sema.gen_ai.*` prefix. See also Observability.

**Stop sequence** — a string (or list of strings, `:stop-sequences`) at which the model halts generation, passed through to providers in the request map.

**Streaming** — receiving an LLM response incrementally as chunks rather than one final string, via `llm/stream` (optionally with a per-chunk callback). Streaming calls bypass automatic retry, the response cache, budget enforcement, and the fallback chain — they hit the provider directly. For Lisp-defined providers streaming falls back to a single chunk. See also *Stream* under Data Structures & Standard Library.

**Structured extraction** — LLM-powered extraction of typed, schema-conforming data from unstructured text (`llm/extract`) or images (`llm/extract-from-image`), with optional validation and retry. The schema maps field names to type descriptors (`:string`/`:number`/`:boolean`/`:list`). Distinct from a tool's parameter schema.

**System prompt** — the instruction/persona message (role `:system`) conditioning an LLM's behavior. Passed via the `:system` option to `llm/complete`, set with `conversation/set-system`, or built as a `(system ...)` message inside a prompt. `conversation/say-as` overrides it for a single turn; an agent carries one in its definition (`agent/system`).

**Temperature** — the sampling option (0.0–1.0) controlling randomness/determinism of output. Part of the response-cache key (prompt + model + temperature). Forced to default while Anthropic extended thinking is active. OpenAI may reject it on certain models, and Sema learns to drop it (`DROP_TEMPERATURE`).

**Token** — disambiguate four meanings: (1) *LLM token* — the unit LLMs measure input/output in (roughly word-pieces); Sema tracks `:prompt-tokens`/`:completion-tokens`/`:total-tokens` and estimates counts via a chars/4 heuristic (`llm/token-count`, see *Token count*); (2) a *token-bucket* rate-limiter unit in `llm/with-rate-limit`; (3) an *auth bearer token* in OTLP headers; (4) a *lexer token* in the reader (unrelated). The chars/4 LLM estimate is heuristic, not a true tokenizer count.

**Token-bucket rate limiting** — a rate-limiting algorithm where requests consume tokens replenished at a fixed rate; `llm/with-rate-limit` caps LLM calls to N requests per second. The "token" here is a rate-limiter unit, not an LLM token. Wraps a thunk.

**Token count (heuristic)** — Sema's tokenizer-free estimate of token usage using a chars/4 heuristic, exposed by `llm/token-count`, `llm/token-estimate`, and `conversation/token-count` (reports `:method "chars/4"`). An estimate, not a true tokenizer count; distinct from provider-reported usage tokens in `llm/last-usage`.

**Tool** — disambiguate: (1) *LLM tool* — a function the LLM can invoke during a conversation, defined with `deftool` (name, description, parameter schema, handler) and passed via `:tools`; a first-class `Value` type (predicate `tool?`). (2) *observability/telemetry tool* — a backend application (Jaeger, Langfuse, Phoenix). (3) *developer tool* — an MCP/CLI tool (`sema fmt`, the MCP defaults). The OTel `execute_tool` span and `gen_ai.tool.*` attributes refer to meaning (1). See *Deftool*.

**Tool call** — an instance of the LLM deciding to invoke a defined tool with arguments, which the runtime dispatches to the handler. Each produces an `execute_tool` span and a correlated tool result fed back to the model. In Lisp-defined providers represented as `:tool-calls` maps with `:id`/`:name`/`:arguments`; observed via `agent/run`'s `:on-tool-call` callback.

**Tool loop** — the agent's automatic multi-turn cycle of sending messages, receiving tool calls, executing them, and feeding results back, repeated until a final answer or the turn limit; bounded by `:max-turns` and aborted after 5 consecutive tool errors. Errors (throwing tool, unknown tool, schema mismatch) are recovered in-loop by feeding the error back, not by aborting. Can be seeded with prior history via `:messages`.

**Tool result** — the output of executing a tool, correlated back to its tool call and fed to the model so it can continue. On error, the error text is fed back as the result rather than aborting the run. Tool-result correlation is mandatory for OpenAI-family providers; in OpenInference compat the result lands in the tool span's `output.value`.

**TTL** — time-to-live in seconds for cached responses (default 3600), passed as `{:ttl ...}` to `llm/with-cache`.

**Usage** — the token-accounting record for LLM calls — prompt/completion/total tokens plus cache tokens, model, and cost — returned by `llm/last-usage` (most recent) and `llm/session-usage` (cumulative, `SESSION_USAGE`). A cache hit reports zero usage. Exported as the `gen_ai.client.token.usage` metric histogram. State is thread-local.

**Validation** — checking an extracted result against its schema (required keys present, types match, optional per-field predicates via `:validate`) before accepting it; on failure a re-ask retry is triggered. `:validate` may be a boolean (on/off) or a per-field predicate `#(...)`.

## Observability (OpenTelemetry)

**Batch span processor** — the background mechanism that queues finished spans and exports them in batches so telemetry never blocks the program; tuned by `OTEL_BSP_MAX_QUEUE_SIZE`, `OTEL_BSP_MAX_EXPORT_BATCH_SIZE`, `OTEL_BSP_SCHEDULE_DELAY` ("BSP"). Network export batches; file export is synchronous.

**Code lens** — an LSP feature: every top-level expression shows a ▶ Run lens that, when clicked, evaluates all forms up to and including it in a sandboxed `sema eval` subprocess and reports value/stdout/stderr/timing via the custom `sema/evalResult` notification. Subprocess execution keeps the LSP backend thread free.

**Compatibility mode** — a `SEMA_OTEL_COMPAT` setting (`openinference`, `langfuse`, `traceloop`, `langsmith`, `braintrust`, or `all`) that makes Sema write extra, tool-specific attribute names alongside the standard `gen_ai.*` ones so backends keying off their own names read the data. Purely additive and self-healing (unknown modes ignored). Distinct from the OpenAI request-compat "mode" (drop-temperature etc.).

**Content capture** — opt-in recording of actual prompt/response text (and tool args/results) into spans, enabled by `OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT=true` (alias `SEMA_OTEL_CAPTURE_CONTENT`); off by default for privacy. When off, only token counts, model names, cost, timing, and span types are recorded; when on, long messages are truncated. Required before compat "content" attributes appear.

**Conversation id** — a telemetry identifier (`gen_ai.conversation.id`) on every span, generated per run or supplied via `:conversation-id`, tying together the spans of one logical conversation. Distinct from the Sema `conversation` value type; session id defaults to it.

**DAP** — Debug Adapter Protocol — Sema's `sema dap` server (`sema-dap`) enables step debugging over stdio JSON-RPC: breakpoints (line/conditional/exception), stepping, call-stack and variable/scope inspection, and evaluate-while-paused. Operates on the bytecode VM via debug hooks; an async Tokio frontend bridges to a backend OS thread running VM bytecode.

**Event** — disambiguate: (1) *OTel span event* — a point-in-time annotation attached to the current span, added with `(otel/event name attrs-map)`; (2) the agent `:on-tool-call` event (`:start`/`:end`), a runtime callback, not an OTel span event.

**Exporter** — the OTel component that ships spans/metrics to a destination — either over the network to an OTLP endpoint or to a local JSONL file (`SEMA_OTEL_FILE`). Configured via `OTEL_EXPORTER_OTLP_*`. The file exporter writes one JSON object per line synchronously; the network exporter batches in the background.

**LSP** — Language Server Protocol — Sema's `sema lsp` server (`sema-lsp`, on tower-lsp) provides IDE features (diagnostics, completion, hover, go-to-definition, references, rename, semantic tokens, signature help, code lenses, formatting) over stdio JSON-RPC. A single-threaded backend thread owns all `Rc`-based state behind tokio mpsc/oneshot channels. Runs parse diagnostics (errors) and compile diagnostics (warnings via the bytecode pipeline).

**Metric histogram** — a distribution-tracking metric instrument; Sema records two standard GenAI histograms over a network endpoint: `gen_ai.client.token.usage` (token counts, with a `gen_ai.token.type` dimension of `input`/`output`) and `gen_ai.client.operation.duration` (call latency in seconds). Cache hits report zero usage, so the token histogram undercounts under caching.

**OpenInference** — the OpenTelemetry attribute convention used by Arize Phoenix/AX and FutureAGI; Sema's `openinference` compat mode adds OpenInference span types and attributes (model/provider, tokens, cost, message I/O, tool args + schemas). Aliases `phoenix`, `arize`. Has no separate tool-result field — the result lands in the tool span's `output.value`.

**OpenTelemetry (OTel)** — an open, vendor-neutral standard for traces and metrics that Sema implements to record LLM/agent activity automatically, toggled by environment variables and exportable to any compatible backend. Off by default and zero-cost when off; telemetry is sent in the background so a slow backend never blocks the script. Sema never installs a global tracer provider when embedded as a library unless told to.

**OTLP** — the OpenTelemetry network protocol for shipping traces/metrics; Sema speaks OTLP so it works with any backend that accepts it (over HTTP or gRPC). Configured via `OTEL_EXPORTER_OTLP_ENDPOINT`/`_PROTOCOL`/`_HEADERS`/`_TIMEOUT`; default protocol `http/protobuf`. Tools that only ingest via their own SDK or proxy can't receive an OTLP push.

**Semantic tokens** — an LSP feature providing token-level classification for richer editor syntax highlighting, among Sema's LSP backend capabilities (completions, hover, folding ranges, inlay hints, etc.). For the GenAI attribute conventions, see *Semantic conventions (GenAI)* under LLM & GenAI.

**Session** — a grouping of multi-turn runs sharing a `session.id` (emitted alongside `gen_ai.conversation.id`) so the turns of one conversation appear together in tools that group by session (e.g. Langfuse), supplied via `:session-id` (defaults to the conversation id). Disambiguate from Sema's runtime "session" (the process lifetime for `session-usage` and the per-session response cache).

**Span** — disambiguate: (1) *trace span* (this section) — an individual timed operation within a trace (a single LLM call or tool execution); spans nest to form the trace tree, each carrying a kind (CLIENT/INTERNAL), a name, and attributes. Sema names: `chat`, `embeddings`, `execute_tool`, `invoke_agent`, `notebook.cell`, `llm.retry_attempt`; users add spans with `otel/span`. (2) *source span* — a byte/line-column range in source the reader records (see *Span (source)* under Reader, Compiler & VM Internals).

**Span kind** — the OTel category of a span: `CLIENT` (an outbound call like `chat`/`embeddings`) or `INTERNAL` (in-process work like `execute_tool`/`invoke_agent`/retries). Separate from the compat *span type*.

**Span type** — a tool-specific label for a span added by a compatibility mode — e.g. Sema's `chat` span is typed `LLM` (OpenInference), `generation` (Langfuse), `task` (Traceloop), or `llm` (LangSmith). Distinct from OTel span *kind*; only written when a `SEMA_OTEL_COMPAT` mode is set.

**Trace** — one complete run, made of nested spans; an agent run appears as a tree (`invoke_agent → chat → execute_tool`). Grouped by `gen_ai.conversation.id`; multi-turn runs can be threaded into sessions.

**Tracer provider** — the OpenTelemetry SDK object that owns and emits spans. When embedded as a Rust library, Sema never installs a global tracer provider on its own; the host chooses behavior via `InterpreterBuilder::with_telemetry(TelemetryMode::...)` (`Off`, `UseHostGlobal`, `OwnProvider(p)`, `FromEnv`). This is the OTel meaning of "provider", not an LLM provider.

## Data Structures & Standard Library

**ANSI escape sequence** — control codes (e.g. `ESC[1;31m`) terminals interpret to style text (color, bold). `term/*` functions wrap strings in them and reset afterward; `term/strip` removes them; `term/rgb` uses 24-bit true color. In WASM all `term/*` return unstyled text.

**Baud rate** — the serial-port signaling speed (bits per second, e.g. 115200, 9600) passed to `serial/open` along with the device path and an optional read timeout. The serial module wraps the cross-platform `serialport` crate; unavailable in WASM, gated by the `serial` capability.

**Byte-buffer** — an in-memory read/write stream (`stream/byte-buffer`) where writes append and reads consume from the current position; contents extracted with `stream/to-bytes` or `stream/to-string`. For building strings/byte sequences incrementally without touching disk.

**Bytevector** — a packed array of unsigned 8-bit integers (0–255) for binary data and string encoding, with literal syntax `#u8(...)`. Supports indexed `ref`/`set!` (copy-on-write), `copy`, `append`, and UTF-8 conversion (`utf8/to-string`, `string/to-utf8`). Used for binary file I/O, base64, stream reads/writes, SQLite BLOBs, embeddings (little-endian `f64`), and multi-modal image inputs. A serializable constant-pool type (tag 0x0C).

**Capture group** — a parenthesized regex sub-pattern whose matched text is captured; `regex/match` returns them in `:groups`, and `$1`/`$2` (or `$name` for `(?P<name>...)`) reference them in replacements. `regex/match` returns a map with `:match`, `:groups`, `:start`, `:end` (byte offsets); non-capturing groups use `(?:...)`.

**Codepoint** — a single Unicode scalar value (an integer). `string/codepoints` returns the list of codepoints in a string and `string/from-codepoints` rebuilds one, revealing that one displayed glyph (e.g. an emoji family) can be several codepoints joined by a Zero Width Joiner (U+200D). Contrast `string/length` (characters) with `string/byte-length` (UTF-8 bytes).

**Context** — disambiguate: (1) *ambient context* (`context/*`) — a thread-flowing key-value store for tracing/metadata that auto-appends to log output, with scoped overrides (`context/with` pushes a temporary frame), ordered stacks, and hidden values (invisible to `get`/`all`/logs, for secrets); inspired by Laravel's Context. (2) the VM-internal *EvalContext* (see Reader, Compiler & VM Internals), not user-facing.

**Cosine similarity** — a similarity measure between two vectors based on the cosine of the angle between them, returning a value in [-1.0, 1.0]; used to compare embeddings via `llm/similarity`, `vector/cosine-similarity`, and `f64-array/dot` (dot product over magnitudes), and to rank vector-store results. Accepts both bytevectors (fast path) and lists of floats.

**Document** — a structured value (`document/create`) pairing `:text` with a `:metadata` map, designed for chunking and vector stores; `document/chunk` splits it while preserving and extending metadata (`:chunk-index`/`:total-chunks`). An LLM/RAG building block; distinct from PDF files processed by `pdf/*`.

**Embedding** — disambiguate: (1) *GenAI embedding* — a dense numeric vector representation of text (`llm/embed`), stored as a bytevector of little-endian `f64` values (or an `f64-array`) for memory efficiency and fast similarity math; accessed with `embedding/length`, `embedding/ref`, `embedding/->list`, `embedding/list->embedding`; auto-configured from `JINA_API_KEY`/`VOYAGE_API_KEY`/`COHERE_API_KEY`/OpenAI; traced as the `embeddings` span. (2) *hosting embedding* — using Sema as a scripting engine inside a Rust or JavaScript host (the `embedding.md`/`embedding-js.md` pages). Same English word, two domains.

**EOF (end of file)** — the end-of-input condition. Stdin reads (`io/read-line` etc.) return nil at EOF, and `io/eof?` reports it; stream reads return fewer bytes or nil. Distinguishes an empty line (`""`) from exhausted input (nil). (1.14.0 changed `io/read-line` to return nil, not `""`, on EOF.)

**Euclidean distance** — the straight-line distance between two vectors, computed by `vector/distance` on embedding bytevectors. Contrasted with cosine similarity (angle-based).

**Glob pattern** — a shell-style wildcard pattern (e.g. `src/**/*.rs`, `*.txt`) passed to `file/glob` to find matching paths; `**` matches across directories, `*` within a segment. Returns a list of matching paths. Distinct from the web-server route `*` wildcard and from regex.

**Handle** — a logical name or integer token referencing an opened resource in later calls — e.g. a KV store name, a SQLite database name (`db/open`), an integer serial-port handle (`serial/open`), or a spinner ID. Forms vary across modules (strings for KV/SQLite, integers for serial/spinner, opaque values for streams). Closing frees the handle; reusing a closed one errors.

**Hashmap** — an unordered, hash-backed map type (hashbrown/SwissTable) for O(1) performance-critical lookups, created with `hashmap/new`. Generic operations (`get`, `assoc`, `merge`, `count`) work on it and preserve the type; `hashmap/to-map`/`map/sort-keys` convert to a sorted map. Contrast with the default sorted `Map` (BTreeMap).

**KV store** — a persistent, JSON-backed key-value store (`kv/*`) for structured data across sessions, opened by a logical store name plus a file path; every `kv/set`/`kv/delete` immediately rewrites the whole backing JSON file. The file isn't created until the first write. Distinct from the in-memory ambient context store and from SQLite.

**Map** — disambiguate: (1) *map data type* — a curly-braced key-value collection `{:k v}` with deterministic sorted ordering, backed by a `BTreeMap` (the default `{}` literal); chosen as default because deterministic ordering matters for equality, printing, and tests, and maps can even be keys in other maps; `map/*` functions operate on it. (2) *the `map` function* — applies a function across each element of one or more lists, returning a list. (3) the `hashmap` sibling (see *Hashmap*).

**Middleware** — in the Sema web server, plain function composition: a function that takes a handler and returns a new handler, used to wrap cross-cutting behavior (logging, CORS, auth). No framework; composed by nesting or with `->`. Outermost middleware runs first.

**Parameterized query** — a SQL statement with `?` placeholders whose values are bound separately (`db/exec`, `db/query`, `db/query-one`), preventing SQL injection. `db/exec-batch` runs static SQL verbatim with no binding (injection-prone for user input). Result column names become keyword keys.

**Record** — a user-defined, named product type created with `define-record-type`, generating a positional constructor, a type predicate, and one accessor per field. Records are immutable, closed (fixed schema), have a distinct type tag, and are `equal?` only to same-type records with pairwise-equal fields. Not JSON/TOML-encodable (convert to a map first); no generic `get`/keyword access. `(type rec)` returns the type name as a keyword. A runtime-only `Value` type. Docs guideline: "maps at the boundary, records internally." Note: "record" is also used loosely in some examples for a data row/map.

**Request map** — the map a web-server handler receives, with `:method` (keyword), `:path`, `:headers` (string keys), `:query` and `:params` (keyword keys), `:body` (raw string), and `:json` (parsed body when Content-Type is application/json). `:params` holds route path parameters; `:json` is auto-populated only for JSON content type. Counterpart to the *response map*.

**Response map** — the `{:status :headers :body}` map returned by HTTP client calls and produced/consumed by the web server. `:status` is an int code, `:headers` a keyword-keyed map, `:body` a raw string. The same shape appears on both the client and server sides.

**Router / route** — `http/router` builds a handler from route definitions, each a vector `[method pattern handler]`. Methods include `:get`/`:post`/…/`:any` plus special `:ws` (WebSocket upgrade) and `:static` (static directory). Routes match top-to-bottom, first match wins; `:param` captures path segments into `:params`, `*` is a wildcard catch-all; `:static` falls through on a missing file (SPA index.html catch-alls).

**SSE (Server-Sent Events)** — a one-way streaming protocol the web server exposes via `http/stream`, which gives the handler a `send` callback; each `send` emits one SSE `data:` event and the stream stays open until the handler returns. Used for token-by-token streaming of LLM completions to the browser. Contrast with WebSocket (bidirectional).

**Standard streams (`*stdin*` / `*stdout*` / `*stderr*`)** — three global stream values for console I/O: `*stdin*` (readable), `*stdout*` (writable), `*stderr*` (writable). Earmuffed names following Lisp convention; used with `stream/write-string`, `stream/flush`. Spinners render to `*stderr*` to avoid corrupting `*stdout*`.

**Stream** — disambiguate: (1) *byte I/O stream* — a first-class, byte-oriented I/O handle providing a unified `stream/read`/`stream/write` interface across files, in-memory buffers, strings, and standard I/O (`stream/open-input`/`open-output`/`byte-buffer`/`from-string`). (2) *SSE/LLM stream* — Server-Sent Events from the web server (`http/stream`) or an LLM streaming callback delivering tokens. Streams are opaque values that can't be round-tripped (require re-eval on notebook reload). See also *Streaming* under LLM & GenAI.

**Strftime directive** — a `%`-prefixed token (e.g. `%Y`, `%m`, `%d`, `%H`, `%F`, `%T`) used by `time/format` and `time/parse` to format/parse timestamps, following chrono's strftime syntax. All `time/` functions operate in UTC with no timezone conversion.

**Typed array** — contiguous, unboxed numeric storage for performance-critical work: `f64-array` (64-bit float) and `i64-array` (64-bit signed int), literals `#f64(...)`/`#i64(...)`. Stores raw values in a flat `Vec` instead of NaN-boxing each element, giving cache locality and no per-element boxing; mutation is copy-on-write via `Rc::make_mut`. Provides `sum`/`dot`/`map`/`fold` in tight Rust loops; `f64-array/dot` powers embedding cosine similarity.

**Unicode normalization form** — a canonical/compatibility form (`:nfc`, `:nfd`, `:nfkc`, `:nfkd`) that `string/normalize` converts a string into, controlling composed vs decomposed characters and compatibility ligatures. Related: `string/foldcase` (case folding) and the Zero Width Joiner used to compose emoji.

**Unix timestamp** — Sema's representation of time: a UTC count of seconds since 1970-01-01 00:00:00 UTC, as a float with millisecond fractional precision. `time/now` returns this; `time-ms` returns integer milliseconds. Negative values are pre-1970. Distinguish seconds-based `time/*` from `sleep` (milliseconds).

**Vector** — disambiguate: (1) *vector data type* — an indexed, immutable collection with square-bracket literal syntax `[1 2 3]` backed by contiguous storage, giving O(1) `nth`/`first`/`length`; distinct from a list (cons-based). Many sequence functions accept a vector but return a list; also used as destructuring/match patterns. (2) *embedding/mathematical vector* — typically an `f64-array` used for dot-product/cosine-similarity work (see *Embedding*, *Cosine similarity*), NOT the `[...]` collection type. When the docs say "vector store" they mean embedding vectors. See also *Bytevector*.

**Vector store** — an in-memory (optionally disk-persisted) named store of documents with embeddings and metadata, supporting semantic search by cosine similarity. Managed via `vector-store/create`/`open`/`add`/`search`/`delete`/`count`/`save`. The backbone of RAG-style workflows; persisted as JSON with base64-encoded embeddings; search returns maps with `:id`, `:score`, `:metadata`. See *Semantic search*, *RAG*.

**Semantic search** — finding documents by meaning rather than keywords: embedding a query and ranking stored embeddings by cosine similarity (top-k) in the vector store (`vector-store/search` takes a query embedding and `k`). Core of the RAG workflow.

**WAL mode** — SQLite's Write-Ahead Logging journal mode, enabled by default when Sema opens a database (`db/open`), along with foreign-key enforcement. Improves concurrency of reads with writes. Backed by the `rusqlite` crate.

**WebSocket** — a bidirectional connection handled via `http/websocket` / the `:ws` route; the handler receives a connection map with `:send`, `:recv` (blocks, nil on close), and `:close` functions. Used for chat/broadcast patterns. Contrast with SSE (one-way).

## Concurrency

**Async / await** — `async` is a special form that spawns its body as a concurrent task on the VM scheduler, returning an async promise; `await` waits for that promise to resolve (or raises if rejected). Inside a task, `await` yields to the scheduler; at top level it runs the scheduler until resolution. Async features are VM-only (default backend since v1.13).

**Async task** — a unit of cooperative concurrency: a zero-argument thunk spawned with `async/spawn` (or the `async` form) that runs on the VM's cooperative scheduler and yields at yield points, returning a promise that resolves on completion. Cooperation, not parallelism — a CPU-bound task without yield points runs to completion before others; spawn order is preserved, channel wake order is FIFO. Also called a green thread/fiber/coroutine.

**Async/await implementation (VM)** — Sema's concurrency model implemented entirely in the VM: each `async`/`spawn` creates a new VM instance sharing the parent's global `Env` and function table, and a cooperative round-robin scheduler (`scheduler.rs`) runs them single-threaded until they yield. Deterministic (FIFO), so the grammar fuzzer can model order-independent async patterns. Not parallel/multithreaded.

**Mutable state** — Sema has **no** Clojure-style `atom` (and no `swap!`/`reset!`). Hold mutable state in a `define`d binding and update it with `set!`. Because the runtime is single-threaded (`Rc`, not `Arc`), no atomics or locks are needed for it.

**Channel** — a bounded FIFO buffer for communication and synchronization between async tasks, created with `(channel/new capacity)` (default 1, minimum 1). `channel/send` blocks (yields) when full, `channel/recv` blocks when empty and returns nil when the channel is closed and empty, `channel/close` closes it; `channel/try-recv` is non-blocking. Blocking only works inside an async task; from top level send/recv raise instead of waiting. (The web server's "channels" bridging HTTP I/O to the evaluator are a related concept at the Rust/Tokio boundary, not the Sema `channel/` API.)

**Cooperative scheduler** — the VM scheduler that interleaves async tasks at yield points (channel ops, `await`, `async/sleep`) rather than preempting them, preserving spawn order among ready tasks and waking channel receivers FIFO. Single-threaded — no true parallelism. Uses a virtual clock for deterministic sleep ordering.

**Promise** — disambiguate: (1) *async promise* — the result of a concurrent task (`async`/`async/spawn`), with states pending/resolved/rejected (and cancelled), operated on by `await`/`all`/`race`/`timeout`/`cancel`; (2) *lazy promise* — created by `delay` and evaluated by `force` (R7RS-style), tested by `promise?`/`promise-forced?`. The data-types table lists both as separate types; watch the `promise-forced?`/`async/forced?` overlap.

**Virtual clock** — the scheduler's logical time source used by `async/sleep`: it only advances when every task is blocked, jumping to the nearest deadline, so shorter sleeps deterministically wake before longer ones. On native it real-sleeps via `thread::sleep`; in the browser it blocks a Web Worker on `Atomics.wait`, falling back to instant advancement without cross-origin isolation. Durations capped at 86,400,000 ms.

**Yield point** — a place where an async task voluntarily suspends so the scheduler can run others — channel send/recv, `await`, and `async/sleep` (cancellation also takes effect here). A "yielding native" (e.g. `channel/recv`) passed directly to a higher-order function can't suspend cleanly — wrap it in a lambda; lambdas that yield resume correctly inside HOF callbacks.

**Yield signal** — a thread-local flag (`sema-core/src/async_signal.rs`) the VM sets to suspend a task at an await/channel/sleep point. On yield the VM leaves a nil placeholder on the stack and advances the PC; on resume the scheduler swaps in the wake value so the call appears to have simply returned. Replaced an earlier replay-based design that corrupted side effects. Yield-aware native fns must work on both the in-VM and fresh-VM closure paths.

## Tooling & Protocols

**ANSI / terminal control** — see *ANSI escape sequence* under Data Structures & Standard Library.

**Bundled executable** — a self-contained binary produced by `sema build` that injects a VFS archive into the Sema runtime binary (ELF raw append, Mach-O/PE section injection via `libsui`), embedding compiled bytecode, all transitive imports, and bundled assets; it runs with no Sema install required. Injection strategy is detected from the runtime binary's magic bytes (not the build host), so cross-compilation works from any platform. The 16-byte Linux trailer (`SEMAEXEC` magic + archive size) is frozen. Contrasts with `sema compile`, whose `.semac` resolves imports from disk at runtime.

**Entrypoint** — the file loaded when a package is imported — `package.sema` by default, or a custom file named in `sema.toml`'s `entrypoint` field. Resolution order: direct sub-module file → custom entrypoint → default `package.sema`. The package's short name becomes the namespace prefix.

**Fat LTO** — Fat Link-Time Optimization (`lto = "fat"`): lets LLVM inline across crate boundaries so the `sema-vm` dispatch loop can inline `sema-core` value accessors it calls millions of times. ~3–9% gain at ~2x build time. Used with PGO; targets that can't PGO fall back to fat LTO. Contrasted with thin LTO.

**Grammar-based fuzzer** — a fuzzer written in Sema itself (`fuzz/grammar-fuzz.sema`) that generates well-typed, closed, valid Sema programs and checks them against correctness oracles, plus crash detection. Exploits homoiconicity; every finding reproduces from one integer seed. Found two shipped bugs (a try-in-let VM crash and silent integer overflow). Distinct from the byte-level cargo-fuzz fuzzers that hammer the parser.

**Interpreter** — the top-level embedding object that holds the global environment and evaluates code, built via `Interpreter::builder()` (Rust) or `new SemaInterpreter()` (JS); each instance has fully isolated state. Builder options include `with_stdlib`, `with_llm`, `with_sandbox`, `with_allowed_paths`, `with_telemetry`. Multiple interpreters can coexist on one thread without sharing module cache/call stack.

**JSON envelope** — the structured JSON result emitted by `sema eval --json` (and notebook/WASM eval results): fields `ok`, `value`, `stdout`, `stderr`, `error` (message/hint/line/col), and `elapsedMs`. Designed for machine/editor/LSP consumption; the WASM `EvalResult` is a related `{value, output, error}` shape.

**Metamorphic law** — a fuzzer-generated theorem whose expected value is the literal `#t`, cross-checking an operation against an independent computation (e.g. `(= (reverse L) (foldl cons-flip L))` or distributivity). Because `#t` is true by construction, a broken op makes the two sides disagree. Caught the silent integer-corruption bug by forcing large intermediate products through a 2-arg add. Sidesteps the value oracle's self-masking blind spot.

**Oracle** — the judge in a fuzzer that decides whether an input revealed a bug. Sema's grammar fuzzer uses three: a printer⇄reader round-trip oracle, a differential value oracle (expected value computed bottom-up vs eval result), and metamorphic laws. The value oracle's blind spot ("self-masking") is that it computes the expected value with the very op under test; metamorphic laws avoid this.

**Raw mode / cooked mode** — terminal input modes: cooked mode (default) buffers a whole line until Enter; raw mode (`io/tty-raw!`) delivers each keystroke immediately, including Ctrl-C and arrows. `io/tty-restore!` returns to cooked mode using a restore-token. Unix-only; used to build TUIs with `io/read-key`, `sys/term-size`, and signal handlers. `io/tty-raw!` returns nil if stdin isn't a TTY.

**Registry** — a package registry server (default `pkg.sema-lang.com`, self-hostable) serving published Sema packages over a REST API/web UI; the alternative source is direct git repos. Registry commands (search/info/publish/yank/login) need a running instance; git packages work without one. `--registry`/`SEMA_REGISTRY_URL` override the default.

**REPL** — the interactive Read-Eval-Print Loop started by running `sema`; reads an expression, evaluates it, prints the result, and loops. Supports history, tab completion, multiline input, and comma-commands (`,quit`, `,doc`, `,type`, `,time`, `,env`, `,builtins`). History saved to `~/.sema/history.txt`; warns on redefining builtins.

**Sandbox / capability** — a capability sandbox stored on the `EvalContext` that restricts what a program can do by named permission gates (shell, fs-read, fs-write, network, env-read/write, process, llm, serial), surfaced via the `--sandbox` flag (modes: strict, all, or comma-separated capabilities). Permission failures produce `SemaError::PermissionDenied`/`PathDenied` (a denied call stays callable but returns the error). `--allowed-paths` confines file ops to directories. The WASM playground is inherently sandboxed.

**Sandbox / SSRF guard** — under `--sandbox`, Sema rejects provider `:base-url`/`:host` values pointing at loopback or private addresses (localhost, 127.0.0.1, 10.x, 169.254.169.254) to prevent Server-Side Request Forgery when running untrusted code. Local endpoints work normally unsandboxed (REPL/CLI/notebook). See also *Sandbox / capability*.

**Semver / lock file** — packages use semantic versioning (semver) for published versions; `sema.lock` records exact resolved versions (registry version + SHA256, or git ref + commit SHA) for reproducible builds, and `--locked` enforces it in CI. `sema.toml` is the manifest (`[package]`, `[deps]`); `sema.lock` is auto-generated and committed.

**Shebang** — a `#!/usr/bin/env sema` line on the first line of a `.sema` file that makes it directly executable; Sema treats the shebang line as a comment. Only allowed on the first line.

**Signal handler** — a callback registered for a Unix signal (`:winch`/SIGWINCH, `:int`/SIGINT, `:term`/SIGTERM) via `sys/on-signal`. Handlers are async-signal-safe: the OS handler only flips an atomic flag and the Sema callback runs later when `sys/check-signals` is called. Deferred dispatch keeps the single-threaded `Rc` runtime intact. No-ops on Windows.

**Spinner** — an animated terminal progress indicator (`term/spinner-start`/`-update`/`-stop`) using braille frames at 80 ms intervals, rendered to stderr and identified by an integer spinner ID so several can run concurrently. `spinner-stop` can show a final `{:symbol :text}` status line.

**VFS backend** — a pluggable persistence layer for the WASM in-memory VFS implementing the `VFSBackend` interface (`init`/`hydrate`/`flush`/`reset`). Built-ins: `MemoryBackend`, `LocalStorageBackend`, `SessionStorageBackend`, `IndexedDBBackend`. `hydrate()` loads files into the VFS on startup; `flush()` persists them out; `namespace` isolates storage between interpreters. See *VFS* under Reader, Compiler & VM Internals.
