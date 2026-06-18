# Evaluator Internals

::: info The tree-walker has been retired
Sema once shipped two evaluators: a tree-walking interpreter and a bytecode VM. The bytecode VM is now the **sole evaluator** — every entry point (the CLI, the REPL, the embedding API, `eval`, `import`/`load`, macros, and async/await) compiles to bytecode and runs on the VM. The tree-walking interpreter has been retired and is no longer reachable; the `--tw` CLI flag is accepted for backward compatibility but is a no-op.

For the architecture of the evaluator, see [Bytecode VM](./bytecode-vm.md).
:::

## The Single Evaluation Pipeline

All Sema code follows one path from source text to a result:

```
Source text
  → Reader        (tokenize + parse → Value AST)
  → Macro expand  (expand macros)
  → Lower         (Value AST → CoreExpr IR)
  → Optimize      (constant folding + simplification on CoreExpr)
  → Resolve       (CoreExpr → ResolvedExpr with slot/upvalue/global analysis)
  → Compile       (ResolvedExpr → bytecode Chunks)
  → VM execution  (stack-based dispatch loop)
```

Each phase is documented in [Bytecode VM](./bytecode-vm.md). Variables are resolved to direct slot/upvalue/global indices at compile time, closures use the Lua-style open-upvalue model, and tail calls reuse the current frame for tail-call optimization without growing the native Rust stack.

## Environment Model

Sema uses a linked-list scope chain, where each scope is a `hashbrown::HashMap` keyed by `Spur`:

```rust
// crates/sema-core/src/value.rs
pub struct Env {
    pub bindings: Rc<RefCell<SpurMap<Spur, Value>>>,
    pub parent: Option<Rc<Env>>,
    pub version: Cell<u64>,
}
```

`Rc<RefCell<...>>` makes each scope mutable and reference-counted. `SpurMap` is an alias for `hashbrown::HashMap` — keys are interned `Spur` handles (`u32`), so hashing is cheap integer hashing rather than string hashing. The `version` counter is bumped on every mutation; the VM's per-instruction inline caches use it to invalidate stale global lookups.

### Operations

| Method                    | Behavior                                                      |
| ------------------------- | ------------------------------------------------------------- |
| `get(spur)`               | Walk the parent chain, return first match                     |
| `set(spur, val)`          | Insert into the current (innermost) scope                     |
| `set_existing(spur, val)` | Walk the chain, update where found (for `set!`)               |
| `take(spur)`              | Remove from current scope only (for COW optimization)         |
| `take_anywhere(spur)`     | Remove from any scope in the chain                            |
| `update(spur, val)`       | Overwrite an existing binding in the current scope (for hot loops) |

The `take` method is critical for the copy-on-write map optimization described in the [Performance](./performance.md) page — by removing a value from the environment before passing it to a function, the `Rc` reference count drops to 1, enabling in-place mutation.

**Literature:** This is the standard lexical environment model described in _Lisp in Small Pieces_ (Queinnec, 1996, Chapter 6) — a chain of frames linked by static (lexical) pointers. The alternative for lexical scoping — flat closures that copy all free variables into each closure — is faster for lookup but uses more memory when closures share large environments. Sema uses the chained model because closures are pervasive and lookup cost is dominated by the `Spur` integer comparison, not chain traversal.

## Further Reading

- Christian Queinnec, [_Lisp in Small Pieces_](https://www.cambridge.org/core/books/lisp-in-small-pieces/66FD2BE3EDDDC68588A4605F14A4D2A4) (Cambridge, 1996) — the canonical deep-dive into Lisp interpreter and compiler implementation, covering environment models, continuations, and compilation strategies
- Guy Lewis Steele Jr., ["Rabbit: A Compiler for Scheme"](https://dspace.mit.edu/handle/1721.1/6913) (MIT AI Memo 474, 1978) — proves that tail calls can be implemented as jumps
- Abelson & Sussman, [_Structure and Interpretation of Computer Programs_](https://mitpress.mit.edu/9780262510875/structure-and-interpretation-of-computer-programs/) (MIT Press, 1996) — Chapter 5 shows how to compile to a register machine
- R. Kent Dybvig, ["Three Implementation Models for Scheme"](https://www.cs.indiana.edu/~dyb/pubs/3imp.pdf) (PhD thesis, 1987) — compares heap-based, stack-based, and string-based models; Sema uses heap-based (Rc+RefCell scopes)
