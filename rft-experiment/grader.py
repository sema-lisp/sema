#!/usr/bin/env python3
"""
Grade a Sema code completion by executing it through the Sema VM.

Usage:
  python3 grader.py --problem problem.json --completion completion.txt
  python3 grader.py --input "(+ 1 2)" --expected "3" --completion "3"

Scoring:
  1.0 = correct output (matches expected)
  0.3  = ran successfully but wrong output
  0.0  = parse error, runtime error, or no code found

Can also be run as an HTTP server for Fireworks RFT remote agent mode:
  python3 grader.py --server --port 8080
"""

import argparse
import json
import re
import subprocess
import sys
from http.server import HTTPServer, BaseHTTPRequestHandler
from pathlib import Path


def find_sema_binary(repo_root: Path | None = None) -> str | None:
    """Find the sema binary."""
    if repo_root is None:
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
    """Run `sema eval --expr CODE --json` and return the parsed result."""
    try:
        result = subprocess.run(
            [sema_path, "eval", "--expr", code, "--json", "--timeout", "5000", "--no-llm"],
            capture_output=True,
            text=True,
            timeout=timeout,
        )
        if result.returncode == 0:
            return json.loads(result.stdout)
        return {"ok": False, "error": {"message": result.stderr.strip()}}
    except subprocess.TimeoutExpired:
        return {"ok": False, "error": {"message": "timeout"}}
    except json.JSONDecodeError:
        return {"ok": False, "error": {"message": f"invalid JSON: {result.stdout[:200]}"}}
    except FileNotFoundError:
        return {"ok": False, "error": {"message": f"sema binary not found at {sema_path}"}}
    except Exception as e:
        return {"ok": False, "error": {"message": str(e)}}


def extract_sema_code(completion: str) -> str | None:
    """Extract Sema code from a model completion.

    The model may wrap code in markdown fences (```sema ... ```) or just
    output raw code. Try to extract the code portion.
    """
    # Try markdown code blocks first
    fence_patterns = [
        r'```(?:sema|lisp|scheme|clj)?\s*\n(.*?)```',
        r'```(?:sema|lisp|scheme|clj)?\s*(.*?)```',
    ]
    for pattern in fence_patterns:
        matches = re.findall(pattern, completion, re.DOTALL)
        if matches:
            return matches[0].strip()

    # If the completion looks like raw Sema (starts with ( or [ or { or ;)
    stripped = completion.strip()
    if stripped and stripped[0] in '([{;\'`#':
        return stripped

    # If it contains a line that starts with (, take from there
    for i, line in enumerate(stripped.split('\n')):
        if line.strip().startswith('('):
            return '\n'.join(stripped.split('\n')[i:]).strip()

    # Last resort: return the whole thing and let the parser decide
    return stripped if stripped else None


def grade(problem: dict, completion: str, sema_path: str | None = None) -> dict:
    """Grade a completion against a problem.

    problem: {"input": "...", "expected": "..."} or {"prompt": "...", "expected": "..."}
    completion: raw model output string

    Returns: {"score": float, "detail": str, "result": dict}
    """
    if sema_path is None:
        sema_path = find_sema_binary()

    if not sema_path:
        return {"score": 0.0, "detail": "sema binary not found", "result": None}

    code = extract_sema_code(completion)
    if not code:
        return {"score": 0.0, "detail": "no Sema code found in completion", "result": None}

    # Determine what to evaluate
    # For eval-match problems: the code IS the expression to evaluate
    # For code-gen problems: the code is a full program, we may need to call a function
    eval_code = code
    expected = problem.get("expected")

    # If expected is None, we can't do exact match — just check if it parses/runs
    if expected is None:
        result = sema_eval(sema_path, eval_code)
        if result.get("ok"):
            return {"score": 0.5, "detail": "runs without error (no expected output to match)", "result": result}
        return {"score": 0.0, "detail": f"error: {result.get('error', {}).get('message', 'unknown')}", "result": result}

    # Exact match grading
    result = sema_eval(sema_path, eval_code)
    if not result.get("ok"):
        return {"score": 0.0, "detail": f"error: {result.get('error', {}).get('message', 'unknown')}", "result": result}

    actual = result.get("value", "")
    if actual is None:
        actual = "nil"

    # Normalize: strip whitespace, compare case-insensitively for symbols
    actual_norm = str(actual).strip()
    expected_norm = str(expected).strip()

    if actual_norm == expected_norm:
        return {"score": 1.0, "detail": "correct", "result": result}

    # Try fuzzy match: ignore whitespace differences
    actual_compact = re.sub(r'\s+', ' ', actual_norm)
    expected_compact = re.sub(r'\s+', ' ', expected_norm)
    if actual_compact == expected_compact:
        return {"score": 1.0, "detail": "correct (whitespace-normalized)", "result": result}

    return {"score": 0.3, "detail": f"ran but wrong output: expected {expected_norm!r}, got {actual_norm!r}", "result": result}


# ─── HTTP Server (for Fireworks RFT remote agent mode) ────────────────────────

class GraderHandler(BaseHTTPRequestHandler):
    sema_path = None

    def do_POST(self):
        content_length = int(self.headers.get('Content-Length', 0))
        body = self.rfile.read(content_length)

        try:
            data = json.loads(body)
            problem = data.get("problem", data)
            completion = data.get("completion", "")
            result = grade(problem, completion, self.sema_path)
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(json.dumps(result).encode())
        except Exception as e:
            self.send_response(500)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(json.dumps({"score": 0.0, "detail": f"grader error: {e}"}).encode())

    def log_message(self, format, *args):
        # Suppress default logging
        pass


def run_server(port: int, sema_path: str):
    GraderHandler.sema_path = sema_path
    server = HTTPServer(("0.0.0.0", port), GraderHandler)
    print(f"Grader server running on port {port} (sema: {sema_path})")
    server.serve_forever()


# ─── Main ─────────────────────────────────────────────────────────────────────

def main():
    parser = argparse.ArgumentParser(description="Grade Sema code completions")
    parser.add_argument("--server", action="store_true", help="Run as HTTP server")
    parser.add_argument("--port", type=int, default=8080, help="Server port")
    parser.add_argument("--sema-path", default=None, help="Path to sema binary")
    parser.add_argument("--problem", default=None, help="Path to problem JSON file")
    parser.add_argument("--completion", default=None, help="Path to completion text file")
    parser.add_argument("--input", default=None, help="Direct input expression")
    parser.add_argument("--expected", default=None, help="Expected output")
    args = parser.parse_args()

    sema_path = args.sema_path or find_sema_binary()

    if args.server:
        if not sema_path:
            print("ERROR: sema binary not found. Build with: cargo build", file=sys.stderr)
            sys.exit(1)
        run_server(args.port, sema_path)
        return

    if args.problem and args.completion:
        problem = json.loads(Path(args.problem).read_text())
        completion = Path(args.completion).read_text()
        result = grade(problem, completion, sema_path)
        print(json.dumps(result, indent=2))
        return

    if args.input is not None:
        problem = {"input": args.input, "expected": args.expected}
        completion = args.completion if args.completion else args.input
        result = grade(problem, completion, sema_path)
        print(json.dumps(result, indent=2))
        return

    parser.print_help()


if __name__ == "__main__":
    main()
