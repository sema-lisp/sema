#!/usr/bin/env python3
"""
Benchmark a model against the 60-task Sema benchmark with tool augmentation.

Supports both local vLLM endpoints (Ornith) and Fireworks serverless API.
The endpoint is determined by OPENAI_BASE_URL env var or --base-url flag.

Usage:
  # Local Ornith via vLLM
  OPENAI_BASE_URL=http://localhost:8000/v1 OPENAI_API_KEY=EMPTY \
    python3 benchmark.py --model Ornith-1.0-9B --model-name "Ornith-9B bare"

  # Local Ornith + tools
  OPENAI_BASE_URL=http://localhost:8000/v1 OPENAI_API_KEY=EMPTY \
    python3 benchmark.py --model Ornith-1.0-9B --model-name "Ornith-9B + tools" --tools

  # Fireworks frontier model
  OPENAI_BASE_URL=https://api.fireworks.ai/inference/v1 \
    python3 benchmark.py --model accounts/fireworks/models/glm-5p2 --model-name "GLM 5.2" --tools

  # No tools (bare model)
  python3 benchmark.py --model Ornith-1.0-9B --model-name "Ornith-9B bare"
"""

import argparse
import json
import os
import re
import subprocess
import sys
import time
from pathlib import Path

import httpx

# ─── Config ───────────────────────────────────────────────────────────────────

SCRIPT_DIR = Path(__file__).parent
SEMA_BINARY = str(SCRIPT_DIR.parent / "target" / "debug" / "sema")
MAX_TOOL_ROUNDS = 5
DEFAULT_MAX_TOKENS = 4096

TOOL_DEFS = [
    {"type": "function", "function": {
        "name": "eval_code",
        "description": "Evaluate Sema code and return the result. Use this to test your code before returning it. Returns the evaluated value as a string, or an error message.",
        "parameters": {"type": "object", "properties": {
            "code": {"type": "string", "description": "The Sema code to evaluate"}}, "required": ["code"]}}},
    {"type": "function", "function": {
        "name": "docs_search",
        "description": "Search Sema documentation semantically. Returns relevant doc entries with function names, descriptions, and code examples. Use this to find the right function or syntax for a task.",
        "parameters": {"type": "object", "properties": {
            "query": {"type": "string", "description": "What you're looking for, e.g. 'reverse a list' or 'read file lines'"}}, "required": ["query"]}}},
]


# ─── API Call ─────────────────────────────────────────────────────────────────

def call_model(base_url, api_key, model, messages, tools=None, max_tokens=DEFAULT_MAX_TOKENS,
               temperature=0.0, top_p=1.0, force_tool=False):
    """Call an OpenAI-compatible chat completions endpoint."""
    url = f"{base_url.rstrip('/')}/chat/completions"
    payload = {
        "model": model,
        "messages": messages,
        "max_tokens": max_tokens,
        "temperature": temperature,
        "top_p": top_p,
    }
    if tools:
        payload["tools"] = tools
        payload["tool_choice"] = "required" if force_tool else "auto"

    headers = {"Content-Type": "application/json"}
    if api_key and api_key != "EMPTY":
        headers["Authorization"] = f"Bearer {api_key}"

    try:
        with httpx.Client(timeout=180) as client:
            resp = client.post(url, json=payload, headers=headers)
            result = resp.json()
            if "error" in result and "choices" not in result:
                return {"error": json.dumps(result["error"])}
            return result
    except httpx.TimeoutException:
        return {"error": "request timeout"}
    except Exception as e:
        return {"error": str(e)}


# ─── Tool Execution ───────────────────────────────────────────────────────────

def execute_eval_code(code):
    try:
        r = subprocess.run(
            [SEMA_BINARY, "eval", "--expr", code, "--json", "--timeout", "5000"],
            capture_output=True, text=True, timeout=15,
        )
        if r.returncode == 0:
            d = json.loads(r.stdout)
            if d.get("ok"):
                val = d.get("value", "")
                stdout = d.get("stdout", "")
                if stdout:
                    return f"Result: {val}\nStdout: {stdout}"
                return f"Result: {val}"
            err = d.get("error", {})
            return f"Error: {err.get('message', 'unknown')}"
        return f"Error: {r.stderr.strip()[:300]}"
    except subprocess.TimeoutExpired:
        return "Error: timeout (code took too long)"
    except Exception as e:
        return f"Error: {e}"


