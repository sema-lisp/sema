# sema-codegen

Cranelift native code generator for the [Sema](https://sema-lang.com) programming language.

The bytecode VM stays the sole evaluator. This crate is an accelerator beside it:

- **Decoder** — checks a compiled function's bytecode against a small, provably pure subset
- **Translator** — lowers that subset to Cranelift IR, with NaN-boxed values as raw 64-bit integers
- **JIT backend** — emits machine code the VM calls instead of pushing a frame

Compiled code handles only *immediate* values — fixnums, floats, booleans, nil, chars, symbols, keywords — which hold no `Rc`, so it performs no reference-count work. Anything else, and any operation that would allocate or raise, runs on the VM instead.

Native-only: WebAssembly hosts cannot map executable pages.

## Usage

This is an internal crate. If you want to embed Sema in your application, use [`sema-lang`](https://crates.io/crates/sema-lang) instead:

```toml
[dependencies]
sema-lang = "1.34"
```

To enable native code generation from the CLI, run `sema --jit`.

📖 [CLI reference](https://sema-lang.com/docs/cli) · [Performance internals](https://sema-lang.com/docs/internals/performance) · [GitHub](https://github.com/sema-lisp/sema)
