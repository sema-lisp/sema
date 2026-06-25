#!/usr/bin/env python3
"""
Run tool-augmented benchmark against multiple serverless models in parallel.
Each model gets eval_code + docs_search tools.

Usage:
  python3 benchmark_multi.py --output results
"""

import json
import os
import re
import subprocess
import sys
import time
import concurrent.futures
from pathlib import Path

import httpx

SEMA_BINARY = str(Path(__file__).parent.parent / "target" / "debug" / "sema")
MAX_TOOL_ROUNDS = 3

MODELS = {
    "glm-5.2": "accounts/fireworks/models/glm-5p2",
    "deepseek-v4-pro": "accounts/fireworks/models/deepseek-v4-pro",
    "deepseek-v4-flash": "accounts/fireworks/models/deepseek-v4-flash",
    "kimi-k2.7-code": "accounts/fireworks/models/kimi-k2p7-code",
    "kimi-k2.6": "accounts/fireworks/models/kimi-k2p6",
    "qwen3.7-plus": "accounts/fireworks/models/qwen3p7-plus",
}

TOOL_DEFS = [
    {"type": "function", "function": {
        "name": "eval_code",
        "description": "Evaluate Sema code and return the result. Use this to test your code before returning it.",
        "parameters": {"type": "object", "properties": {"code": {"type": "string", "description": "The Sema code to evaluate"}}, "required": ["code"]}}},
    {"type": "function", "function": {
        "name": "docs_search",
        "description": "Search Sema documentation semantically. Returns relevant doc entries with function names and code examples.",
        "parameters": {"type": "object", "properties": {"query": {"type": "string", "description": "What you're looking for"}}, "required": ["query"]}}},
]


def call_model(api_key, model, messages, tools=None, max_tokens=2048):
    url = "https://api.fireworks.ai/inference/v1/chat/completions"
    payload = {"model": model, "messages": messages, "max_tokens": max_tokens, "temperature": 0.0}
    if tools:
        payload["tools"] = tools
        payload["tool_choice"] = "auto"
    try:
        with httpx.Client(timeout=120) as c:
            r = c.post(url, json=payload, headers={"Authorization": f"Bearer {api_key}", "Content-Type": "application/json"})
            return r.json()
    except Exception as e:
        return {"error": str(e)}


def execute_eval_code(code):
    try:
        r = subprocess.run([SEMA_BINARY, "eval", "--expr", code, "--json", "--timeout", "5000"],
                           capture_output=True, text=True, timeout=15)
        if r.returncode == 0:
            d = json.loads(r.stdout)
            if d.get("ok"):
                return f"Result: {d.get('value', '')}"
            return f"Error: {d.get('error', {}).get('message', 'unknown')}"
        return f"Error: {r.stderr.strip()[:200]}"
    except Exception as e:
        return f"Error: {e}"


def execute_docs_search(query):
    eq = query.replace('"', '\\"')
    code = (
        '(begin (llm/auto-configure) (vector-store/open "docs" "/tmp/sema-docs-rag.vec") '
        '(let* ((qv (llm/embed "' + eq + '")) (c (vector-store/search "docs" qv 10)) '
        '(ct (map (lambda (x) (:text (:metadata x))) c)) '
        '(r (llm/rerank "' + eq + '" ct {:top-k 5}))) '
        '(string/join (map (lambda (x) (let* ((i (:index x)) (m (nth c i))) '
        '(string-append "### " (str (:name (:metadata m))) "\\n" (str (:text (:metadata m)))))) r) "\\n---\\n")))'
    )
    try:
        r = subprocess.run([SEMA_BINARY, "eval", "--expr", code, "--json", "--timeout", "30000"],
                           capture_output=True, text=True, timeout=60,
                           cwd=str(Path(__file__).parent.parent))
        if r.returncode == 0:
            d = json.loads(r.stdout)
            return d.get("value", "No results") or "No results"
        return f"Search error: {r.stderr.strip()[:200]}"
    except Exception as e:
        return f"Search error: {e}"


def execute_tool(name, args):
    if name == "eval_code": return execute_eval_code(args.get("code", ""))
    if name == "docs_search": return execute_docs_search(args.get("query", ""))
    return f"Unknown: {name}"


