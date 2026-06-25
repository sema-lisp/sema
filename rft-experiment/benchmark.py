#!/usr/bin/env python3
"""
Benchmark runner — compare multiple models on the Sema coding task set.

Models:
  1. glm-5.2-serverless: GLM 5.2 on Fireworks serverless (with system prompt, no fine-tuning)
  2. qwen3-8b-rft: Fine-tuned Qwen3-8B on Fireworks dedicated deployment
  3. claude: Claude via Anthropic API (with system prompt)
  4. ollama-local: Fine-tuned model via Ollama (optional, for local testing)

Usage:
  python3 benchmark.py --tasks data/benchmark_tasks.jsonl --models glm-5.2-serverless,claude
  python3 benchmark.py --tasks data/benchmark_tasks.jsonl --models all --output results/

Requires API keys in environment:
  FIREWORKS_API_KEY - for Fireworks models
  ANTHROPIC_API_KEY - for Claude
"""

import argparse
import json
import os
import re
import subprocess
import sys
import time
from pathlib import Path

# ─── Model Clients ────────────────────────────────────────────────────────────

def call_fireworks_serverless(api_key: str, model: str, system_prompt: str, user_prompt: str, max_tokens: int = 2048) -> dict:
    """Call a Fireworks serverless model via OpenAI-compatible API."""
    import httpx
    url = "https://api.fireworks.ai/inference/v1/chat/completions"
    payload = {
        "model": model,
        "messages": [
            {"role": "system", "content": system_prompt},
            {"role": "user", "content": user_prompt},
        ],
        "max_tokens": max_tokens,
        "temperature": 0.0,
    }
    try:
        with httpx.Client(timeout=60) as client:
            resp = client.post(url, json=payload, headers={
                "Content-Type": "application/json",
                "Authorization": f"Bearer {api_key}",
            })
            result = resp.json()
            return {
                "content": result["choices"][0]["message"]["content"],
                "tokens_in": result["usage"]["prompt_tokens"],
                "tokens_out": result["usage"]["completion_tokens"],
                "latency_ms": 0,
            }
    except Exception as e:
        return {"content": "", "error": str(e), "tokens_in": 0, "tokens_out": 0, "latency_ms": 0}


def call_fireworks_dedicated(api_key: str, account_id: str, model: str, system_prompt: str, user_prompt: str, max_tokens: int = 2048) -> dict:
    """Call a Fireworks dedicated (on-demand) deployment."""
    import httpx
    url = "https://api.fireworks.ai/inference/v1/chat/completions"
    # If model looks like a deployment name (no /), use deployments/ path
    if "/" not in model:
        full_model = f"accounts/{account_id}/deployments/{model}"
    else:
        full_model = model
    payload = {
        "model": full_model,
        "messages": [
            {"role": "system", "content": system_prompt},
            {"role": "user", "content": user_prompt},
        ],
        "max_tokens": max_tokens,
        "temperature": 0.0,
    }
    try:
        with httpx.Client(timeout=120) as client:
            resp = client.post(url, json=payload, headers={
                "Content-Type": "application/json",
                "Authorization": f"Bearer {api_key}",
            })
            result = resp.json()
            return {
                "content": result["choices"][0]["message"]["content"],
                "tokens_in": result["usage"]["prompt_tokens"],
                "tokens_out": result["usage"]["completion_tokens"],
                "latency_ms": 0,
            }
    except Exception as e:
        return {"content": "", "error": str(e), "tokens_in": 0, "tokens_out": 0, "latency_ms": 0}


def call_anthropic(api_key: str, model: str, system_prompt: str, user_prompt: str, max_tokens: int = 2048) -> dict:
    """Call Claude via Anthropic API."""
    import httpx
    url = "https://api.anthropic.com/v1/messages"
    payload = {
        "model": model,
        "max_tokens": max_tokens,
        "system": system_prompt,
        "messages": [
            {"role": "user", "content": user_prompt},
        ],
    }
    try:
        with httpx.Client(timeout=60) as client:
            resp = client.post(url, json=payload, headers={
                "Content-Type": "application/json",
                "x-api-key": api_key,
                "anthropic-version": "2023-06-01",
            })
            result = resp.json()
            content = "".join(block.get("text", "") for block in result.get("content", []))
            return {
                "content": content,
                "tokens_in": result["usage"]["input_tokens"],
                "tokens_out": result["usage"]["output_tokens"],
                "latency_ms": 0,
            }
    except Exception as e:
        return {"content": "", "error": str(e), "tokens_in": 0, "tokens_out": 0, "latency_ms": 0}


