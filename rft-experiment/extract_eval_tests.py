#!/usr/bin/env python3
"""
Extract eval_tests! and eval_error_tests! cases from Rust test files.

Produces two JSONL files:
  - data/eval_pairs.jsonl:      (input, expected_output) pairs from eval_tests!
  - data/eval_error_pairs.jsonl: (input, expected_error_substring) pairs from eval_error_tests!

The expected_output for eval_tests! is obtained by running each input through
`sema eval --json` and recording the printed value. This is reliable because
these tests already pass — the VM output IS the expected value.

Usage:
  python3 extract_eval_tests.py [--sema-path PATH] [--repo-root PATH]

  --sema-path: path to the sema binary (default: auto-detect via cargo)
  --repo-root: path to the repo root (default: parent of this script's dir)
"""

import argparse
import json
import os
import re
import subprocess
import sys
from pathlib import Path

# ─── Config ───────────────────────────────────────────────────────────────────

TEST_FILES = [
    "crates/sema/tests/eval_test.rs",
    "crates/sema/tests/eval_core_test.rs",
    "crates/sema/tests/eval_collections_test.rs",
    "crates/sema/tests/eval_stdlib_test.rs",
    "crates/sema/tests/eval_map_test.rs",
    "crates/sema/tests/eval_data_test.rs",
    "crates/sema/tests/eval_types_test.rs",
    "crates/sema/tests/eval_ergonomic_test.rs",
]

# ─── Parsing ──────────────────────────────────────────────────────────────────

def strip_comments(text: str) -> str:
    """Remove // ... comments from Rust source (line-level only, safe for macro bodies)."""
    lines = text.split("\n")
    result = []
    for line in lines:
        # Simple: cut at // that's not inside a string
        # Good enough for test files — strings with // are rare in Sema code
        in_string = False
        in_raw = False
        i = 0
        while i < len(line):
            c = line[i]
            if c == '"' and not in_raw:
                in_string = not in_string
            elif c == '\\' and in_string:
                i += 1  # skip escaped char
            i += 1
        # Find // that's not in a string
        # Simplified: just check if // appears after any string content
        # For our purposes, // at start of line or after whitespace is always a comment
        stripped = line.lstrip()
        if stripped.startswith("//"):
            continue
        result.append(line)
    return "\n".join(result)


def extract_raw_string(text: str, start: int) -> tuple[str, int]:
    """Extract a raw string r#"..."# (or r##"..."## etc.) starting at position `start`.
    Returns (content, end_index_after_closing)."""
    # Count the # signs
    i = start
    hash_count = 0
    while i < len(text) and text[i] == '#':
        hash_count += 1
        i += 1
    # Now at the opening "
    if i >= len(text) or text[i] != '"':
        raise ValueError(f"Expected '\"' after r{'#'*hash_count} at position {start}")
    i += 1  # skip opening "
    content_start = i
    # Find closing "#...#
    close_marker = '"' + '#' * hash_count
    close_pos = text.find(close_marker, content_start)
    if close_pos == -1:
        raise ValueError(f"Unterminated raw string starting at position {start}")
    content = text[content_start:close_pos]
    return content, close_pos + len(close_marker)


def extract_regular_string(text: str, start: int) -> tuple[str, int]:
    """Extract a regular string "..." starting at position `start` (text[start] == '"').
    Handles escape sequences. Returns (unescaped_content, end_index_after_closing_quote)."""
    i = start + 1  # skip opening "
    result = []
    while i < len(text):
        c = text[i]
        if c == '\\':
            if i + 1 < len(text):
                next_c = text[i + 1]
                escapes = {'n': '\n', 't': '\t', 'r': '\r', '\\': '\\', '"': '"', '0': '\0', "'": "'"}
                result.append(escapes.get(next_c, next_c))
                i += 2
            else:
                i += 1
        elif c == '"':
            return ''.join(result), i + 1
        else:
            result.append(c)
            i += 1
    raise ValueError(f"Unterminated string starting at position {start}")


def parse_eval_tests_block(block_text: str) -> list[tuple[str, str]]:
    """Parse an eval_tests! block body, returning [(test_name, input_string), ...]."""
    entries = []
    text = block_text
    i = 0

    while i < len(text):
        # Skip whitespace and commas
        while i < len(text) and text[i] in ' \t\n,':
            i += 1
        if i >= len(text):
            break

        # Read identifier (test name)
        id_start = i
        while i < len(text) and (text[i].isalnum() or text[i] == '_'):
            i += 1
        if i == id_start:
            i += 1  # skip unexpected char
            continue
        test_name = text[id_start:i]

        # Skip whitespace
        while i < len(text) and text[i] in ' \t\n':
            i += 1

        # Expect ':'
        if i >= len(text) or text[i] != ':':
            continue
        i += 1  # skip ':'

        # Skip whitespace
        while i < len(text) and text[i] in ' \t\n':
            i += 1

        # Now we expect a string: either r#"..."# or "..."
        if i >= len(text):
            break

        if text[i] == 'r' and i + 1 < len(text) and text[i + 1] == '#':
            input_str, end = extract_raw_string(text, i + 1)
            i = end
        elif text[i] == '"':
            input_str, end = extract_regular_string(text, i)
            i = end
        else:
            # Not a string — skip to next comma or end
            while i < len(text) and text[i] != ',':
                i += 1
            continue

        entries.append((test_name, input_str))

        # Skip to the next comma (past the => expected part)
        while i < len(text) and text[i] != ',':
            # Handle nested strings in the expected value
            if text[i] == '"':
                _, end = extract_regular_string(text, i)
                i = end
            elif text[i] == 'r' and i + 1 < len(text) and text[i + 1] == '#':
                _, end = extract_raw_string(text, i + 1)
                i = end
            else:
                i += 1

    return entries