def execute_docs_search(query):
    eq = query.replace('"', '\\"')
    code = (
        '(begin (llm/auto-configure) (vector-store/open "docs" "/tmp/sema-docs-rag.vec") '
        '(let* ((qv (llm/embed "' + eq + '")) '
        '(candidates (vector-store/search "docs" qv 10)) '
        '(ctexts (map (lambda (c) (:text (:metadata c))) candidates)) '
        '(reranked (llm/rerank "' + eq + '" ctexts {:top-k 5}))) '
        '(string/join (map (lambda (r) '
        '(let* ((idx (:index r)) (c (nth candidates idx)) '
        '(name (:name (:metadata c))) (text (:text (:metadata c)))) '
        '(string-append "### " (str name) "\\n" (str text)))) '
        'reranked) "\\n---\\n")))'
    )
    try:
        r = subprocess.run(
            [SEMA_BINARY, "eval", "--expr", code, "--json", "--timeout", "30000"],
            capture_output=True, text=True, timeout=60,
            cwd=str(SCRIPT_DIR.parent),
        )
        if r.returncode == 0:
            d = json.loads(r.stdout)
            return d.get("value", "No results") or "No results"
        return f"Search error: {r.stderr.strip()[:200]}"
    except Exception as e:
        return f"Search error: {e}"


def execute_tool(name, args):
    if name == "eval_code":
        return execute_eval_code(args.get("code", ""))
    if name == "docs_search":
        return execute_docs_search(args.get("query", ""))
    return f"Unknown tool: {name}"


# ─── Grading ──────────────────────────────────────────────────────────────────

def extract_sema_code(completion):
    for p in [r'```(?:sema|lisp|scheme|clj)?\s*\n(.*?)```', r'```(?:sema|lisp|scheme|clj)?\s*(.*?)```']:
        m = re.findall(p, completion, re.DOTALL)
        if m:
            return m[0].strip()
    s = completion.strip()
    if s and s[0] in '([{;\'`#':
        return s
    for i, l in enumerate(s.split('\n')):
        if l.strip().startswith('('):
            return '\n'.join(s.split('\n')[i:]).strip()
    return s if s else None


def extract_answer_value(completion):
    if not completion:
        return ""
    text = completion.strip()
    # Strip reasoning traces (Ornith/Qwen3 <think>...</think> or similar)
    think_end = text.find("</think>")
    if think_end != -1:
        text = text[think_end + len("</think>"):].strip()

    # Try code blocks
    code_match = re.search(r'```(?:sema|lisp|scheme|clj)?\s*\n?(.*?)```', text, re.DOTALL)
    if code_match:
        return code_match.group(1).strip()

    # "the result is X" pattern
    result_match = re.search(r'(?:result|answer|value)\s+is\s+[:\s]*`?([^`\s.]+)`?', text, re.IGNORECASE)
    if result_match:
        val = result_match.group(1).strip().rstrip('.')
        if val.startswith('"') and val.endswith('"'):
            val = val[1:-1]
        return val

    # Backtick-wrapped values
    bt_matches = re.findall(r'`([^`]+)`', text)
    if bt_matches:
        val = bt_matches[-1].strip()
        if val.startswith('"') and val.endswith('"'):
            val = val[1:-1]
        return val

    lines = [l.strip() for l in text.split('\n') if l.strip()]
    if len(lines) == 1:
        val = lines[0]
        if val.startswith('"') and val.endswith('"'):
            val = val[1:-1]
        return val
    return lines[-1] if lines else text


def find_sema_binary():
    for c in [SCRIPT_DIR.parent / "target" / "debug" / "sema",
              SCRIPT_DIR.parent / "target" / "release" / "sema"]:
        if c.exists():
            return str(c)
    return None


def sema_eval(sema_path, code, timeout=10):
    try:
        r = subprocess.run(
            [sema_path, "eval", "--expr", code, "--json", "--timeout", "5000"],
            capture_output=True, text=True, timeout=timeout,
        )
        if r.returncode == 0:
            return json.loads(r.stdout)
        return {"ok": False, "error": {"message": r.stderr.strip()}}
    except Exception as e:
        return {"ok": False, "error": {"message": str(e)}}