def call_ollama(base_url: str, model: str, system_prompt: str, user_prompt: str, max_tokens: int = 2048) -> dict:
    """Call a model via Ollama (OpenAI-compatible endpoint)."""
    import httpx
    url = f"{base_url}/v1/chat/completions"
    payload = {
        "model": model,
        "messages": [
            {"role": "system", "content": system_prompt},
            {"role": "user", "content": user_prompt},
        ],
        "max_tokens": max_tokens,
        "temperature": 0.0,
        "stream": False,
    }
    try:
        with httpx.Client(timeout=120) as client:
            resp = client.post(url, json=payload, headers={"Content-Type": "application/json"})
            result = resp.json()
            return {
                "content": result["choices"][0]["message"]["content"],
                "tokens_in": result.get("usage", {}).get("prompt_tokens", 0),
                "tokens_out": result.get("usage", {}).get("completion_tokens", 0),
                "latency_ms": 0,
            }
    except Exception as e:
        return {"content": "", "error": str(e), "tokens_in": 0, "tokens_out": 0, "latency_ms": 0}


# ─── Model Registry ───────────────────────────────────────────────────────────

MODEL_CONFIGS = {
    "glm-5.2-serverless": {
        "name": "GLM 5.2 (serverless, no FT)",
        "call": lambda sp, up, mt: call_fireworks_serverless(
            os.environ["FIREWORKS_API_KEY"], "accounts/fireworks/models/glm-5p2", sp, up, mt),
    },
    "qwen3-8b-rft": {
        "name": "Qwen3-8B (RFT fine-tuned)",
        "call": lambda sp, up, mt: call_fireworks_dedicated(
            os.environ["FIREWORKS_API_KEY"],
            os.environ.get("FIREWORKS_ACCOUNT_ID", "helge-sverre-99daaa"),
            "sema-rft-h100",  # deployment name
            sp + " /no_think", up, mt),
    },
    "claude": {
        "name": "Claude Sonnet (no FT)",
        "call": lambda sp, up, mt: call_anthropic(
            os.environ["ANTHROPIC_API_KEY"], "claude-sonnet-4-20250514", sp, up, mt)
            if os.environ.get("ANTHROPIC_API_KEY") else {"content": "", "error": "ANTHROPIC_API_KEY not set", "tokens_in": 0, "tokens_out": 0, "latency_ms": 0},
    },
    "ollama-local": {
        "name": "Qwen3-8B RFT (local)",
        "call": lambda sp, up, mt: call_ollama(
            os.environ.get("OLLAMA_BASE_URL", "http://localhost:8001"),
            os.environ.get("OLLAMA_MODEL", "sema-qwen8b-rft-v1"),
            sp, up, mt),
    },
}


# ─── Grading ──────────────────────────────────────────────────────────────────

def find_sema_binary() -> str | None:
    repo_root = Path(__file__).parent.parent
    candidates = [
        repo_root / "target" / "debug" / "sema",
        repo_root / "target" / "release" / "sema",
    ]
    for c in candidates:
        if c.exists():
            return str(c)
    return None


def sema_eval(sema_path: str, code: str, timeout: int = 10) -> dict:
    try:
        result = subprocess.run(
            [sema_path, "eval", "--expr", code, "--json", "--timeout", "5000", "--no-llm"],
            capture_output=True, text=True, timeout=timeout,
        )
        if result.returncode == 0:
            return json.loads(result.stdout)
        return {"ok": False, "error": {"message": result.stderr.strip()}}
    except Exception as e:
        return {"ok": False, "error": {"message": str(e)}}


