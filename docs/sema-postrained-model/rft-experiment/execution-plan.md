# RFT Experiment — Execution Plan

Practical plan to fine-tune Qwen3-8B on Sema via free RFT on Fireworks, then benchmark it
against a non-fine-tuned frontier model and Claude on real Sema coding tasks.

**Branch**: `experiment/rft-qwen3-sema`
**Worktree**: `/Users/helge/code/sema-rft-experiment`
**Estimated cost**: ~$15-25 (benchmark API calls only; training is free)
**Timeline**: 3-5 days

---

## Overview

```
                    ┌─────────────────┐
                    │  Sema codebase  │
                    │  (eval tests,   │
                    │   examples,     │
                    │   docs)         │
                    └────────┬────────┘
                             │
                    ┌────────▼────────┐
                    │  Extraction     │
                    │  scripts        │
                    └────────┬────────┘
                             │
              ┌──────────────┼──────────────┐
              │              │              │
     ┌────────▼───┐  ┌──────▼──────┐  ┌────▼───────┐
     │ RFT        │  │ SFT dataset │  │ Benchmark  │
     │ problems   │  │ (JSONL)     │  │ tasks      │
     │ (200-500)  │  │ (3000+)     │  │ (50-100)   │
     └────────┬───┘  └─────────────┘  └────┬───────┘
              │                             │
     ┌────────▼───┐                  ┌─────▼──────┐
     │ Fireworks   │                  │ Benchmark  │
     │ RFT job     │                  │ runner     │
     │ (free,      │                  │ (3 models) │
     │  Qwen3-8B)  │                  └─────┬──────┘
     └────────┬───┘                        │
              │                    ┌───────┼───────┐
     ┌────────▼───┐               │       │       │
     │ Fine-tuned │          ┌────▼──┐ ┌──▼──┐ ┌──▼───┐
     │ Qwen3-8B   │          │GLM 5.2│ │Claude│ │Qwen3 │
     │ model      │          │+prompt│ │+prompt│ │FT    │
     └────────┬───┘          └───────┘ └──────┘ └──┬───┘
              │                              │
     ┌────────▼───┐                   ┌──────▼──────┐
     │ Ollama     │                   │ Results    │
     │ (local)    │                   │ comparison │
     └────────────┘                   └─────────────┘
```

---

## Step 1: Extract Training Data (Day 1)

### 1a. Parse eval_tests! cases → JSONL

**Script**: `rft-experiment/extract_eval_tests.py`

Parse the 8 eval test files:
- `crates/sema/tests/eval_test.rs`
- `crates/sema/tests/eval_core_test.rs`
- `crates/sema/tests/eval_collections_test.rs`
- `crates/sema/tests/eval_stdlib_test.rs`
- `crates/sema/tests/eval_map_test.rs`
- `crates/sema/tests/eval_data_test.rs`
- `crates/sema/tests/eval_types_test.rs`
- `crates/sema/tests/eval_ergonomic_test.rs`

**Challenge**: Expected outputs are Rust `Value::int(42)` expressions, not Sema printed forms.
We need to convert them. Two approaches:

1. **Run each input through `sema eval --json`** to get the Sema printed form of the output.
   This gives us `(input, printed_output)` pairs directly. The eval_tests already pass, so
   the VM output IS the expected value.

2. **Parse the Rust Value constructors** — more complex, but doesn't require running sema.

**Approach 1 is simpler and more reliable.** The extraction script will:
1. Parse eval_tests! blocks to extract `(test_name, input_string)` pairs
2. Run each input through `sema eval --expr "$input" --json`
3. Record the `value` field as the expected output
4. Write JSONL training pairs

**Output**: `rft-experiment/data/eval_pairs.jsonl`

```jsonl
{"input": "(+ 1 2)", "expected": "3", "category": "arithmetic"}
{"input": "(map (fn (x) (* x 2)) '(1 2 3))", "expected": "(2 4 6)", "category": "list"}
{"input": "(get {:a 1 :b 2} :a)", "expected": "1", "category": "map"}
```

### 1b. Extract example programs → instruction pairs

**Script**: `rft-experiment/extract_examples.py`

For each .sema file in `examples/` and `playground/examples/`:
1. Read the file
2. Extract any comments at the top as "description"
3. Use the filename as a category tag
4. Generate instruction-tuning pairs:
   - "Write a Sema program that [description]" → file content
   - "Explain this Sema code: [first 20 lines]" → explanation (skip if no comments)

**Output**: `rft-experiment/data/example_pairs.jsonl`

### 1c. Extract sema-docs API examples

**Script**: `rft-experiment/extract_api_docs.py`

Search for `; =>` patterns in documentation files:
```bash
rg '; =>' --glob '*.md' -n /Users/helge/code/sema-rft-experiment/
```

Parse each into `(function_name, example_code, expected_output)` triples.

