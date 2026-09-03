---
outline: [2, 3]
---

# Build a Bytecode VM (in Sema)

Compilers have a reputation for being black magic — dragon-book mystique, register allocators, the works. Most of that reputation is about *optimizing* compilers for *hardware*. The core idea — turning a program into instructions a machine can run in a loop — is small enough to fit on one screen.

So let's build one. A real compiler and a real virtual machine, in about 80 lines of Sema, that you can paste into the [playground](https://sema.run) and run right now. Then we'll show that Sema's own engine — the thing running your code — is the *same pipeline*, just larger and faster.

By the end you'll be able to read [Bytecode VM](./bytecode-vm.md) and [Bytecode File Format](./bytecode-format.md) and recognize every piece.

## The pipeline

Every language that "compiles to bytecode" — Python, Lua, the JVM, Sema — follows the same shape:

```
source text → [ front end ] → instructions → [ virtual machine ] → result
```

Sema's full version has a few more stages (we'll get to them):

```
source → Reader → Lower → Optimize → Resolve → Compile → bytecode → VM
```

We're going to **borrow Sema's reader** for the front end, because our toy language *is* s-expressions — `(+ 1 (* 2 3))` is already a tree once Sema reads it. That lets us skip straight to the two stages everyone thinks are magic and aren't: **the compiler** (tree → instructions) and **the VM** (instructions → result).

## A stack machine

Our VM is a *stack machine*. It has one stack and a handful of instructions that push and pop it. This is how most real bytecode VMs work (Sema, CPython, the JVM) — it's simpler than juggling registers.

Watch how `(+ 1 (* 2 3))` becomes a flat list of instructions, and how running them left-to-right computes the answer using nothing but a stack:

| Instruction | Stack after |
| ----------- | ----------- |
| `push 1`    | `(1)`       |
| `push 2`    | `(2 1)`     |
| `push 3`    | `(3 2 1)`   |
| `mul`       | `(6 1)` &nbsp;&nbsp;← popped 3 and 2, pushed 6 |
| `add`       | `(7)` &nbsp;&nbsp;&nbsp;&nbsp;← popped 6 and 1, pushed 7 |

The answer, `7`, is the last thing on the stack. That's the whole trick: **operands go on the stack, operators consume them.** No magic — just a discipline for evaluating nested expressions without recursion at run time.

## The compiler

The compiler is one recursive walk over the tree. Each kind of node emits a little list of instructions:

- a **number** → `push` it
- a **variable** → `load` it by name
- a **binary form** `(op a b)` → compile `a`, compile `b`, then emit the operator (so the operands are already on the stack when it runs)

```sema
(define (second xs) (nth xs 1))
(define (third xs)  (nth xs 2))

;; (op a b) -> [ ...code for a... ][ ...code for b... ][ op ]
(define (emit-binary opcode args)
  (append (compile-expr (first args))
          (compile-expr (second args))
          (list (list opcode))))

(define (compile-expr expr)
  (cond
    ((number? expr) (list (list :push expr)))
    ((symbol? expr) (list (list :load expr)))
    (else
      (let ((op (first expr)) (args (rest expr)))
        (cond
          ((= op '+)  (emit-binary :add args))
          ((= op '-)  (emit-binary :sub args))
          ((= op '*)  (emit-binary :mul args))
          ((= op '<)  (emit-binary :lt  args))
          ((= op 'if) (compile-if args))
          (else (error (str "unknown form: " op))))))))
```

That's the entire expression compiler. Notice it emits operands *before* the operator — that "postfix" ordering is exactly what makes the stack machine work.

## Control flow is just jumps

The one piece that feels like it should be hard — `if` — turns out to be two jumps. We compile the test, then a **jump-if-false** that skips over the "then" branch, then the "then" code, then an unconditional **jump** that skips the "else", then the "else" code:

