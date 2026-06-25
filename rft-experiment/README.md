# Sema RFT Experiment

Fine-tune Qwen3-8B on Sema via free Reinforcement Fine-Tuning (RFT) on Fireworks,
then benchmark it against GLM 5.2 (serverless, no fine-tuning) and Claude on real
Sema coding tasks.

**Branch**: `experiment/rft-qwen3-sema`
**Estimated cost**: ~$15-25 (benchmark API calls only; training is free)
**Timeline**: 3-5 days

---

## Quick Start

```bash
# 1. Build the sema binary (needed for grading)
cargo build

# 2. Extract training data from eval_tests! cases
cd rft-experiment
python3 extract_eval_tests.py

# 3. Prepare RFT problem set and SFT dataset
python3 prepare_rft_problems.py

# 4. Generate benchmark task set
python3 prepare_benchmark.py

# 5. Run the grader server (for Fireworks RFT)
python3 grader.py --server --port 8080
# Expose with: ngrok http 8080

# 6. Launch RFT on Fireworks (see instructions below)
# ... after training completes ...

# 7. Run the benchmark (compares 3+ models)
export FIREWORKS_API_KEY=...
export ANTHROPIC_API_KEY=...
python3 benchmark.py --models glm-5.2-serverless,claude,qwen3-8b-rft
```

---

## Prerequisites

### 1. Python 3.11+
```bash
python3 --version
```

### 2. Sema binary (for grading)
```bash
cargo build  # from the repo root
```
This produces `target/debug/sema`. The grader uses `sema eval --json` to execute
generated code and check correctness.

### 3. Fireworks account + firectl
```bash
pip install firectl
firectl configure  # set your API key
```

### 4. API keys (for benchmarking)
```bash
export FIREWORKS_API_KEY=...        # for GLM 5.2 serverless + fine-tuned model
export ANTHROPIC_API_KEY=...        # for Claude comparison
export FIREWORKS_ACCOUNT_ID=...     # for dedicated deployment
```

### 5. ngrok (to expose grader to Fireworks RFT)
```bash
# Install ngrok: https://ngrok.com
ngrok http 8080
```

---

## Step-by-Step Instructions

### Step 1: Extract Training Data

```bash
cd rft-experiment
python3 extract_eval_tests.py
```

This parses the 8 `eval_tests!` test files in the codebase, extracts ~1,000
input→expected-output pairs, and runs each input through `sema eval --json`
to get the printed result.

Output:
- `data/eval_pairs.jsonl` — (test_name, input, expected) triples
- `data/eval_error_pairs.jsonl` — error test cases

### Step 2: Prepare RFT Problem Set

```bash
python3 prepare_rft_problems.py
```

Filters and splits the eval pairs into:
- `data/rft_problems.jsonl` — 200-500 problems for RFT training (90%)
- `data/sft_dataset.jsonl` — Same data in OpenAI chat format for SFT
- `data/eval_holdout.jsonl` — Held-out 10% for benchmarking

### Step 3: Generate Benchmark Tasks

```bash
python3 prepare_benchmark.py
```

Creates `data/benchmark_tasks.jsonl` with 60 Sema coding tasks across 5 difficulty
levels (L1: trivial evaluation, L2: write a function, L3: multi-feature, L4: full
program, L5: macros/async/lazy).

### Step 4: Build and Run the Grader

The grader is an HTTP endpoint that Fireworks RFT calls to score model completions.

```bash
# Start the grader server
python3 grader.py --server --port 8080

# In another terminal, expose it to the internet
ngrok http 8080

# Test it
curl -X POST http://localhost:8080 \
  -H "Content-Type: application/json" \
  -d '{"problem": {"input": "(+ 1 2)", "expected": "3"}, "completion": "3"}'
# → {"score": 1.0, "detail": "correct", "result": {...}}
```

### Step 5: Launch RFT on Fireworks

```bash
# Upload the problem set
firectl dataset create sema-rft-problems data/rft_problems.jsonl

# Launch RFT job (FREE for Qwen3-8B, which is <16B parameters)
firectl rftj create \
  --base-model qwen3-8b \
  --dataset sema-rft-problems \
  --output-model sema-qwen8b-rft-v1 \
  --grader-url https://YOUR_NGROK_URL.ngrok.io

# Monitor progress
firectl rftj get sema-qwen8b-rft-v1
```

Or use the Fireworks web UI at https://app.fireworks.ai/dashboard/fine-tuning/create
and select "Reinforcement Fine-Tuning" → `qwen3-8b` (look for "Free tuning" filter).

