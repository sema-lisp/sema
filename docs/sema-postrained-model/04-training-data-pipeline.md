# 04 — Training Data Pipeline

How to transform Sema codebase artifacts into training-ready JSONL datasets for Fireworks
fine-tuning (OpenAI-compatible chat completion format).

---

## 1. Target Format

Fireworks uses OpenAI-compatible JSONL. Each line is one training example:

```jsonl
{"messages": [{"role": "system", "content": "..."}, {"role": "user", "content": "..."}, {"role": "assistant", "content": "..."}]}
```

We need several **categories** of training examples, each teaching different skills:

| Category | Teaches | Example Count (est.) | Source |
|----------|---------|---------------------|--------|
| Code completion | "Complete this Sema code" | 500+ | Example files |
| Evaluation | "What does X evaluate to?" | 1,095 | eval_tests! cases |
| API reference | "How does function X work?" | 1,156 | sema-docs entries |
| Code generation | "Write a program that does X" | 300+ | Example files + descriptions |
| Explanation | "Explain this Sema code" | 200+ | Example files + LLM generation |
| Error debugging | "Fix this error" | 141 | Error hints + test cases |
| Translation | "Translate from Clojure/Scheme" | 100+ | veteran_hint table + manual |
| Syntax reference | "What's the syntax for X?" | 200+ | Website docs |
| **Total** | | **~3,700+** | |

---

## 2. Extraction Scripts

### 2.1 Extract eval_tests! Cases

The `eval_tests!` macro in test files contains `$input => $expected` pairs. These are the
highest-value training data — ground-truth input→output mappings.

**Source files**:
- `crates/sema/tests/eval_test.rs`
- `crates/sema/tests/eval_collections_test.rs`
- `crates/sema/tests/eval_stdlib_test.rs`
- `crates/sema/tests/eval_core_test.rs`
- `crates/sema/tests/eval_map_test.rs`
- `crates/sema/tests/eval_data_test.rs`
- `crates/sema/tests/eval_types_test.rs`
- `crates/sema/tests/eval_ergonomic_test.rs`

**Extraction approach**: Parse each `eval_tests!` block with a regex or simple parser to
extract `(input, expected)` pairs. The macro format is:

```rust
eval_tests! {
    "(+ 1 2)" => 3,
    "(map (fn (x) (* x 2)) '(1 2 3))" => (2 4 6),
    // ...
}
```

**Conversion to JSONL**:

```jsonl
{"messages": [{"role": "system", "content": "You are a Sema Lisp evaluator. Given a Sema expression, return its evaluated result."}, {"role": "user", "content": "What does (+ 1 2) evaluate to in Sema?"}, {"role": "assistant", "content": "3"}]}
{"messages": [{"role": "system", "content": "You are a Sema Lisp evaluator."}, {"role": "user", "content": "Evaluate: (map (fn (x) (* x 2)) '(1 2 3))"}, {"role": "assistant", "content": "(2 4 6)"}]}
```

**Estimated yield**: ~1,095 training pairs.

### 2.2 Extract sema-docs API Entries

The `sema-docs` crate generates 826 .md files with per-function documentation and `; =>`
executable examples.

**Source**: `crates/sema-docs/` output (or the source files that generate them).

**Extraction approach**: Parse each .md file to extract:
- Function name
- Description
- Signature
- Example(s) with `; =>` expected output

**Conversion to JSONL**:

```jsonl
{"messages": [{"role": "system", "content": "You are a Sema programming assistant with knowledge of the Sema standard library."}, {"role": "user", "content": "How does string/trim work in Sema? Show an example."}, {"role": "assistant", "content": "string/trim removes leading and trailing whitespace from a string.\n\nSignature: (string/trim s)\n\nExample:\n(string/trim \"  hello  \")\n; => \"hello\""}]}
```

**Estimated yield**: ~1,156 training pairs (one per executable example).

### 2.3 Extract Example Programs

224 unique .sema files covering all language features.

**Extraction approach**: For each example file, generate multiple training pairs:

1. **Code → Description**: Use the filename and any comments to generate "Write a program
   that does X" → file content
