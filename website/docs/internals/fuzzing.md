# Fuzzing the VM

Sema is fuzzed two ways: byte-level fuzzers that hammer the parser frontend, and a **grammar-based fuzzer written in Sema itself** that generates *valid* programs and checks them against correctness oracles. The second is the interesting one — it has already found **two real, shipped bugs**: a VM crash and a case of silent integer corruption.

## The hard part of fuzzing: the oracle

Generating random input is easy. The hard part is the **oracle** — the judge that decides whether a given input revealed a bug. A crash-only fuzzer has a trivial oracle ("did it panic?") but is blind to the far more common failure mode: code that runs fine and silently returns the *wrong answer*. An oracle is what catches those.

Sema's grammar fuzzer (`fuzz/grammar-fuzz.sema`) leans on **homoiconicity** — a generated program is just an ordinary Sema value — to get two sharp oracles almost for free, plus crash detection:

### 1. Round-trip oracle (printer ⇄ reader)

```scheme
(= form (read (str form)))
```

Generate arbitrary valid s-expression *data* (atoms of every kind, nested lists, vectors, maps), print it, read it back, and assert structural equality. Any asymmetry between the printer and the reader falls straight out.

### 2. Differential value oracle (compiler/VM)

For a generated *program*, compute its **expected** value bottom-up *while generating it* — applying the real primitive ops to the already-known sub-values — then `eval` the whole nested form through the full `macro-expand → lower → optimize → compile → bytecode-VM` pipeline and compare:

```scheme
(= expected (eval form))
```

The expected value is the oracle. Because it's computed by straight-line, bottom-up evaluation while the form is run through the optimizing compiler and VM, a mismatch means the **compiler/optimizer/VM disagrees with the obvious answer** — constant folding, `if`/`let` lowering, closure capture, TCO, short-circuit logic, stack management, and so on.

### 3. Metamorphic / differential laws

The value oracle has one blind spot, and it's a sharp one: it computes `expected` by *calling the very operation under test*. If a native op like `reverse` is broken, both the oracle's `expected` and the form's `actual` route through the same broken `reverse` — they agree on the wrong answer, and the bug is invisible. (Verified the hard way: a deliberately no-op `reverse` slipped past 100,000 iterations of the value oracle.) Arithmetic escapes this only because the oracle computes it via the *native* builtin while the compiled form hits a different *inline opcode* — two implementations, so a divergence shows.

To cover the rest, the fuzzer also generates **metamorphic laws** — theorems whose expected value is the literal `#t`, cross-checking an op against an *independent* computation:

```scheme
(= (reverse L) (foldl (fn (a x) (cons x a)) (list) L))   ; reverse vs fold-cons
(= (append (take n L) (drop n L)) L)                      ; take/drop partition
(= (length L) (+ (length (filter even? L))                ; filter partition
                 (length (filter odd? L))))
(= (* a (+ b c)) (+ (* a b) (* a c)))                     ; distributivity
```

Because the expected is `#t` by construction (not computed by running the op), a broken op makes the two sides disagree → the law evaluates to `#f ≠ #t` → caught. This is the oracle that found the integer-corruption bug below.

### 4. Crash detection

Release builds are `panic=abort`, so a VM panic kills the process. The driver (`scripts/grammar-fuzz.sh`) writes the in-flight seed to a breadcrumb file before each iteration, so even a hard abort is reproducible from a single integer.

## How the generator stays sound

Every generated program is **well-typed and closed** — it references only variables it has bound, and each sub-expression has a known type and value. The generator threads an environment of `(symbol value type)` triples so every variable reference is in scope and type-correct, and it computes each form's expected value as it builds it. Types covered: `int`, `bool`, `float`, `string`, `list`, `vector`, `map`.

Everything is driven by a small, self-contained, seedable PRNG, so **every finding reproduces from one integer**. Iteration `i` uses seed `base + i`, re-seeding each time:

```
SEMA_FUZZ_SEED=<seed> SEMA_FUZZ_COUNT=1   # reproduce a single finding
```

### What it covers

Arithmetic (`+ - *` incl. variadic, `min`/`max`, `mod`, `abs`, unary `-`, `expt`), bitwise ops (`bit/and|or|xor`, shifts, `bit/not`), all comparisons, numeric and type predicates (`even?`/`zero?`/…, `string?`/`list?`/`map?`/`vector?`/`bool?`/`nil?`), `and`/`or`/`not`, `if`, `cond`, `case`, `match` (including binding clauses), multi-binding mixed-type `let`, multi-arg and curried lambdas, `try`/`throw`/`catch`, `apply`, named-let TCO recursion at large N, and a broad set of list/vector/map/string operations (`map`/`filter`/`foldl`/`reverse`/`append`/`cons`/`range`/`take`/`drop`/`sort`/`nth`/`length`/`last`, `assoc`/`dissoc`/`get`/`count`/`contains?`/`merge`/`keys`/`vals`, `string-append`/`substring`/`upcase`/`downcase`/`string/repeat`/`number->string`/…).