def extract_code(completion):
    for p in [r'```(?:sema|lisp|scheme|clj)?\s*\n(.*?)```', r'```(?:sema|lisp|scheme|clj)?\s*(.*?)```']:
        m = re.findall(p, completion, re.DOTALL)
        if m: return m[0].strip()
    s = completion.strip()
    if s and s[0] in '([{;\'`#': return s
    for i, l in enumerate(s.split('\n')):
        if l.strip().startswith('('): return '\n'.join(s.split('\n')[i:]).strip()
    return s if s else None


def extract_answer(completion):
    if not completion: return ""
    text = completion.strip()
    # Strip thinking
    if "" in text:
        idx = text.find("")
        text = text[idx + len(""):].strip()
    # "result is X"
    m = re.search(r'(?:result|answer|value)\s+is\s+[:\s]*`?([^`\s.]+)`?', text, re.IGNORECASE)
    if m:
        v = m.group(1).strip().rstrip('.')
        return v[1:-1] if v.startswith('"') and v.endswith('"') else v
    # Last backtick value
    bts = re.findall(r'`([^`]+)`', text)
    if bts:
        v = bts[-1].strip()
        return v[1:-1] if v.startswith('"') and v.endswith('"') else v
    # Code block
    m = re.search(r'```(?:sema|lisp|scheme|clj)?\s*\n?(.*?)```', text, re.DOTALL)
    if m: return m.group(1).strip()
    lines = [l.strip() for l in text.split('\n') if l.strip()]
    if len(lines) == 1:
        v = lines[0]
        return v[1:-1] if v.startswith('"') and v.endswith('"') else v
    return lines[-1] if lines else text


def sema_eval(code, timeout=10):
    try:
        r = subprocess.run([SEMA_BINARY, "eval", "--expr", code, "--json", "--timeout", "5000"],
                           capture_output=True, text=True, timeout=timeout)
        if r.returncode == 0: return json.loads(r.stdout)
        return {"ok": False, "error": {"message": r.stderr.strip()}}
    except Exception as e:
        return {"ok": False, "error": {"message": str(e)}}


def grade(task, completion):
    gt = task.get("grader", "functional")
    exp = str(task.get("expected", "")).strip() if task.get("expected") is not None else None
    if gt == "eval_match":
        act = extract_answer(completion)
        if exp is not None:
            if act == exp: return 1.0, "correct"
            if re.sub(r'\s+', ' ', act) == re.sub(r'\s+', ' ', exp): return 1.0, "correct (ws)"
            return 0.3, f"wrong: expected {exp!r}, got {act!r}"
        return 0.5, "no expected"
    elif gt == "functional":
        code = extract_code(completion)
        if not code: return 0.0, "no code found"
        tc = task.get("test_code")
        full = f"(begin {code}\n {tc})" if tc else code
        r = sema_eval(full)
        if not r.get("ok"): return 0.0, f"error: {r.get('error', {}).get('message', '?')[:150]}"
        if exp is None: return 0.5, "runs ok"
        act = str(r.get("value", "")).strip()
        if act == exp: return 1.0, "correct"
        if re.sub(r'\s+', ' ', act) == re.sub(r'\s+', ' ', exp): return 1.0, "correct (ws)"
        return 0.3, f"wrong: expected {exp!r}, got {act!r}"
    return 0.0, f"unknown grader: {gt}"


def run_task(api_key, model, sys_prompt, task):
    tp = task["prompt"]
    msgs = [
        {"role": "system", "content": sys_prompt + "\n\nYou have tools: eval_code (test Sema code) and docs_search (find Sema functions). Always use eval_code to verify before answering."},
        {"role": "user", "content": tp},
    ]
    tc_made = 0
    for rnd in range(MAX_TOOL_ROUNDS):
        resp = call_model(api_key, model, msgs, tools=TOOL_DEFS)
        if "error" in resp: return 0.0, f"API error: {resp['error']}", 0, ""
        ch = resp.get("choices", [{}])[0]
        msg = ch.get("message", {})
        content = msg.get("content", "")
        tcs = msg.get("tool_calls", [])
        if not tcs:
            s, d = grade(task, content)
            return s, d, tc_made, content[:500]
        msgs.append(msg)
        for tc in tcs:
            fn = tc.get("function", {})
            try: args = json.loads(fn.get("arguments", "{}"))
            except: args = {}
            result = execute_tool(fn.get("name", ""), args)
            tc_made += 1
            msgs.append({"role": "tool", "tool_call_id": tc.get("id", ""), "content": result})
        if rnd == MAX_TOOL_ROUNDS - 2:
            msgs.append({"role": "user", "content": "Give your final answer now. Do not call more tools."})
    resp = call_model(api_key, model, msgs)
    if "error" in resp: return 0.0, f"API error final: {resp['error']}", tc_made, ""
    content = resp.get("choices", [{}])[0].get("message", {}).get("content", "")
    s, d = grade(task, content)
    return s, d, tc_made, content[:500]