```sema
(define (compile-if args)
  (let* ((test (compile-expr (first args)))
         (then (compile-expr (second args)))
         (alt  (compile-expr (third args)))
         ;; jump-if-false skips THEN + the trailing jmp
         (jf   (list (list :jfalse (+ (length then) 1))))
         ;; jmp at end of THEN skips ELSE
         (jp   (list (list :jmp (length alt)))))
    (append test jf then jp alt)))
```

The jump *offsets* are computed from the lengths of the branches we just compiled — relative "skip N instructions" jumps. Every loop, conditional, and `&&`/`||` you've ever written compiles down to exactly this: conditional and unconditional jumps over blocks of instructions. There is no `if` at the machine level — only "maybe jump."

## The virtual machine

The VM is a loop. It holds a **program counter** (`pc`, which instruction we're on) and a **stack**. Each turn of the loop reads one instruction and does the obvious thing:

```sema
;; pop two, apply f, push result. (a was pushed first, so it's deeper.)
(define (binop f stack)
  (let ((b (first stack)) (a (second stack)) (more (rest (rest stack))))
    (cons (f a b) more)))

(define (run code env)
  (let loop ((pc 0) (stack '()))
    (if (>= pc (length code))
      (first stack)                         ; result = top of stack
      (let* ((ins (nth code pc)) (op (first ins)))
        (cond
          ((= op :push)   (loop (+ pc 1) (cons (second ins) stack)))
          ((= op :load)   (loop (+ pc 1) (cons (get env (second ins)) stack)))
          ((= op :add)    (loop (+ pc 1) (binop + stack)))
          ((= op :sub)    (loop (+ pc 1) (binop - stack)))
          ((= op :mul)    (loop (+ pc 1) (binop * stack)))
          ((= op :lt)     (loop (+ pc 1) (binop < stack)))
          ((= op :jmp)    (loop (+ pc 1 (second ins)) stack))
          ((= op :jfalse)
            (let ((top (first stack)) (more (rest stack)))
              (if top (loop (+ pc 1) more)
                      (loop (+ pc 1 (second ins)) more))))
          (else (error (str "bad op: " op))))))))
```

That `cond` is the **dispatch loop** — the literal heart of every bytecode interpreter. A jump is just "set `pc` to somewhere else instead of `pc + 1`." Sema's real dispatch loop is the same idea with 66 instructions instead of 8, written in Rust for speed.

## Run it

```sema
(define program '(+ 1 (* 2 3)))
(define bc (compile-expr program))
(println "source:   " program)
(println "bytecode: " bc)
(println "result:   " (run bc {}))

(define p2 '(if (< x 5) (* x 10) (- x 5)))
(println "")
(println "source:   " p2)
(println "bytecode: " (compile-expr p2))
(println "x=3   ->  " (run (compile-expr p2) {'x 3}))
(println "x=9   ->  " (run (compile-expr p2) {'x 9}))
```

Output:

```
source:    (+ 1 (* 2 3))
bytecode:  ((:push 1) (:push 2) (:push 3) (:mul) (:add))
result:    7

source:    (if (< x 5) (* x 10) (- x 5))
bytecode:  ((:load x) (:push 5) (:lt) (:jfalse 4) (:load x) (:push 10) (:mul) (:jmp 3) (:load x) (:push 5) (:sub))
x=3   ->   30
x=9   ->   4
```

That's a working compiler and VM. You can see the bytecode it produces, watch the `if` become a `:jfalse`/`:jmp` pair, and run the result. Nothing was hidden.

## How Sema does the same thing, at scale

Sema's engine is this exact pipeline — every concept above has a real counterpart, just bigger and faster. Here's the map:

| Toy version (this page)            | Sema's engine                          | Where |
| ---------------------------------- | -------------------------------------- | ----- |
| Borrowed the reader                | Real lexer + recursive-descent parser, with source spans | [Reader & Spans](./reader.md), `crates/sema-reader` |
| `compile-expr` (one flat walk)     | Four passes: **Lower → Optimize → Resolve → Compile** | [Bytecode VM](./bytecode-vm.md), `crates/sema-vm` |
| 8 keyword "ops"                    | **66 real opcodes** (`Op` enum)        | [Bytecode VM](./bytecode-vm.md#instruction-set) |
| Cons-list as the stack             | A contiguous value stack of NaN-boxed `Value`s | [Architecture](./architecture.md#the-value-type) |
| Name-keyed `env` map for variables | Names **resolved to integer slots / upvalues at compile time** | [Bytecode VM](./bytecode-vm.md), `resolve.rs` |
| `run` interpreting a list          | A tight Rust dispatch loop with per-instruction inline caches | [Performance](./performance.md) |
| (we never saved it)                | Serialize to a `.semac` file           | [Bytecode File Format](./bytecode-format.md) |

A few of those are worth a sentence each, because they're exactly the corners we cut:

::: tip We hand-waved variable lookup; Sema doesn't.
Our VM looks variables up by *name* in a map at run time (`(get env ...)`). That's the slow way. Sema's **Resolve** pass walks the program once at compile time and replaces every variable with a fixed integer **slot** (a stack offset) or an **upvalue** index for closures. At run time a variable read is an array index, not a hash lookup. That single idea is most of the gap between a teaching interpreter and a fast one — see `resolve.rs`.
:::

::: tip We had no optimizer; Sema folds constants.
Sema's **Optimize** pass runs on the intermediate representation before compilation — constant folding, simple simplification — so `(+ 1 2)` becomes the constant `3` at compile time instead of two pushes and an add.
:::

::: info Desugaring: ~40 forms become ~35.
Sema's **Lower** pass turns the ~40 special forms you actually write (`let`, `cond`, `when`, `and`, `or`, `case`, …) into a small core IR (`CoreExpr`) of ~35 node kinds — `cond` becomes nested `if`, `and`/`or` become `if`, and so on. The compiler only has to know about the small core, exactly like our `compile-if` only knew about `if`. Macros expand earlier, in `sema-eval`, and feed the same pipeline.
:::

## What we skipped (and where Sema handles it)

To stay on one screen, our toy left out the genuinely harder parts of a real language runtime. None of them are magic either — they're just more code:

- **Functions & closures.** Calling a function means pushing a *call frame* and jumping; a closure that captures a variable needs **upvalues**. Sema uses the Lua-style "open upvalue" model — see [Bytecode VM](./bytecode-vm.md) and `vm.rs`.
- **Tail calls.** Sema reuses the current frame for a call in tail position, so deep recursion doesn't grow the native stack — see [Evaluator & TCO](./evaluator.md).
- **Memory.** We leaned on Sema's own reference counting. Sema's heap-backed values use `Rc` for deterministic destruction, with a synchronous cycle collector for unreachable cycles — see [Architecture](./architecture.md#why-rc-not-arc).
- **Macros.** Sema expands macros *before* compilation, producing more AST that goes through the same pipeline you just built.

## The punchline

You just read a compiler and a virtual machine end to end. The "magic" turned out to be a recursive walk that emits operands before operators, a couple of jumps for control flow, and a loop with a `cond` in it.

Sema's engine is the same shape. It's bigger because it has 66 opcodes, four well-separated passes, integer slot resolution, inline caches, closures, and a verifier for untrusted bytecode — but if you trace any Sema program from source to result, you'll pass through every stage you just built by hand. When you're ready for the real thing:

- [Bytecode VM](./bytecode-vm.md) — the actual pipeline and instruction set
- [Bytecode File Format](./bytecode-format.md) — how compiled bytecode is laid out on disk
- [Architecture](./architecture.md) — the `Value` type, the environment model, the whole engine
- The source: `crates/sema-vm/src/{lower,resolve,compiler,vm}.rs`