def extract_sema_code(completion: str) -> str | None:
    fence_patterns = [r'```(?:sema|lisp|scheme|clj)?\s*\n(.*?)```', r'```(?:sema|lisp|scheme|clj)?\s*(.*?)```']
    for pattern in fence_patterns:
        matches = re.findall(pattern, completion, re.DOTALL)
        if matches:
            return matches[0].strip()
    stripped = completion.strip()
    if stripped and stripped[0] in '([{;\'`#':
        return stripped
    for i, line in enumerate(stripped.split('\n')):
        if line.strip().startswith('('):
            return '\n'.join(stripped.split('\n')[i:]).strip()
    return stripped if stripped else None


def extract_answer_value(completion: str) -> str:
    """Extract the answer value from a model completion for eval-match tasks.
    
    Handles models that wrap answers in markdown, quotes, explanations, or thinking traces.
    """
    if not completion:
        return ""
    text = completion.strip()

    # Strip Qwen3 thinking traces:  Müd...  
    if "" in text:
        think_end = text.find("")
        text = text[think_end + len(""):].strip()
    elif text.startswith(""):
        # Thinking started but no closing tag — take after first newline
        lines = text.split("\n")
        if len(lines) > 1:
            text = "\n".join(lines[1:]).strip()

    # Try code blocks first — the answer is inside
    code_match = re.search(r'```(?:sema|lisp|scheme|clj)?\s*\n?(.*?)```', text, re.DOTALL)
    if code_match:
        return code_match.group(1).strip()

    # Try backtick-wrapped values: `30` or `"hi"`
    bt_match = re.search(r'`([^`]+)`', text)
    if bt_match:
        val = bt_match.group(1).strip()
        if val.startswith('"') and val.endswith('"'):
            val = val[1:-1]
        return val

    # Try "**Result: X**" or "result is X"
    result_match = re.search(r'(?:result|answer|output)\s*(?:is|:)\s*\*{0,2}(.+?)\*{0,2}(?:\n|$)', text, re.IGNORECASE)
    if result_match:
        val = result_match.group(1).strip().rstrip('*')
        if val.startswith('"') and val.endswith('"'):
            val = val[1:-1]
        return val

    # If single line or very short, return as-is
    lines = [l.strip() for l in text.split('\n') if l.strip()]
    if len(lines) == 1:
        val = lines[0]
        if val.startswith('"') and val.endswith('"'):
            val = val[1:-1]
        return val

    # Last resort: first non-empty line
    return lines[0] if lines else text


def grade_task(task: dict, completion: str, sema_path: str | None) -> dict:
    """Grade a benchmark task. Returns {score, detail}."""
    if sema_path is None:
        return {"score": 0.0, "detail": "sema binary not found"}

    grader_type = task.get("grader", "functional")
    expected = str(task.get("expected", "")).strip() if task.get("expected") is not None else None

    if grader_type == "eval_match":
        # The model outputs the *result value*, not code to execute.
        # Extract the answer and compare directly to expected.
        actual = extract_answer_value(completion)
        if expected is not None:
            if actual == expected:
                return {"score": 1.0, "detail": "correct"}
            if re.sub(r'\s+', ' ', actual) == re.sub(r'\s+', ' ', expected):
                return {"score": 1.0, "detail": "correct (whitespace-normalized)"}
            return {"score": 0.3, "detail": f"wrong: expected {expected!r}, got {actual!r}"}
        return {"score": 0.5, "detail": "no expected value to compare"}

    elif grader_type == "functional":
        # The code defines functions; then test_code is evaluated
        code = extract_sema_code(completion)
        if not code:
            return {"score": 0.0, "detail": "no Sema code found in completion"}
        # Combine: run the completion code, then evaluate test_code
        test_code = task.get("test_code")
        if test_code:
            # Wrap in a begin to define then evaluate
            full_code = f"(begin {code}\n {test_code})"
        else:
            full_code = code

        result = sema_eval(sema_path, full_code)
        if not result.get("ok"):
            return {"score": 0.0, "detail": f"error: {result.get('error', {}).get('message', 'unknown')[:200]}"}

        expected = task.get("expected")
        if expected is None:
            # No expected output — just check it runs
            return {"score": 0.5, "detail": "runs without error"}

        actual = str(result.get("value", "")).strip()
        expected = str(expected).strip()
        if actual == expected:
            return {"score": 1.0, "detail": "correct"}
        if re.sub(r'\s+', ' ', actual) == re.sub(r'\s+', ' ', expected):
            return {"score": 1.0, "detail": "correct (whitespace-normalized)"}
        return {"score": 0.3, "detail": f"wrong output: expected {expected!r}, got {actual!r}"}

    return {"score": 0.0, "detail": f"unknown grader type: {grader_type}"}