**Output**: `rft-experiment/data/api_pairs.jsonl`

### 1d. Combine into final datasets

- **RFT problems**: 200-500 eval_tests! pairs (for RL training with grader)
- **SFT dataset**: ~3000+ pairs (for optional supervised fine-tuning)
- **Benchmark tasks**: 50-100 held-out tasks (for evaluation, NOT training)

**Important**: Hold out 10% of eval_tests! cases for benchmarking — don't train on them.

---

## Step 2: Build the Grader (Day 1)

### 2a. Sema eval wrapper

**Script**: `rft-experiment/grader.py`

The grader receives a problem (prompt + expected output) and a model completion, then:
1. Extract Sema code from the completion
2. Run it through `sema eval --expr "$code" --json --timeout 5000`
3. Parse the JSON response
4. Return a score:

```python
def grade(problem: dict, completion: str) -> float:
    code = extract_sema_code(completion)
    if code is None:
        return 0.0  # No valid code found

    result = run_sema_eval(code)

    if not result["ok"]:
        return 0.0  # Runtime/parse error

    if result["value"] == problem["expected"]:
        return 1.0  # Correct output

    return 0.3  # Ran but wrong output
```

### 2b. Grader endpoint (for Fireworks remote agent RFT)

For Fireworks RFT in "remote agents" mode, the grader needs to be an HTTP endpoint:

```python
# rft-experiment/grader_server.py
from http.server import HTTPServer, BaseHTTPRequestHandler
import json, subprocess

class GraderHandler(BaseHTTPRequestHandler):
    def do_POST(self):
        data = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        score = grade(data["problem"], data["completion"])
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(json.dumps({"score": score}).encode())
```

Run locally with ngrok, or on a small VPS ($5/month).

---

## Step 3: Run RFT on Fireworks (Day 2-3)

### 3a. Prepare RFT problem set

Format: JSONL with `prompt` and `expected` fields:

```jsonl
{"prompt": "Evaluate this Sema expression: (+ 1 2)", "expected": "3"}
{"prompt": "Write a Sema function that reverses a list", "expected": "(define (reverse lst) (foldl (fn (acc x) (cons x acc)) '() lst))"}
```

### 3b. Launch RFT job

```bash
# Install firectl
pip install firectl

# Upload problem set
firectl dataset create sema-rft-problems data/rft_problems.jsonl

# Launch RFT job (free for Qwen3-8B, <16B)
firectl rftj create \
  --base-model qwen3-8b \
  --dataset sema-rft-problems \
  --output-model sema-qwen8b-rft-v1 \
  --grader-url https://your-grader.ngrok.io/grade
```

### 3c. Monitor and iterate

- Watch training progress in Fireworks dashboard
- Check reward curves (should trend upward)
- If model doesn't converge, adjust:
  - Problem difficulty (start with simpler tasks)
  - Grader strictness (allow partial credit)
  - Number of problems (add more if underfitting)

---

## Step 4: Build Benchmark Suite (Day 2)

### 4a. Task categories

50-100 Sema coding tasks across 5 difficulty levels:

| Level | Count | Description | Example |
|-------|-------|-------------|---------|
| L1: Trivial | 15 | Single expression evaluation | "What does (map (fn (x) (* x 2)) '(1 2 3)) evaluate to?" |
| L2: Simple | 15 | Write a small function | "Write a function that checks if a number is prime" |
| L3: Medium | 15 | Multi-feature program | "Write a JSON API handler that returns user data by ID" |
| L4: Complex | 10 | Full program | "Write a web server with 3 routes: GET /users, POST /users, DELETE /users/:id" |
| L5: Advanced | 5 | LLM/async/macro | "Write a defmacro that implements a for/list comprehension" |

**Source**: Held-out eval_tests! cases (L1), hand-written tasks (L2-L5), adapted from examples/ (L3-L4).

### 4b. Task format

```jsonl
{"id": "L1-001", "level": 1, "prompt": "Evaluate: (+ 1 2)", "expected": "3", "grader": "eval_match"}
{"id": "L2-001", "level": 2, "prompt": "Write a Sema function that returns the nth fibonacci number using named let for TCO", "expected": "(define (fib n) (let loop ((i n) (a 0) (b 1)) (if (= i 0) a (loop (- i 1) b (+ a b)))))", "grader": "eval_match"}
{"id": "L2-002", "level": 2, "prompt": "Write a Sema function that checks if a string is a palindrome", "expected": null, "grader": "functional"}
```

For L1-L2: exact output match via `sema eval --json`
For L3-L5: functional grading (does it parse? does it run? does it produce correct output for test inputs?)

### 4c. Benchmark tasks file

**Output**: `rft-experiment/data/benchmark_tasks.jsonl`

---

## Step 5: Run the Benchmark (Day 3-4)

### 5a. System prompt

**File**: `rft-experiment/system_prompt.txt`