def grade_task(task, completion, sema_path):
    if not sema_path:
        return 0.0, "sema binary not found"
    grader_type = task.get("grader", "functional")
    expected = str(task.get("expected", "")).strip() if task.get("expected") is not None else None

    if grader_type == "eval_match":
        actual = extract_answer_value(completion)
        if expected is not None:
            if actual == expected:
                return 1.0, "correct"
            if re.sub(r'\s+', ' ', actual) == re.sub(r'\s+', ' ', expected):
                return 1.0, "correct (whitespace-normalized)"
            return 0.3, f"wrong: expected {expected!r}, got {actual!r}"
        return 0.5, "no expected value"

    elif grader_type == "functional":
        code = extract_sema_code(completion)
        if not code:
            return 0.0, "no Sema code found"
        test_code = task.get("test_code")
        full_code = f"(begin {code}\n {test_code})" if test_code else code
        result = sema_eval(sema_path, full_code)
        if not result.get("ok"):
            err = result.get("error", {}).get("message", "unknown")
            return 0.0, f"error: {err[:150]}"
        if expected is None:
            return 0.5, "runs without error"
        actual = str(result.get("value", "")).strip()
        if actual == expected:
            return 1.0, "correct"
        if re.sub(r'\s+', ' ', actual) == re.sub(r'\s+', ' ', expected):
            return 1.0, "correct (whitespace-normalized)"
        return 0.3, f"wrong output: expected {expected!r}, got {actual!r}"

    return 0.0, f"unknown grader: {grader_type}"


# ─── Benchmark Loop ───────────────────────────────────────────────────────────

def run_task(base_url, api_key, model, system_prompt, task, sema_path,
             use_tools, temperature, top_p, is_reasoning):
    """Run a single benchmark task, optionally with tool calls."""
    task_prompt = task["prompt"]
    sys_content = system_prompt
    if use_tools:
        sys_content += ("\n\nYou have access to tools: eval_code (test your Sema code) "
                        "and docs_search (find Sema functions). "
                        "Use eval_code to verify your code works before giving your final answer.")

    messages = [
        {"role": "system", "content": sys_content},
        {"role": "user", "content": task_prompt},
    ]

    tool_calls_made = 0

    if not use_tools:
        # Single-shot, no tools
        resp = call_model(base_url, api_key, model, messages,
                          temperature=temperature, top_p=top_p)
        if "error" in resp:
            return 0.0, f"API error: {resp['error']}", 0, "", 0
        choice = resp.get("choices", [{}])[0]
        msg = choice.get("message", {})
        content = msg.get("content", "") or ""
        score, detail = grade_task(task, content, sema_path)
        return score, detail, 0, content[:500], 1

    rounds_done = 0
    for round_num in range(MAX_TOOL_ROUNDS):
        rounds_done = round_num + 1
        force_tool = (round_num == 0 and is_reasoning)
        resp = call_model(base_url, api_key, model, messages,
                          tools=TOOL_DEFS, temperature=temperature, top_p=top_p,
                          force_tool=force_tool)

        if "error" in resp:
            return 0.0, f"API error: {resp['error']}", tool_calls_made, "", rounds_done

        choice = resp.get("choices", [{}])[0]
        msg = choice.get("message", {})
        content = msg.get("content", "") or ""
        tool_calls = msg.get("tool_calls", [])

        if not tool_calls:
            score, detail = grade_task(task, content, sema_path)
            return score, detail, tool_calls_made, content[:500], rounds_done

        # Execute tool calls
        messages.append(msg)
        for tc in tool_calls:
            func = tc.get("function", {})
            tool_name = func.get("name", "")
            try:
                args = json.loads(func.get("arguments", "{}"))
            except json.JSONDecodeError:
                args = {}

            tool_result = execute_tool(tool_name, args)
            tool_calls_made += 1
            messages.append({
                "role": "tool",
                "tool_call_id": tc.get("id", ""),
                "content": tool_result,
            })

        if round_num == MAX_TOOL_ROUNDS - 2:
            messages.append({
                "role": "user",
                "content": "Based on the tool results above, give your final answer now. Do not call any more tools.",
            })

    # Force final answer without tools
    resp = call_model(base_url, api_key, model, messages,
                      temperature=temperature, top_p=top_p)
    if "error" in resp:
        return 0.0, f"API error on final: {resp['error']}", tool_calls_made, "", rounds_done
    content = resp.get("choices", [{}])[0].get("message", {}).get("content", "") or ""
    score, detail = grade_task(task, content, sema_path)
    return score, detail, tool_calls_made, content[:500], rounds_done


# ─── Main ─────────────────────────────────────────────────────────────────────

