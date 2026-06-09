# LLM Pipeline & Stdlib Extensions — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add 15 features spanning LLM response caching, retry/reask, fallback chains, text processing, vector stores, and convenience utilities to make Sema a batteries-included LLM scripting language.

**Architecture:** Features are split across two crates — `sema-llm` (tasks 1-3, 7-10, 12-13) and `sema-stdlib` (tasks 4-6, 11, 14). Each feature is a self-contained module or addition to `builtins.rs`. Thread-local state pattern used for LLM runtime. Stdlib modules use `register_fn` pattern.

**Tech Stack:** Rust 2021, serde_json, sha2, chrono, regex, tokio (for LLM calls). No new crate dependencies.

---

## Sub-Plans (by priority)

| File                                                              | Priority | Tasks | Features                                                                                         |
| ----------------------------------------------------------------- | -------- | ----- | ------------------------------------------------------------------------------------------------ |
| [00-p0-llm-layer.md](llm-pipeline/00-p0-llm-layer.md)             | 🔴 P0    | 1-3   | LLM Response Caching, Enhanced Retry/Reask, Fallback Provider Chains                             |
| [01-p1-text-processing.md](llm-pipeline/01-p1-text-processing.md) | 🟡 P1    | 4-7   | Text Chunking, Text Cleaning, Prompt Templates, Token Counting                                   |
| [02-p0-vector-store.md](llm-pipeline/02-p0-vector-store.md)       | 🔴 P0    | 8-9   | In-Memory Vector Store, Vector Math                                                              |
| [03-p2-resilience.md](llm-pipeline/03-p2-resilience.md)           | 🟢 P2    | 10-15 | Rate Limiting, Generic Retry, llm/summarize, llm/compare, Persistent KV Store, Document Metadata |

## Dependency Map

```
Tasks 1-7, 10-14: fully independent
Task 8 (vector store): independent (uses existing llm/embed format)
Task 9 (vector math): independent
Task 15 (document metadata): depends on Task 4 (text chunking)
```

## Test Commands

```bash
cargo test                                          # All tests
cargo test -p sema --test integration_test          # Integration tests only
cargo test -p sema --test integration_test -- name  # Single test
make lint                                           # Lint
cargo test -p sema-llm                              # sema-llm unit tests
cargo test -p sema-stdlib                           # sema-stdlib unit tests
```

## Key Codebase References

| What                             | Where                                                             |
| -------------------------------- | ----------------------------------------------------------------- |
| LLM builtins registration        | `crates/sema-llm/src/builtins.rs:482` — `register_llm_builtins()` |
| `do_complete` (dispatch + retry) | `crates/sema-llm/src/builtins.rs:2680`                            |
| `track_usage` (budget tracking)  | `crates/sema-llm/src/builtins.rs:135`                             |
| Thread-local state               | `crates/sema-llm/src/builtins.rs:26-39`                           |
| `reset_runtime_state`            | `crates/sema-llm/src/builtins.rs:57`                              |
| `llm/extract` (existing retry)   | `crates/sema-llm/src/builtins.rs:1157`                            |
| `validate_extraction`            | `crates/sema-llm/src/builtins.rs:2623`                            |
| `format_schema`                  | `crates/sema-llm/src/builtins.rs:2593`                            |
| `llm/embed` + embeddings         | `crates/sema-llm/src/builtins.rs:1963`                            |
| `llm/similarity` (cosine)        | `crates/sema-llm/src/builtins.rs:2021`                            |
| `extract_float_vec`              | `crates/sema-llm/src/builtins.rs:2516`                            |
| Provider registry                | `crates/sema-llm/src/provider.rs`                                 |
| LLM types                        | `crates/sema-llm/src/types.rs`                                    |
| Stdlib registration              | `crates/sema-stdlib/src/lib.rs:28` — `register_stdlib()`          |
| Stdlib `register_fn` helper      | `crates/sema-stdlib/src/lib.rs:73`                                |
| Stdlib `register_fn_gated`       | `crates/sema-stdlib/src/lib.rs:54`                                |
| Integration tests                | `crates/sema/tests/integration_test.rs`                           |
| Value type + accessors           | `crates/sema-core/src/value.rs`                                   |
| Error constructors               | `crates/sema-core/src/error.rs`                                   |

## Conventions

- **Naming:** slash-namespaced (`text/chunk`, `llm/cache-clear`), predicates end `?`
- **Errors:** `SemaError::eval("msg")`, `::type_error("expected", "got")`, `::arity("fn", "expected", got)`, `::Llm("msg".into())`, `::Io("msg".into())`
- **Registration:** `register_fn(env, "name", |args| { ... })` for simple fns, `register_fn_gated(env, sandbox, Caps::X, ...)` for capability-gated
- **Maps:** `BTreeMap<Value, Value>` with `Value::keyword("key")` keys
- **Tests:** `fn eval(input: &str) -> Value { Interpreter::new().eval_str(input).unwrap() }`
- **Thread-locals:** reset in `reset_runtime_state()` for test isolation

## Total Scope

~15 tasks, ~70+ integration tests, 3 new files (`text.rs`, `vector_store.rs`, `kv.rs`), substantial additions to `builtins.rs`.
