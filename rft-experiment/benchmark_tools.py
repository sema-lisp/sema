#!/usr/bin/env python3
"""
Benchmark GLM 5.2 with MCP-style tool access (eval + docs_search).

Simulates the tool-call loop:
1. Send the task prompt + system prompt to GLM 5.2
2. If the model calls a tool (eval_code, docs_search), execute it and send the result back
3. Repeat until the model gives a final answer (no tool call) or max iterations reached
4. Grade the final answer

Tools available:
- eval_code(code): Run Sema code, return result or error
- docs_search(query): Semantic search over Sema docs
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

SEMA_BINARY = str(Path(__file__).parent.parent / "target" / "debug" / "sema")
MAX_TOOL_ROUNDS = 3  # Max back-and-forth tool calls before forcing a final answer

TOOL_DEFINITIONS = [
    {
        "type": "function",
        "function": {
            "name": "eval_code",
            "description": "Evaluate Sema code and return the result. Use this to test your code before returning it. Returns the evaluated value as a string, or an error message.",
            "parameters": {
                "type": "object",
                "properties": {
                    "code": {"type": "string", "description": "The Sema code to evaluate"}
                },
                "required": ["code"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "docs_search",
            "description": "Search Sema documentation semantically. Returns relevant doc entries with function names, descriptions, and code examples. Use this to find the right function or syntax.",
            "parameters": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "What you're looking for, e.g. 'reverse a list' or 'read file lines'"}
                },
                "required": ["query"],
            },
        },
    },
]


def call_glm_with_tools(api_key: str, messages: list, max_tokens: int = 2048, model: str = "accounts/fireworks/models/glm-5p2", force_tool: bool = False) -> dict:
    """Call a model with tool definitions."""
    url = "https://api.fireworks.ai/inference/v1/chat/completions"
    payload = {
        "model": model,
        "messages": messages,
        "max_tokens": max_tokens,
        "temperature": 0.0,
        "tools": TOOL_DEFINITIONS,
        "tool_choice": "required" if force_tool else "auto",
    }
    try:
        with httpx.Client(timeout=120) as client:
            resp = client.post(url, json=payload, headers={
                "Content-Type": "application/json",
                "Authorization": f"Bearer {api_key}",
            })
            result = resp.json()
            return result
    except Exception as e:
        return {"error": str(e)}


def call_glm_final(api_key: str, messages: list, max_tokens: int = 2048, model: str = "accounts/fireworks/models/glm-5p2") -> dict:
    """Call a model without tools (for final answer extraction)."""
    url = "https://api.fireworks.ai/inference/v1/chat/completions"
    payload = {
        "model": model,
        "messages": messages,
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
            return result
    except Exception as e:
        return {"error": str(e)}


# ─── Tool Execution ───────────────────────────────────────────────────────────

def execute_eval_code(code: str) -> str:
    """Run Sema code through the sema binary."""
    try:
        result = subprocess.run(
            [SEMA_BINARY, "eval", "--expr", code, "--json", "--timeout", "5000"],
            capture_output=True, text=True, timeout=15,
        )
        if result.returncode == 0:
            data = json.loads(result.stdout)
            if data.get("ok"):
                val = data.get("value", "")
                stdout = data.get("stdout", "")
                if stdout:
                    return f"Result: {val}\nStdout: {stdout}"
                return f"Result: {val}"
            else:
                err = data.get("error", {})
                return f"Error: {err.get('message', 'unknown')}"
        else:
            return f"Error: {result.stderr.strip()}"
    except subprocess.TimeoutExpired:
        return "Error: timeout (code took too long)"
    except Exception as e:
        return f"Error: {e}"


def execute_docs_search(query: str) -> str:
    """Search Sema docs via the sema binary with the vector store."""
    # Build the search code safely without f-strings
    escaped_query = query.replace('"', '\\"')
    search_code = (
        '(begin (llm/auto-configure) '
        '(vector-store/open "docs" "/tmp/sema-docs-rag.vec") '
        '(let* ((qv (llm/embed "' + escaped_query + '")) '
        '(candidates (vector-store/search "docs" qv 10)) '
        '(ctexts (map (lambda (c) (:text (:metadata c))) candidates)) '
        '(reranked (llm/rerank "' + escaped_query + '" ctexts {:top-k 5}))) '
        '(string/join (map (lambda (r) '
        '(let* ((idx (:index r)) (c (nth candidates idx)) '
        '(name (:name (:metadata c))) (text (:text (:metadata c)))) '
        '(string-append "### " (str name) "\\n" (str text)))) '
        'reranked) "\\n---\\n")))'
    )
    try:
        result = subprocess.run(
            [SEMA_BINARY, "eval", "--expr", search_code, "--json", "--timeout", "30000"],
            capture_output=True, text=True, timeout=60,
            cwd=str(Path(__file__).parent.parent),
        )
        if result.returncode == 0:
            data = json.loads(result.stdout)
            if data.get("ok"):
                return data.get("value", "No results") or "No results"
            else:
                return f"Search error: {data.get('error', {}).get('message', 'unknown')}"
        else:
            return f"Search error: {result.stderr.strip()[:200]}"
    except Exception as e:
        return f"Search error: {e}"


def execute_tool(name: str, arguments: dict) -> str:
    """Dispatch a tool call."""
    if name == "eval_code":
        return execute_eval_code(arguments.get("code", ""))
    elif name == "docs_search":
        return execute_docs_search(arguments.get("query", ""))
    return f"Unknown tool: {name}"


# ─── Grading (reused from benchmark.py) ───────────────────────────────────────

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
    if not completion:
        return ""
    text = completion.strip()

    # Strip Qwen3 thinking traces
    if "" in text:
        think_end = text.find("")
        text = text[think_end + len(""):].strip()

    # Try code blocks first
    code_match = re.search(r'```(?:sema|lisp|scheme|clj)?\s*\n?(.*?)```', text, re.DOTALL)
    if code_match:
        return code_match.group(1).strip()

    # Try "the result is X" pattern FIRST (before backtick matching)
    result_match = re.search(r'(?:result|answer|value)\s+is\s+[:\s]*`?([^`\s.]+)`?', text, re.IGNORECASE)
    if result_match:
        val = result_match.group(1).strip().rstrip('.')
        if val.startswith('"') and val.endswith('"'):
            val = val[1:-1]
        return val

    # Try backtick-wrapped values (last one is usually the answer)
    bt_matches = re.findall(r'`([^`]+)`', text)
    if bt_matches:
        val = bt_matches[-1].strip()  # Take the LAST backtick value
        if val.startswith('"') and val.endswith('"'):
            val = val[1:-1]
        return val

    # If single line, return as-is
    lines = [l.strip() for l in text.split('\n') if l.strip()]
    if len(lines) == 1:
        val = lines[0]
        if val.startswith('"') and val.endswith('"'):
            val = val[1:-1]
        return val

    # Last resort: last non-empty line (often the answer after explanation)
    return lines[-1] if lines else text


def find_sema_binary() -> str | None:
    candidates = [
        Path(__file__).parent.parent / "target" / "debug" / "sema",
        Path(__file__).parent.parent / "target" / "release" / "sema",
    ]
    for c in candidates:
        if c.exists():
            return str(c)
    return None


def sema_eval(sema_path: str, code: str, timeout: int = 10) -> dict:
    try:
        result = subprocess.run(
            [sema_path, "eval", "--expr", code, "--json", "--timeout", "5000"],
            capture_output=True, text=True, timeout=timeout,
        )
        if result.returncode == 0:
            return json.loads(result.stdout)
        return {"ok": False, "error": {"message": result.stderr.strip()}}
    except Exception as e:
        return {"ok": False, "error": {"message": str(e)}}


def grade_task(task: dict, completion: str, sema_path: str | None) -> dict:
    if sema_path is None:
        return {"score": 0.0, "detail": "sema binary not found"}
    grader_type = task.get("grader", "functional")
    expected = str(task.get("expected", "")).strip() if task.get("expected") is not None else None

    if grader_type == "eval_match":
        actual = extract_answer_value(completion)
        if expected is not None:
            if actual == expected:
                return {"score": 1.0, "detail": "correct"}
            if re.sub(r'\s+', ' ', actual) == re.sub(r'\s+', ' ', expected):
                return {"score": 1.0, "detail": "correct (whitespace-normalized)"}
            return {"score": 0.3, "detail": f"wrong: expected {expected!r}, got {actual!r}"}
        return {"score": 0.5, "detail": "no expected value"}

    elif grader_type == "functional":
        code = extract_sema_code(completion)
        if not code:
            return {"score": 0.0, "detail": "no Sema code found"}
        test_code = task.get("test_code")
        if test_code:
            full_code = f"(begin {code}\n {test_code})"
        else:
            full_code = code
        result = sema_eval(sema_path, full_code)
        if not result.get("ok"):
            err = result.get("error", {}).get("message", "unknown")
            return {"score": 0.0, "detail": f"error: {err[:150]}"}
        if expected is None:
            return {"score": 0.5, "detail": "runs without error"}
        actual = str(result.get("value", "")).strip()
        if actual == expected:
            return {"score": 1.0, "detail": "correct"}
        if re.sub(r'\s+', ' ', actual) == re.sub(r'\s+', ' ', expected):
            return {"score": 1.0, "detail": "correct (whitespace-normalized)"}
        return {"score": 0.3, "detail": f"wrong output: expected {expected!r}, got {actual!r}"}

    return {"score": 0.0, "detail": f"unknown grader: {grader_type}"}


# ─── Main Benchmark Loop ──────────────────────────────────────────────────────

def run_task_with_tools(api_key: str, system_prompt: str, task: dict, sema_path: str, model: str = "accounts/fireworks/models/glm-5p2", model_name: str = "GLM 5.2") -> dict:
    """Run a single task with tool-augmented model."""
    task_prompt = task["prompt"]
    # For Qwen3-8B RFT, add /no_think to disable thinking traces
    sys_content = system_prompt + "\n\nYou have access to tools: eval_code (test your Sema code) and docs_search (find Sema functions). Use eval_code to verify your code works before giving your final answer."
    if "qwen" in model.lower() or "rft" in model.lower():
        sys_content += " /no_think"
    messages = [
        {"role": "system", "content": sys_content},
        {"role": "user", "content": task_prompt},
    ]

    tool_calls_made = 0
    completions = []

    for round_num in range(MAX_TOOL_ROUNDS):
        # Force tool use on first round for smaller models that tend to skip tools
        force_tool = (round_num == 0 and ("qwen" in model.lower() or "rft" in model.lower()))
        response = call_glm_with_tools(api_key, messages, model=model, force_tool=force_tool)

        if "error" in response:
            return {"score": 0.0, "detail": f"API error: {response['error']}", "tool_calls": 0, "completion": ""}

        choice = response.get("choices", [{}])[0]
        message = choice.get("message", {})
        content = message.get("content", "")
        tool_calls = message.get("tool_calls", [])

        if not tool_calls:
            # No tool calls — this is the final answer
            completions.append(content)
            grade = grade_task(task, content, sema_path)
            return {
                "score": grade["score"],
                "detail": grade["detail"],
                "tool_calls": tool_calls_made,
                "completion": content[:500],
                "rounds": round_num + 1,
            }

        # Execute tool calls
        messages.append(message)  # Add assistant message with tool_calls

        for tc in tool_calls:
            func = tc.get("function", {})
            tool_name = func.get("name", "")
            try:
                args = json.loads(func.get("arguments", "{}"))
            except json.JSONDecodeError:
                args = {}

            tool_result = execute_tool(tool_name, args)
            tool_calls_made += 1

            # Add tool result to conversation
            messages.append({
                "role": "tool",
                "tool_call_id": tc.get("id", ""),
                "content": tool_result,
            })

        # If this is the last round, force a final answer without tools
        if round_num == MAX_TOOL_ROUNDS - 2:
            messages.append({
                "role": "user",
                "content": "Based on the tool results above, give your final answer now. Do not call any more tools.",
            })

    # If we exhausted rounds, get a final answer without tools
    response = call_glm_final(api_key, messages, model=model)
    if "error" in response:
        return {"score": 0.0, "detail": f"API error on final: {response['error']}", "tool_calls": tool_calls_made, "completion": ""}

    content = response.get("choices", [{}])[0].get("message", {}).get("content", "")
    completions.append(content)
    grade = grade_task(task, content, sema_path)
    return {
        "score": grade["score"],
        "detail": grade["detail"],
        "tool_calls": tool_calls_made,
        "completion": content[:500],
        "rounds": MAX_TOOL_ROUNDS + 1,
    }


def main():
    parser = argparse.ArgumentParser(description="Benchmark model with MCP-style tool access")
    parser.add_argument("--tasks", default="data/benchmark_tasks.jsonl")
    parser.add_argument("--output", default="results")
    parser.add_argument("--system-prompt", default="system_prompt.txt")
    parser.add_argument("--limit", type=int, default=None)
    parser.add_argument("--model", default="accounts/fireworks/models/glm-5p2", help="Model ID")
    parser.add_argument("--model-name", default="GLM 5.2 + tools", help="Display name")
    args = parser.parse_args()

    script_dir = Path(__file__).parent
    api_key = os.environ.get("FIREWORKS_API_KEY")
    if not api_key:
        print("ERROR: FIREWORKS_API_KEY not set", file=sys.stderr)
        sys.exit(1)

    sema_path = find_sema_binary()
    if not sema_path:
        print("ERROR: sema binary not found", file=sys.stderr)
        sys.exit(1)
    print(f"Using sema: {sema_path}")

    system_prompt = (script_dir / args.system_prompt).read_text()

    # Load tasks
    tasks = []
    with (script_dir / args.tasks).open() as f:
        for line in f:
            if line.strip():
                tasks.append(json.loads(line))
    if args.limit:
        tasks = tasks[:args.limit]

    out_dir = script_dir / args.output
    out_dir.mkdir(exist_ok=True)

    # Run benchmark
    scores = []
    for i, task in enumerate(tasks):
        print(f"\n[{i+1}/{len(tasks)}] {task['id']} (L{task['level']}, {task['category']})")
        print(f"  Prompt: {task['prompt'][:80]}...")

        t0 = time.time()
        result = run_task_with_tools(api_key, system_prompt, task, sema_path, model=args.model, model_name=args.model_name)
        latency = time.time() - t0

        score = result["score"]
        detail = result["detail"]
        tc = result["tool_calls"]
        print(f"  → {args.model_name}... score={score} ({detail[:60]}) [tools={tc}, {latency:.1f}s]")

        scores.append({
            "task_id": task["id"],
            "level": task["level"],
            "category": task["category"],
            "score": score,
            "detail": detail,
            "tool_calls": tc,
            "latency_s": latency,
            "completion": result.get("completion", ""),
        })

    # Save results
    raw_out = out_dir / "tool_augmented_results.json"
    with raw_out.open("w") as f:
        json.dump({"scores": scores}, f, indent=2)

    # Generate summary
    by_level = {}
    for s in scores:
        lvl = s["level"]
        by_level.setdefault(lvl, []).append(s["score"])

    total_tool_calls = sum(s["tool_calls"] for s in scores)

    print(f"\n{'='*60}")
    print(f"{args.model_name}")
    print(f"{'='*60}")
    print(f"Tasks: {len(scores)}")
    print(f"Total tool calls: {total_tool_calls} ({total_tool_calls/len(scores):.1f} per task)")
    print(f"\nBy level:")
    for lvl in sorted(by_level):
        vals = by_level[lvl]
        avg = sum(vals) / len(vals)
        correct = sum(1 for v in vals if v == 1.0)
        print(f"  L{lvl}: {avg*100:.0f}% ({correct}/{len(vals)} correct)")

    overall = sum(s["score"] for s in scores) / len(scores)
    print(f"\nOverall: {overall*100:.0f}%")
    print(f"\nResults: {raw_out}")


if __name__ == "__main__":
    main()