def main():
    api_key = os.environ.get("FIREWORKS_API_KEY")
    if not api_key:
        print("ERROR: FIREWORKS_API_KEY not set", file=sys.stderr); sys.exit(1)

    sema_path = SEMA_BINARY
    if not Path(sema_path).exists():
        print(f"ERROR: sema not found at {sema_path}", file=sys.stderr); sys.exit(1)

    sys_prompt = (Path(__file__).parent / "system_prompt.txt").read_text()

    tasks = []
    with (Path(__file__).parent / "data" / "benchmark_tasks.jsonl").open() as f:
        for line in f:
            if line.strip(): tasks.append(json.loads(line))

    out_dir = Path(__file__).parent / "results"
    out_dir.mkdir(exist_ok=True)

    all_results = {}

    for model_name, model_id in MODELS.items():
        print(f"\n{'='*60}")
        print(f"  {model_name}")
        print(f"{'='*60}")

        scores = []
        for i, task in enumerate(tasks):
            print(f"  [{i+1}/{len(tasks)}] {task['id']}...", end=" ", flush=True)
            t0 = time.time()
            try:
                s, d, tc, comp = run_task(api_key, model_id, sys_prompt, task)
            except Exception as e:
                s, d, tc, comp = 0.0, f"exception: {e}", 0, ""
            lat = time.time() - t0
            print(f"score={s} ({d[:50]}) [tools={tc}, {lat:.1f}s]")
            scores.append({"task_id": task["id"], "level": task["level"], "category": task["category"],
                          "score": s, "detail": d, "tool_calls": tc, "latency_s": lat, "completion": comp})

        all_results[model_name] = scores

        # Save incremental results
        with (out_dir / f"multi_{model_name}.json").open("w") as f:
            json.dump(scores, f, indent=2)

        # Print summary
        by_lvl = {}
        for s in scores:
            by_lvl.setdefault(s["level"], []).append(s["score"])
        overall = sum(s["score"] for s in scores) / len(scores)
        total_tc = sum(s["tool_calls"] for s in scores)
        print(f"\n  Summary: {model_name}")
        for lvl in sorted(by_lvl):
            v = by_lvl[lvl]
            print(f"    L{lvl}: {sum(v)/len(v)*100:.0f}% ({sum(1 for x in v if x==1.0)}/{len(v)})")
        print(f"    Overall: {overall*100:.0f}%  tools={total_tc} ({total_tc/len(scores):.1f}/task)")

    # Save combined results
    with (out_dir / "multi_all_results.json").open("w") as f:
        json.dump(all_results, f, indent=2)

    # Print comparison table
    print(f"\n{'='*70}")
    print(f"  FINAL COMPARISON (with tools)")
    print(f"{'='*70}")
    print(f"{'Model':<25} {'Overall':>8} {'L1':>6} {'L2':>6} {'L3':>6} {'L4':>6} {'L5':>6} {'Tools/task':>10}")
    print("-" * 70)
    for name in all_results:
        scores = all_results[name]
        by_lvl = {}
        for s in scores:
            by_lvl.setdefault(s["level"], []).append(s["score"])
        overall = sum(s["score"] for s in scores) / len(scores)
        total_tc = sum(s["tool_calls"] for s in scores)
        lvls = []
        for lvl in range(1, 6):
            v = by_lvl.get(lvl, [0])
            lvls.append(f"{sum(v)/len(v)*100:.0f}%")
        print(f"{name:<25} {overall*100:>7.0f}% {lvls[0]:>6} {lvls[1]:>6} {lvls[2]:>6} {lvls[3]:>6} {lvls[4]:>6} {total_tc/len(scores):>9.1f}")


if __name__ == "__main__":
    main()