### Step 6: Deploy the Fine-Tuned Model

After RFT completes:

```bash
# Deploy as a dedicated (on-demand) deployment
firectl deployment create "accounts/YOUR_ACCOUNT_ID/models/sema-qwen8b-rft-v1"

# Verify it's running
firectl deployment get sema-qwen8b-rft-v1
```

Note: This costs $7/hr (H100 GPU). Only keep it running during benchmarking.

### Step 7: Run the Benchmark

```bash
# Test with just 5 tasks first (to verify it works)
python3 benchmark.py --models glm-5.2-serverless --limit 5

# Full benchmark with all models
export FIREWORKS_API_KEY=...
export ANTHROPIC_API_KEY=...
export FIREWORKS_ACCOUNT_ID=...

python3 benchmark.py \
  --models glm-5.2-serverless,claude,qwen3-8b-rft \
  --output results

# View results
cat results/summary.md
```

### Step 8 (Optional): Self-Host with Ollama

```bash
# Export the LoRA adapter from Fireworks
firectl model get sema-qwen8b-rft-v1

# Create an Ollama Modelfile
cat > Modelfile <<EOF
FROM qwen3-8b
ADAPTER /path/to/sema-lora-adapter
SYSTEM "$(cat system_prompt.txt)"
EOF

# Build and run
ollama create sema-qwen8b-rft-v1 -f Modelfile

# Benchmark the local model too
export OLLAMA_BASE_URL=http://localhost:11434
export OLLAMA_MODEL=sema-qwen8b-rft-v1
python3 benchmark.py --models ollama-local --output results
```

---

## File Structure

```
rft-experiment/
├── README.md                    # This file
├── system_prompt.txt            # Sema language reference for all models
├── execution-plan.md            # Detailed execution plan (in docs/)
├── extract_eval_tests.py        # Parse eval_tests! → JSONL
├── prepare_rft_problems.py      # Split into RFT problems + SFT dataset + holdout
├── prepare_benchmark.py         # Generate 60 benchmark tasks
├── grader.py                    # Grade completions via sema eval (HTTP server or CLI)
├── benchmark.py                 # Run N models on benchmark tasks
├── data/
│   ├── eval_pairs.jsonl         # Extracted (input, expected) pairs
│   ├── eval_error_pairs.jsonl   # Error test cases
│   ├── rft_problems.jsonl       # RFT training problems
│   ├── sft_dataset.jsonl        # SFT training dataset
│   ├── eval_holdout.jsonl       # Held-out eval pairs (not in training)
│   └── benchmark_tasks.jsonl    # 60 benchmark tasks
└── results/
    ├── benchmark_results.json   # Raw benchmark results
    └── summary.md               # Markdown comparison table
```

---

## Cost Breakdown

| Item | Cost | Notes |
|------|------|-------|
| RFT training (Qwen3-8B) | **$0** | Free for <16B on Fireworks |
| Grader server | $0 | Run locally with ngrok |
| GLM 5.2 API (benchmark, ~60 tasks) | ~$5 | 60 × ~50K tokens × $1.40/M |
| Claude API (benchmark, ~60 tasks) | ~$15 | 60 × ~50K tokens × $3/M + $15/M output |
| Fireworks deployment (benchmark, ~3hr) | ~$21 | 3hr × $7/hr H100 |
| **Total (with Claude)** | **~$41** | |
| **Total (without Claude)** | **~$26** | |
| **Total (serverless only, no dedicated)** | **~$5** | Skip the fine-tuned model benchmark |

---

## Expected Results

The benchmark produces a table like:

| Level | GLM 5.2 (no FT) | Qwen3-8B (RFT) | Claude Sonnet |
|-------|:-:|:-:|:-:|
| L1: Trivial | 90-95% | 95-100% | 85-95% |
| L2: Simple | 60-75% | 80-90% | 70-85% |
| L3: Medium | 40-60% | 55-70% | 55-70% |
| L4: Complex | 25-40% | 35-50% | 40-55% |
| L5: Advanced | 15-30% | 20-35% | 25-40% |
| **Overall** | **~55%** | **~65%** | **~60%** |

**Hypothesis**: The fine-tuned Qwen3-8B will beat GLM 5.2 on L1-L2 (syntax is baked
in) but lose on L4-L5 (less reasoning capacity than a 743B model). Claude should
be competitive on L3-L5 due to strong general reasoning.

The interesting question is whether RFT's correctness feedback loop pushes Qwen3-8B
above GLM 5.2 on L3 (medium complexity) — that's where fine-tuning should shine.