# ─── Main ─────────────────────────────────────────────────────────────────────

def main():
    parser = argparse.ArgumentParser(description="Benchmark models on Sema coding tasks")
    parser.add_argument("--tasks", default="data/benchmark_tasks.jsonl", help="Path to tasks JSONL")
    parser.add_argument("--models", default="all", help="Comma-separated model names or 'all'")
    parser.add_argument("--output", default="results", help="Output directory")
    parser.add_argument("--system-prompt", default="system_prompt.txt", help="System prompt file")
    parser.add_argument("--limit", type=int, default=None, help="Limit to N tasks (for testing)")
    parser.add_argument("--skip-grading", action="store_true", help="Skip grading, just collect completions")
    args = parser.parse_args()

    script_dir = Path(__file__).parent

    # Load tasks
    tasks = []
    with (script_dir / args.tasks).open() as f:
        for line in f:
            if line.strip():
                tasks.append(json.loads(line))
    if args.limit:
        tasks = tasks[:args.limit]

    # Load system prompt
    system_prompt = (script_dir / args.system_prompt).read_text()

    # Determine models
    if args.models == "all":
        model_names = list(MODEL_CONFIGS.keys())
    else:
        model_names = args.models.split(",")

    # Validate API keys
    for name in model_names:
        if name == "glm-5.2-serverless" and not os.environ.get("FIREWORKS_API_KEY"):
            print(f"WARNING: FIREWORKS_API_KEY not set — skipping {name}")
            model_names.remove(name)
        if name == "qwen3-8b-rft" and not os.environ.get("FIREWORKS_API_KEY"):
            print(f"WARNING: FIREWORKS_API_KEY not set — skipping {name}")
            model_names.remove(name)
        if name == "claude" and not os.environ.get("ANTHROPIC_API_KEY"):
            print(f"WARNING: ANTHROPIC_API_KEY not set — skipping {name}")
            model_names.remove(name)

    if not model_names:
        print("ERROR: No models to run. Set API keys in environment.", file=sys.stderr)
        sys.exit(1)

    # Find sema binary for grading
    sema_path = find_sema_binary() if not args.skip_grading else None
    if sema_path:
        print(f"Using sema binary: {sema_path}")
    elif not args.skip_grading:
        print("WARNING: sema binary not found — will skip grading")

    # Output dir
    out_dir = script_dir / args.output
    out_dir.mkdir(exist_ok=True)

    # Run benchmark
    results = {name: {"scores": [], "details": [], "completions": []} for name in model_names}

    for i, task in enumerate(tasks):
        print(f"\n[{i+1}/{len(tasks)}] {task['id']} (L{task['level']}, {task['category']})")
        print(f"  Prompt: {task['prompt'][:80]}...")

        for model_name in model_names:
            config = MODEL_CONFIGS[model_name]
            print(f"  → {config['name']}... ", end="", flush=True)

            t0 = time.time()
            try:
                response = config["call"](system_prompt, task["prompt"], 2048)
            except Exception as e:
                response = {"content": "", "error": str(e), "tokens_in": 0, "tokens_out": 0}

            latency = time.time() - t0

            if response.get("error"):
                print(f"ERROR: {response['error'][:60]}")
                score = 0.0
                detail = f"API error: {response['error'][:100]}"
            elif args.skip_grading or not sema_path:
                score = None
                detail = "not graded"
                print(f"OK (not graded)")
            else:
                grade_result = grade_task(task, response["content"], sema_path)
                score = grade_result["score"]
                detail = grade_result["detail"]
                print(f"score={score} ({detail[:50]})")

            results[model_name]["scores"].append({
                "task_id": task["id"],
                "level": task["level"],
                "category": task["category"],
                "score": score,
                "detail": detail,
                "latency_s": latency,
                "tokens_in": response.get("tokens_in", 0),
                "tokens_out": response.get("tokens_out", 0),
            })
            results[model_name]["completions"].append({
                "task_id": task["id"],
                "completion": response.get("content", ""),
            })

    # Save raw results
    raw_out = out_dir / "benchmark_results.json"
    with raw_out.open("w") as f:
        json.dump(results, f, indent=2)
    print(f"\nRaw results: {raw_out}")

    # Generate summary
    summary = generate_summary(results, model_names)
    summary_out = out_dir / "summary.md"
    with summary_out.open("w") as f:
        f.write(summary)
    print(f"Summary: {summary_out}")
    print(f"\n{summary}")