def main():
    parser = argparse.ArgumentParser(description="Benchmark model on Sema tasks")
    parser.add_argument("--tasks", default="data/benchmark_tasks.jsonl")
    parser.add_argument("--output", default="results")
    parser.add_argument("--system-prompt", default="system_prompt.txt")
    parser.add_argument("--limit", type=int, default=None)
    parser.add_argument("--model", required=True, help="Model ID")
    parser.add_argument("--model-name", required=True, help="Display name")
    parser.add_argument("--base-url", default=None, help="OpenAI-compatible base URL")
    parser.add_argument("--api-key", default=None, help="API key")
    parser.add_argument("--tools", action="store_true", help="Enable eval_code + docs_search tools")
    parser.add_argument("--temperature", type=float, default=0.6, help="Sampling temperature")
    parser.add_argument("--top-p", type=float, default=0.95, help="Top-p sampling")
    parser.add_argument("--reasoning", action="store_true", help="Model is a reasoning model (force tool on round 0)")
    parser.add_argument("--sema-path", default=None)
    args = parser.parse_args()

    base_url = args.base_url or os.environ.get("OPENAI_BASE_URL", "https://api.fireworks.ai/inference/v1")
    api_key = args.api_key or os.environ.get("OPENAI_API_KEY") or os.environ.get("FIREWORKS_API_KEY")
    if not api_key:
        api_key = "EMPTY"

    sema_path = args.sema_path or find_sema_binary()
    if not sema_path:
        print("ERROR: sema binary not found. Build with: cargo build", file=sys.stderr)
        sys.exit(1)
    print(f"Using sema: {sema_path}")
    print(f"Endpoint: {base_url}")
    print(f"Model: {args.model}")
    print(f"Tools: {'yes' if args.tools else 'no'}")
    print(f"Temperature: {args.temperature}, top_p: {args.top_p}")

    system_prompt = (SCRIPT_DIR / args.system_prompt).read_text()

    tasks = []
    with (SCRIPT_DIR / args.tasks).open() as f:
        for line in f:
            if line.strip():
                tasks.append(json.loads(line))
    if args.limit:
        tasks = tasks[:args.limit]

    out_dir = SCRIPT_DIR / args.output
    out_dir.mkdir(exist_ok=True)

    scores = []
    for i, task in enumerate(tasks):
        print(f"\n[{i+1}/{len(tasks)}] {task['id']} (L{task['level']}, {task['category']})")
        print(f"  Prompt: {task['prompt'][:80]}...")

        t0 = time.time()
        try:
            score, detail, tc, comp, rounds = run_task(
                base_url, api_key, args.model, system_prompt, task, sema_path,
                use_tools=args.tools, temperature=args.temperature,
                top_p=args.top_p, is_reasoning=args.reasoning,
            )
        except Exception as e:
            score, detail, tc, comp, rounds = 0.0, f"exception: {e}", 0, "", 0
        latency = time.time() - t0

        print(f"  -> {args.model_name}: score={score} ({detail[:60]}) [tools={tc}, rounds={rounds}, {latency:.1f}s]")

        scores.append({
            "task_id": task["id"],
            "level": task["level"],
            "category": task["category"],
            "score": score,
            "detail": detail,
            "tool_calls": tc,
            "rounds": rounds,
            "latency_s": latency,
            "completion": comp,
        })

    # Save results
    safe_name = args.model_name.replace(" ", "_").replace("+", "plus")
    raw_out = out_dir / f"benchmark_{safe_name}.json"
    with raw_out.open("w") as f:
        json.dump({"model": args.model, "model_name": args.model_name,
                   "tools": args.tools, "scores": scores}, f, indent=2)

    # Summary
    by_level = {}
    by_category = {}
    for s in scores:
        by_level.setdefault(s["level"], []).append(s["score"])
        by_category.setdefault(s["category"], []).append(s["score"])

    total_tc = sum(s["tool_calls"] for s in scores)
    total_rounds = sum(s["rounds"] for s in scores)
    overall = sum(s["score"] for s in scores) / len(scores)

    print(f"\n{'='*60}")
    print(f"{args.model_name}")
    print(f"{'='*60}")
    print(f"Tasks: {len(scores)}")
    if args.tools:
        print(f"Tool calls: {total_tc} ({total_tc/len(scores):.1f}/task)")
    print(f"\nBy level:")
    for lvl in sorted(by_level):
        v = by_level[lvl]
        avg = sum(v) / len(v)
        correct = sum(1 for x in v if x == 1.0)
        print(f"  L{lvl}: {avg*100:.0f}% ({correct}/{len(v)} correct)")
    print(f"\nOverall: {overall*100:.0f}%")
    print(f"\nResults: {raw_out}")


if __name__ == "__main__":
    main()
