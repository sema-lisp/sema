#!/usr/bin/env python3
"""
Prepare the RFT problem set from extracted eval_tests! pairs.

Reads data/eval_pairs.jsonl and produces:
  data/rft_problems.jsonl — 200-500 problems for RFT training (90% of eval pairs)
  data/sft_dataset.jsonl  — Full SFT dataset (all eval pairs + example pairs)

Holds out 10% of eval pairs for benchmarking (NOT included in training data).
"""

import json
import random
from pathlib import Path

SYSTEM_PROMPT_SUMMARY = "You are a Sema Lisp evaluator. Given a Sema expression, return its evaluated result as a Sema printed value."

def make_instruction_pair(input_str: str, expected: str, system: str = SYSTEM_PROMPT_SUMMARY) -> dict:
    """Create an OpenAI-compatible instruction-tuning pair."""
    return {
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": f"Evaluate this Sema expression:\n{input_str}"},
            {"role": "assistant", "content": expected},
        ]
    }


def make_codegen_pair(prompt: str, code: str, system: str) -> dict:
    """Create a code generation instruction-tuning pair."""
    return {
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": prompt},
            {"role": "assistant", "content": code},
        ]
    }


def main():
    script_dir = Path(__file__).parent
    data_dir = script_dir / "data"

    # Load eval pairs
    eval_pairs = []
    eval_file = data_dir / "eval_pairs.jsonl"
    if not eval_file.exists():
        print(f"ERROR: {eval_file} not found. Run extract_eval_tests.py first.")
        return

    with eval_file.open() as f:
        for line in f:
            if line.strip():
                pair = json.loads(line)
                if pair.get("expected") is not None:
                    eval_pairs.append(pair)

    print(f"Loaded {len(eval_pairs)} eval pairs with expected outputs")

    # Filter out pairs that are too long or too short
    def is_good(pair):
        inp = pair["input"]
        exp = pair["expected"]
        # Skip very long inputs (>500 chars) — likely integration tests
        if len(inp) > 500:
            return False
        # Skip very short (<5 chars) — trivial
        if len(inp) < 5:
            return False
        # Skip ones with I/O or network calls
        io_patterns = ["file/", "http/", "channel/", "async/", "println", "display", "terminal/", "system/"]
        if any(p in inp for p in io_patterns):
            return False
        return True

    filtered = [p for p in eval_pairs if is_good(p)]
    print(f"Filtered to {len(filtered)} pairs (removed {len(eval_pairs) - len(filtered)} I/O or too long/short)")

    # Categorize
    def categorize(pair):
        inp = pair["input"]
        if any(p in inp for p in ["string/", "string-", "str ", "char-"]):
            return "string"
        if any(p in inp for p in ["map/", "get ", "assoc ", "hash-", ":{", ":keys"]):
            return "map"
        if any(p in inp for p in ["list", "cons", "car", "cdr", "map ", "filter", "fold", "append", "reverse", "sort", "range", "zip", "take", "drop"]):
            return "list"
        if any(p in inp for p in ["+", "-", "*", "/", "mod", "pow", "round", "floor", "ceil", "sqrt"]):
            return "arithmetic"
        if any(p in inp for p in ["let", "let*", "letrec", "define", "fn ", "lambda"]):
            return "binding"
        if any(p in inp for p in ["if ", "cond ", "when ", "unless", "match", "case"]):
            return "control-flow"
        if any(p in inp for p in ["defmacro", "quasiquote", "unquote", "`", ",@", "macroexpand"]):
            return "macro"
        if any(p in inp for p in ["try", "catch", "throw"]):
            return "error-handling"
        if any(p in inp for p in ["regex", "#\""]):
            return "regex"
        if any(p in inp for p in ["f\"", "${"]):
            return "f-string"
        if any(p in inp for p in ["#("]):
            return "short-lambda"
        return "other"

    for p in filtered:
        p["category"] = categorize(p)

    # Split: 90% train, 10% holdout
    random.seed(42)
    random.shuffle(filtered)
    split_idx = int(len(filtered) * 0.9)
    train_pairs = filtered[:split_idx]
    holdout_pairs = filtered[split_idx:]

    print(f"Split: {len(train_pairs)} train, {len(holdout_pairs)} holdout")

    # Write RFT problems (format for Fireworks RFT)
    rft_out = data_dir / "rft_problems.jsonl"
    with rft_out.open("w") as f:
        for pair in train_pairs:
            f.write(json.dumps({
                "prompt": f"Evaluate this Sema expression:\n{pair['input']}",
                "expected": pair["expected"],
                "category": pair["category"],
            }) + "\n")

    # Write SFT dataset (OpenAI-compatible chat format)
    sft_system = "You are a Sema programming assistant. Sema is a Lisp dialect with LLM primitives. Evaluate Sema expressions and return their printed result."
    sft_out = data_dir / "sft_dataset.jsonl"
    with sft_out.open("w") as f:
        for pair in train_pairs:
            training_pair = make_instruction_pair(pair["input"], pair["expected"], sft_system)
            f.write(json.dumps(training_pair) + "\n")

    # Write holdout (for benchmarking — NOT training)
    holdout_out = data_dir / "eval_holdout.jsonl"
    with holdout_out.open("w") as f:
        for pair in holdout_pairs:
            f.write(json.dumps(pair) + "\n")

    # Stats
    cats = {}
    for p in train_pairs:
        cats[p["category"]] = cats.get(p["category"], 0) + 1

    print(f"\nRFT problems: {rft_out} ({len(train_pairs)} problems)")
    print(f"SFT dataset:  {sft_out} ({len(train_pairs)} pairs)")
    print(f"Holdout:      {holdout_out} ({len(holdout_pairs)} pairs)")
    print(f"\nCategory distribution:")
    for cat in sorted(cats, key=cats.get, reverse=True):
        print(f"  {cat:20s} {cats[cat]:4d}")


if __name__ == "__main__":
    main()