def parse_eval_error_tests_block(block_text: str) -> list[tuple[str, str, str | None]]:
    """Parse an eval_error_tests! block body.
    Returns [(test_name, input_string, expected_error_substring_or_None), ...]."""
    entries = []
    text = block_text
    i = 0

    while i < len(text):
        # Skip whitespace and commas
        while i < len(text) and text[i] in ' \t\n,':
            i += 1
        if i >= len(text):
            break

        # Read identifier
        id_start = i
        while i < len(text) and (text[i].isalnum() or text[i] == '_'):
            i += 1
        if i == id_start:
            i += 1
            continue
        test_name = text[id_start:i]

        # Skip whitespace, expect ':'
        while i < len(text) and text[i] in ' \t\n':
            i += 1
        if i >= len(text) or text[i] != ':':
            continue
        i += 1

        # Skip whitespace
        while i < len(text) and text[i] in ' \t\n':
            i += 1

        # Read input string
        if i >= len(text):
            break

        if text[i] == 'r' and i + 1 < len(text) and text[i + 1] == '#':
            input_str, end = extract_raw_string(text, i + 1)
            i = end
        elif text[i] == '"':
            input_str, end = extract_regular_string(text, i)
            i = end
        else:
            while i < len(text) and text[i] != ',':
                i += 1
            continue

        # Check for => (strong form) or just comma (legacy form)
        # Skip whitespace
        while i < len(text) and text[i] in ' \t\n':
            i += 1

        expected_error = None
        if i < len(text) - 1 and text[i] == '=' and text[i + 1] == '>':
            i += 2  # skip =>
            # Skip whitespace
            while i < len(text) and text[i] in ' \t\n':
                i += 1
            # Read the expected error string
            if i < len(text) and text[i] == '"':
                expected_error, end = extract_regular_string(text, i)
                i = end
            elif i < len(text) and text[i] == 'r' and i + 1 < len(text) and text[i + 1] == '#':
                expected_error, end = extract_raw_string(text, i + 1)
                i = end

        entries.append((test_name, input_str, expected_error))

        # Skip to next comma
        while i < len(text) and text[i] != ',':
            if text[i] == '"':
                _, end = extract_regular_string(text, i)
                i = end
            elif text[i] == 'r' and i + 1 < len(text) and text[i + 1] == '#':
                _, end = extract_raw_string(text, i + 1)
                i = end
            else:
                i += 1

    return entries


def find_macro_blocks(source: str, macro_name: str) -> list[str]:
    """Find all invocations of `macro_name! { ... }` in source, returning the body text."""
    blocks = []
    pattern = macro_name + '!'

    search_start = 0
    while True:
        idx = source.find(pattern, search_start)
        if idx == -1:
            break
        # Skip past the pattern
        i = idx + len(pattern)
        # Skip whitespace
        while i < len(source) and source[i] in ' \t\n':
            i += 1
        # Expect '{'
        if i >= len(source) or source[i] != '{':
            search_start = idx + 1
            continue
        # Find matching '}' (handle nesting)
        depth = 1
        body_start = i + 1
        i += 1
        while i < len(source) and depth > 0:
            if source[i] == '{':
                depth += 1
            elif source[i] == '}':
                depth -= 1
            elif source[i] == '"':
                # Skip string content
                _, end = extract_regular_string(source, i)
                i = end
                continue
            elif source[i] == 'r' and i + 1 < len(source) and source[i + 1] == '#':
                # Skip raw string content
                try:
                    _, end = extract_raw_string(source, i + 1)
                    i = end
                    continue
                except ValueError:
                    pass
            i += 1
        if depth == 0:
            blocks.append(source[body_start:i - 1])
            search_start = i
        else:
            break

    return blocks


# ─── Sema Eval ────────────────────────────────────────────────────────────────

def find_sema_binary(repo_root: Path) -> str | None:
    """Try to find the sema binary."""
    # Check for a pre-built binary
    candidates = [
        repo_root / "target" / "debug" / "sema",
        repo_root / "target" / "release" / "sema",
    ]
    for c in candidates:
        if c.exists():
            return str(c)
    return None