A comprehensive Sema language reference (~1500 tokens) covering:
- Syntax overview (s-expressions, special forms, literals)
- Short lambdas, f-strings, regex literals
- Stdlib naming conventions (slash-namespaced, predicates end ?, arrows for conversions)
- Common patterns (threading macros, match with :keys, async channels)
- 3 example programs

### 5b. Benchmark runner

**Script**: `rft-experiment/benchmark.py`

Runs each benchmark task against 3 models:

1. **GLM 5.2 serverless** (Fireworks, with system prompt, no fine-tuning)
2. **Fine-tuned Qwen3-8B** (Fireworks dedicated deployment, with system prompt)
3. **Claude Sonnet** (Anthropic API, with same system prompt)

For each task:
- Send the prompt + system prompt to the model
- Get the completion
- Grade it with the appropriate grader
- Record: model, task_id, level, score, completion, latency

### 5c. Results format

**Output**: `rft-experiment/results/benchmark_results.json`

```json
{
  "models": {
    "glm-5.2-serverless": {"scores": [...], "avg_by_level": {...}, "total_avg": 0.75},
    "qwen3-8b-rft": {"scores": [...], "avg_by_level": {...}, "total_avg": 0.85},
    "claude-sonnet": {"scores": [...], "avg_by_level": {...}, "total_avg": 0.80}
  }
}
```

### 5d. Results summary

**Output**: `rft-experiment/results/summary.md`

A markdown table comparing the three models:

| Level | GLM 5.2 (no FT) | Qwen3-8B (RFT) | Claude Sonnet |
|-------|:-:|:-:|:-:|
| L1: Trivial | 95% | 98% | 92% |
| L2: Simple | 70% | 85% | 78% |
| L3: Medium | 55% | 65% | 68% |
| L4: Complex | 40% | 45% | 55% |
| L5: Advanced | 30% | 35% | 42% |
| **Overall** | **58%** | **66%** | **67%** |

*(These are illustrative — actual numbers TBD)*

---

## Step 6: Self-Host the Fine-Tuned Model (Day 4-5, Optional)

### 6a. Export the LoRA adapter from Fireworks

```bash
firectl model get sema-qwen8b-rft-v1
# Download the LoRA adapter weights
```

### 6b. Run locally with Ollama

```bash
# Create Modelfile
cat > Modelfile <<EOF
FROM qwen3-8b
ADAPTER /path/to/sema-lora-adapter
SYSTEM "$(cat system_prompt.txt)"
EOF

ollama create sema-qwen8b-rft-v1 -f Modelfile
ollama run sema-qwen8b-rft-v1
```

### 6c. Benchmark the local model too

Add a 4th column to the benchmark: Qwen3-8B RFT running locally via Ollama.

---

## Cost Breakdown

| Item | Cost | Notes |
|------|------|-------|
| RFT training (Qwen3-8B, free) | $0 | Fireworks free tier for <16B |
| Grader endpoint | $0 | Run locally with ngrok |
| GLM 5.2 API calls (benchmark, ~100 tasks) | ~$5 | 100 × 50K tokens × $1.40/M input + output |
| Claude API calls (benchmark, ~100 tasks) | ~$15 | 100 × 50K tokens × $3/M input + $15/M output |
| Fireworks deployment for benchmark (2-3 hrs) | ~$21 | 3hr × $7/hr H100 |
| **Total** | **~$41** | |

If we skip Claude to save money: **~$26**.
If we also skip the dedicated deployment and only benchmark serverless models: **~$5**.

---

## File Structure

```
rft-experiment/
├── README.md                    # Full instructions
├── system_prompt.txt            # Sema language reference for all models
├── extract_eval_tests.py        # Parse eval_tests! → JSONL
├── extract_examples.py          # Parse .sema examples → JSONL
├── extract_api_docs.py          # Parse sema-docs → JSONL
├── prepare_rft_problems.py      # Combine + format RFT problem set
├── prepare_sft_dataset.py       # Combine + format SFT dataset
├── prepare_benchmark.py         # Create held-out benchmark task set
├── grader.py                    # Grade a completion against expected
├── grader_server.py             # HTTP endpoint for Fireworks RFT
├── benchmark.py                 # Run 3 models on benchmark tasks
├── results_to_summary.py        # Generate markdown summary table
├── data/
│   ├── eval_pairs.jsonl         # Raw extracted eval_tests! pairs
│   ├── example_pairs.jsonl      # Raw extracted example pairs
│   ├── api_pairs.jsonl          # Raw extracted API doc pairs
│   ├── rft_problems.jsonl       # Final RFT problem set (200-500)
│   ├── sft_dataset.jsonl        # Final SFT dataset (3000+)
│   └── benchmark_tasks.jsonl    # Held-out benchmark tasks (50-100)
└── results/
    ├── benchmark_results.json   # Raw results
    └── summary.md               # Markdown comparison table
```