2. **Description → Code**: Use an LLM to generate a description, then pair as
   "Write X in Sema" → code
3. **Code completion**: Take the first N lines, ask to complete
4. **Explanation**: "Explain this Sema code" → line-by-line explanation

**Conversion to JSONL**:

```jsonl
{"messages": [{"role": "system", "content": "You are a Sema programming assistant."}, {"role": "user", "content": "Write a Fibonacci function in Sema using named let for tail recursion."}, {"role": "assistant", "content": "(define (fib n)\n  (let loop ((i n) (a 0) (b 1))\n    (if (= i 0)\n        a\n        (loop (- i 1) b (+ a b)))))"}]}
```

**Estimated yield**: ~500-800 training pairs (multiple per file).

### 2.4 Extract Playground Examples

68 curated, self-contained examples in `playground/examples/`.

**Extraction approach**: Each playground example is categorized (getting-started, functional,
data, http, patterns, concurrency, etc.). Use the category + filename as the "prompt" and the
file content as the "completion".

**Estimated yield**: ~200 training pairs.

### 2.5 Extract Website Documentation Code Blocks

1,024 `sema` code blocks across 72 documentation files.

**Extraction approach**: Parse each .md file, extract code blocks with surrounding context
as the prompt. The surrounding text explains what the code does.

**Estimated yield**: ~400 training pairs (filtering for substantive blocks).

### 2.6 Extract Error Hints

141 `with_hint()` calls + the `veteran_hint` table.

**Extraction approach**: For each error + hint pair, create a training example:

```jsonl
{"messages": [{"role": "system", "content": "You are a Sema programming assistant helping debug errors."}, {"role": "user", "content": "I got this error: Unbound variable: defn"}, {"role": "assistant", "content": "Sema uses 'defun' to define named functions. Try:\n(defun my-func (args...) body...)"}]}
```

**Estimated yield**: ~141 training pairs.

### 2.7 Generate Synthetic Pairs with an LLM

Use an existing strong model (GLM 5.2 serverless, Claude) to generate additional training
data:

| Generation Strategy | Input | Output | Pairs |
|---------------------|-------|--------|-------|
| Code variations | Existing example | Rewritten version | 200+ |
| Clojure→Sema translation | Clojure snippet | Sema equivalent | 100+ |
| Scheme→Sema translation | Scheme snippet | Sema equivalent | 100+ |
| Bug injection | Correct code | Buggy code + error | 100+ |
| Refactoring | Verbose code | Idiomatic version | 100+ |
| Test generation | Function code | Test cases | 100+ |
| **Total** | | | **700+** |

**Cost**: Using GLM 5.2 serverless at $1.40/$4.40 per M tokens, generating 700 pairs at
~500 tokens each = ~700K tokens = ~$3.00 total.

---

## 3. System Prompt for Training Data

Include a consistent system prompt across all training examples to establish the model's
role:

```
You are a Sema programming assistant. Sema is a Lisp dialect with LLM primitives, implemented
in Rust. Key features:
- S-expressions with slash-namespaced stdlib (string/trim, map/get, list/filter)
- Special forms: define, fn, lambda, let, let*, letrec, if, cond, match, try/catch, defmacro
- Short lambdas: #(+ % 1) — % is %1, %2 for multi-arg
- F-strings: f"hello ${name}"
- Regex literals: #"pattern"
- Vectors [...], maps {:key val}, lists (...)
- Threading macros: ->, ->>, as->, some->
- Pattern matching with :keys destructuring and when guards
- Async: (async ...), (await ...), channels
- LLM primitives: prompt, message, deftool, defagent
- NaN-boxed values, single-threaded (Rc not Arc)
- Predicates end with ?, conversions use -> (string->symbol)
```

This same system prompt should be used at inference time for consistency.

---

## 4. Data Quality Controls

### Deduplication

- Ignore historical references to the removed `benchmarks/1brc/sema-src/` snapshot
- Deduplicate identical training pairs across extraction sources
- Ensure no test data leaks into training (hold out 10% for evaluation)

### Filtering