def sema_eval(sema_path: str, code: str, timeout: int = 10) -> dict | None:
    """Run `sema eval --expr CODE --json` and return the parsed JSON result."""
    try:
        result = subprocess.run(
            [sema_path, "eval", "--expr", code, "--json", "--timeout", "5000"],
            capture_output=True,
            text=True,
            timeout=timeout,
        )
        if result.returncode == 0:
            return json.loads(result.stdout)
        else:
            return {"ok": False, "error": {"message": result.stderr.strip()}}
    except subprocess.TimeoutExpired:
        return {"ok": False, "error": {"message": "timeout"}}
    except json.JSONDecodeError:
        return {"ok": False, "error": {"message": f"invalid JSON output: {result.stdout[:200]}"}}
    except Exception as e:
        return {"ok": False, "error": {"message": str(e)}}


# ─── Main ─────────────────────────────────────────────────────────────────────

def main():
    parser = argparse.ArgumentParser(description="Extract eval_tests! cases from Rust test files")
    parser.add_argument("--sema-path", default=None, help="Path to sema binary")
    parser.add_argument("--repo-root", default=None, help="Path to repo root")
    parser.add_argument("--skip-eval", action="store_true", help="Skip running sema (only extract test names + inputs)")
    args = parser.parse_args()

    script_dir = Path(__file__).parent
    repo_root = Path(args.repo_root) if args.repo_root else script_dir.parent
    data_dir = script_dir / "data"
    data_dir.mkdir(exist_ok=True)

    # Find sema binary
    sema_path = args.sema_path or find_sema_binary(repo_root)
    if not sema_path and not args.skip_eval:
        print("Building sema binary (cargo build)...")
        build = subprocess.run(
            ["cargo", "build"],
            cwd=str(repo_root),
            capture_output=True,
            text=True,
            timeout=300,
        )
        if build.returncode != 0:
            print(f"ERROR: cargo build failed:\n{build.stderr}", file=sys.stderr)
            sys.exit(1)
        sema_path = str(repo_root / "target" / "debug" / "sema")

    all_eval_pairs = []
    all_error_pairs = []
    stats = {"files": 0, "eval_tests": 0, "error_tests": 0, "eval_ok": 0, "eval_fail": 0}

    for rel_path in TEST_FILES:
        file_path = repo_root / rel_path
        if not file_path.exists():
            print(f"  SKIP {rel_path} (not found)")
            continue

        source = file_path.read_text()
        stats["files"] += 1

        # Extract eval_tests! blocks
        eval_blocks = find_macro_blocks(source, "eval_tests")
        for block in eval_blocks:
            entries = parse_eval_tests_block(block)
            for test_name, input_str in entries:
                stats["eval_tests"] += 1
                pair = {"test_name": test_name, "input": input_str, "file": rel_path}

                if not args.skip_eval and sema_path:
                    result = sema_eval(sema_path, input_str)
                    if result and result.get("ok"):
                        pair["expected"] = result.get("value", "")
                        pair["stdout"] = result.get("stdout", "")
                        stats["eval_ok"] += 1
                    else:
                        pair["expected"] = None
                        pair["error"] = result.get("error", {}).get("message", "unknown") if result else "no result"
                        stats["eval_fail"] += 1
                else:
                    pair["expected"] = None

                all_eval_pairs.append(pair)

        # Extract eval_error_tests! blocks
        error_blocks = find_macro_blocks(source, "eval_error_tests")
        for block in error_blocks:
            entries = parse_eval_error_tests_block(block)
            for test_name, input_str, expected_error in entries:
                stats["error_tests"] += 1
                all_error_pairs.append({
                    "test_name": test_name,
                    "input": input_str,
                    "expected_error": expected_error,
                    "file": rel_path,
                })

    # Write outputs
    eval_out = data_dir / "eval_pairs.jsonl"
    with eval_out.open("w") as f:
        for pair in all_eval_pairs:
            f.write(json.dumps(pair) + "\n")

    error_out = data_dir / "eval_error_pairs.jsonl"
    with error_out.open("w") as f:
        for pair in all_error_pairs:
            f.write(json.dumps(pair) + "\n")

    print(f"\nExtraction complete:")
    print(f"  Files processed:     {stats['files']}")
    print(f"  eval_tests! cases:   {stats['eval_tests']} ({stats['eval_ok']} ok, {stats['eval_fail']} failed)")
    print(f"  error_tests! cases:  {stats['error_tests']}")
    print(f"  Output: {eval_out}")
    print(f"  Output: {error_out}")

    if stats["eval_fail"] > 0 and not args.skip_eval:
        print(f"\n  WARNING: {stats['eval_fail']} inputs failed to evaluate.")
        print(f"  These may be tests that require specific runtime context (I/O, sandbox, etc.).")


if __name__ == "__main__":
    main()