**Concurrency** is fuzzed too, exploiting the fact that Sema's scheduler is cooperative and FIFO — i.e. deterministic. Only patterns whose result is computable regardless of interleaving are generated: `(async/all (list (async T) …))` preserves spawn order (so the result is exactly predictable), and channel fan-in is reduced order-independently (`sum`). The task bodies `T` are ordinary generated programs, and `async` captures enclosing locals, so this also exercises closures crossing task boundaries and the in-VM higher-order-callback path.

**Excluded by design:** anything the value oracle can't model soundly. That means non-determinism — LLM calls, time, randomness, `uuid`, file/network I/O, and the timing-dependent async primitives (`async/sleep`, `async/timeout`, `async/race`, cancellation) — *and* `set!`: the value oracle assumes every sub-expression is referentially transparent (same value however many times it's evaluated), and `set!` is the lone impurity, so combined with async or law-style duplication its evaluation count diverges from the bottom-up model and produces false positives. (`set!` correctness is covered by the eval test suite instead.)

## Running it

```bash
jake fuzz.grammar                          # default sweep (random seed)
jake fuzz.grammar SEED=123 N=20000 DEPTH=6 # pinned, larger, deeper
jake fuzz.grammar-emit                     # print sample generated programs
```

Exit status: `0` all clear, `1` a deterministic value/round-trip mismatch (the program prints the offending form, expected, actual, and the reproducing seed), `2` a hard crash (the driver prints the reproducing seed).

## Case studies: two real bugs it found

**1 — A crash (`try` in a `let` binding).** Expanding the grammar to cover `try`/`catch` immediately produced a crash. Minimized:

```scheme
(let ((a 1) (b (try (throw 1) (catch e 2)))) b)   ; aborted instead of returning 2
```

A throwing `try`/`catch` used as a **non-first binding in a parallel `let`** corrupted the operand stack. The compiler pushed all binding inits onto the operand stack before storing them but didn't track the stack height for those pushes, so the exception handler restored the stack *below* the earlier already-pushed bindings; the subsequent stores and local-slot reads then went out of bounds. (`let*`, `letrec`, and function calls tracked the height correctly and were unaffected.) The fix was a few lines in `compile_let`; afterward, **715,000 generated programs up to depth 9 ran with zero crashes and zero value-oracle mismatches.**

**2 — Silent integer corruption (caught by a metamorphic law).** The distributivity law `(= (* a (+ b c)) (+ (* a b) (* a c)))` failed for some large operands. Minimized:

```scheme
(let ((a 9000000000000)) (+ a a))   ; => -17184372088832, should be 18000000000000
```

The branchless inline `+`/`-` opcodes did raw-bit arithmetic on the NaN-box payload and masked to 45 bits with **no overflow check**, so any runtime add/subtract whose result crossed the small-int boundary (~±17.5 trillion / 2⁴⁴) was silently truncated. `*` was already correct (it builds via `Value::int`, which promotes to a boxed integer on overflow); literal operands were masked by constant folding. This is exactly the kind of bug the plain value oracle is blind to (it computes `expected` via the native builtin, which was fine — only the inline opcode was wrong), and it took the *metamorphic* law, which forces large intermediate products into a 2-arg add, to expose it. The fix made `+`/`-` mirror `*`.

Both bugs were shipped, both gave wrong answers on perfectly valid code, and both were invisible to the entire test suite until the fuzzer's grammar (and oracle) reached the relevant corner.

## Extending the grammar

To teach the fuzzer a new construct, add a production to the generator for the result type (`gen-int`, `gen-bool`, `gen-flt`, `gen-str`, `gen-ilist`, `gen-vec`, `gen-map-v`), building the form and its expected value together. Two rules:

- **Determinism.** If a construct's result can't be predicted while generating it, it has no oracle and doesn't belong here.
- **Mind the self-masking trap.** If you compute `expected` by calling the same operation the form uses (true for any native builtin with a single implementation), a bug in that op hides itself. For those, add a **metamorphic law** in `gen-law` instead — a theorem cross-checking the op against an *independent* computation — so the oracle is genuinely independent of the implementation. And keep impure constructs (anything with a side effect, like `set!`) out: the value oracle's bottom-up model only holds for referentially-transparent expressions.