- Skip trivially short examples (< 20 characters)
- Skip examples with syntax errors (validate by running through the Sema reader)
- Ensure balanced coverage of features (not all list operations, not all string operations)

### Validation

- For eval_tests! pairs: verify the expected output by running through the Sema VM
- For sema-docs examples: verify `; =>` outputs are correct
- For generated code: run through `cargo test` to ensure validity

---

## 5. Dataset Splits

| Split | Purpose | Size |
|-------|---------|------|
| **Training** | Fine-tuning | 90% (~3,300 pairs) |
| **Evaluation** | Held-out validation | 10% (~370 pairs) |
| **RFT grader set** | Reward function input | 200 pairs (from eval_tests!) |

### RFT Grader Function

For reinforcement fine-tuning, the grader receives the model's output and returns a score.
For Sema, the grader would:

```python
def sema_grader(prompt: str, completion: str, expected: str) -> float:
    """Grade a Sema code generation by executing it."""
    try:
        # 1. Parse the completion as Sema code
        # 2. Execute through the Sema VM (via subprocess or FFI)
        result = run_sema(completion)
        # 3. Compare output to expected
        if result == expected:
            return 1.0  # Correct
        elif result is not None:
            return 0.3  # Ran but wrong output
        else:
            return 0.0  # Failed to execute
    except ParseError:
        return 0.0  # Syntax error
    except RuntimeError:
        return 0.0  # Runtime error
```

This requires either:
- A subprocess call to `sema -e "(eval ...)"` 
- A Rust FFI binding to the Sema VM
- A HTTP endpoint wrapping the Sema VM (could use Sema's own `http/serve`)

---

## 6. Dataset Creation Script (Pseudocode)

```python
#!/usr/bin/env python3
"""Generate Sema training datasets from the codebase."""

import json, re, pathlib

REPO = pathlib.Path("/Users/helge/code/sema-lisp")
OUTPUT = pathlib.Path("training_data.jsonl")

SYSTEM_PROMPT = "You are a Sema programming assistant. ..."

def write_pair(messages, file=OUTPUT):
    file.open("a").write(json.dumps({"messages": messages}) + "\n")

# 1. Extract eval_tests! cases
for test_file in REPO.glob("crates/sema/tests/eval_*.rs"):
    text = test_file.read_text()
    for match in re.finditer(r'"([^"]+)"\s*=>\s*([^,]+)', text):
        input_expr, expected = match.group(1), match.group(2).strip()
        write_pair([
            {"role": "system", "content": SYSTEM_PROMPT},
            {"role": "user", "content": f"Evaluate this Sema expression: {input_expr}"},
            {"role": "assistant", "content": expected},
        ])

# 2. Extract sema-docs entries
for doc_file in REPO.glob("crates/sema-docs/**/*.md"):
    # Parse function name, description, examples
    ...

# 3. Extract example programs
for example in REPO.glob("examples/**/*.sema"):
    # Generate description from filename + comments
    ...

# 4. Extract playground examples
for example in REPO.glob("playground/examples/**/*.sema"):
    ...

# 5. Extract website docs code blocks
for doc in REPO.glob("website/docs/**/*.md"):
    # Extract ```sema blocks with context
    ...

print(f"Generated {count} training pairs")
```

---

## 7. Estimated Final Dataset

| Source | Pairs | Avg tokens/pair | Total tokens |
|--------|-------|-----------------|-------------|
| eval_tests! cases | 1,095 | 200 | 219,000 |
| sema-docs entries | 1,156 | 300 | 346,800 |
| Example programs | 600 | 800 | 480,000 |
| Playground examples | 200 | 500 | 100,000 |
| Website docs blocks | 400 | 400 | 160,000 |
| Error hints | 141 | 300 | 42,300 |
| Synthetic (LLM-generated) | 700 | 500 | 350,000 |
| **Total** | **~4,292** | **~400 avg** | **~1,698,000** |

With 2 epochs of training: ~3.4M training tokens.
At $0.50/M tokens (LoRA SFT on <16B model): **~$1.70 training cost**.
At $3.00/M tokens (LoRA SFT on GLM 5.1): **~$10.20 training cost**.
RFT on Qwen3-8B: **Free**.
