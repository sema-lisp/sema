# Embedding Bytevector Representation — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Change `llm/embed` to return bytevector-encoded f64 vectors instead of `List<Float>`, achieving 2x memory reduction and 4x faster similarity computation.

**Status:** Implemented

**Architecture:** `llm/embed` returns `Value::Bytevector` with IEEE 754 LE-encoded f64s. `llm/similarity` gains a fast path for bytevectors (fused single-pass cosine) while maintaining backward compatibility with lists. New accessor functions (`embedding/length`, `embedding/ref`, `embedding/->list`) provide typed access.

**Tech Stack:** Rust, sema-core (Value::Bytevector), sema-llm (builtins), sema-stdlib (length support)

---

### Task 1: Make `length` work on bytevectors

Add `Value::Bytevector` arm to `length` in `list.rs` so `(length embedding)` returns byte count.
Later, `embedding/length` will return the logical f64 count.

### Task 2: Add embedding accessor functions to sema-llm builtins

- `embedding/length` — returns number of f64 elements (byte-length / 8)
- `embedding/ref` — returns f64 at index i
- `embedding/->list` — converts bytevector to list of floats
- `embedding/bytevector?` — predicate: bytevector whose length is divisible by 8

### Task 3: Change `llm/embed` to return bytevector

Convert `Vec<f64>` → LE bytes → `Value::Bytevector`.

### Task 4: Upgrade `llm/similarity` with fast path + backward compat

- Bytevector path: fused single-pass cosine (no extraction)
- List path: preserved for backward compat (existing user code)
- Mixed: error with helpful message

### Task 5: Add integration tests

### Task 6: Update all documentation and examples

### Task 7: Clean up benchmark file