def generate_summary(results: dict, model_names: list[str]) -> str:
    """Generate a markdown summary table."""
    lines = ["# Benchmark Results\n"]
    lines.append(f"Models: {', '.join(model_names)}\n")

    # Overall scores
    lines.append("## Overall Scores\n")
    lines.append("| Model | Overall | L1 | L2 | L3 | L4 | L5 |")
    lines.append("|-------|:-:|:-:|:-:|:-:|:-:|:-:|")

    for name in model_names:
        scores = results[name]["scores"]
        total = [s["score"] for s in scores if s["score"] is not None]
        overall = sum(total) / len(total) if total else 0

        by_level = {}
        for s in scores:
            if s["score"] is not None:
                lvl = s["level"]
                by_level.setdefault(lvl, []).append(s["score"])

        level_avgs = []
        for lvl in range(1, 6):
            if lvl in by_level:
                level_avgs.append(f"{sum(by_level[lvl])/len(by_level[lvl])*100:.0f}%")
            else:
                level_avgs.append("—")

        display_name = MODEL_CONFIGS[name]["name"]
        lines.append(f"| {display_name} | {overall*100:.0f}% | {' | '.join(level_avgs)} |")

    # Category breakdown
    lines.append("\n## By Category\n")
    lines.append("| Model | " + " | ".join(sorted(set(s["category"] for sc in results.values() for s in sc["scores"]))) + " |")
    lines.append("|-------|" + "|".join(["---"] * len(set(s["category"] for sc in results.values() for s in sc["scores"]))) + "|")

    categories = sorted(set(s["category"] for sc in results.values() for s in sc["scores"]))
    for name in model_names:
        scores = results[name]["scores"]
        by_cat = {}
        for s in scores:
            if s["score"] is not None:
                cat = s["category"]
                by_cat.setdefault(cat, []).append(s["score"])
        cat_avgs = [f"{sum(by_cat[c])/len(by_cat[c])*100:.0f}%" if c in by_cat else "—" for c in categories]
        display_name = MODEL_CONFIGS[name]["name"]
        lines.append(f"| {display_name} | " + " | ".join(cat_avgs) + " |")

    # Token usage
    lines.append("\n## Token Usage\n")
    lines.append("| Model | Total Input | Total Output | Avg Latency |")
    lines.append("|-------|---:|---:|---:|")
    for name in model_names:
        scores = results[name]["scores"]
        total_in = sum(s["tokens_in"] for s in scores)
        total_out = sum(s["tokens_out"] for s in scores)
        avg_latency = sum(s["latency_s"] for s in scores) / len(scores) if scores else 0
        display_name = MODEL_CONFIGS[name]["name"]
        lines.append(f"| {display_name} | {total_in:,} | {total_out:,} | {avg_latency:.1f}s |")

    lines.append("\n## Detailed Results\n")
    for name in model_names:
        scores = results[name]["scores"]
        lines.append(f"\n### {MODEL_CONFIGS[name]['name']}\n")
        lines.append("| Task | Level | Category | Score | Detail |")
        lines.append("|------|--------|----------|:-:|------|")
        for s in scores:
            lines.append(f"| {s['task_id']} | L{s['level']} | {s['category']} | {s['score'] or '—'} | {s['detail'][:80]} |")

    return "\n".join(lines)


if __name__ == "__main__":
    main()
