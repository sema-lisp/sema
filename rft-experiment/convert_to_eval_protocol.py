#!/usr/bin/env python3
"""
Convert RFT problems to eval-protocol dataset format.

eval-protocol expects JSONL with:
  - messages: [{role, content}, ...] — the conversation
  - ground_truth: optional — expected value for scoring

This script reads data/rft_problems.jsonl and writes
sema_artifacts/development/sema_eval_dataset.jsonl
"""

import json
from pathlib import Path

SYSTEM_PROMPT = (
    "You are a Sema Lisp evaluator. Given a Sema expression, return only its "
    "evaluated result as a Sema printed value. Do not explain — just output the value."
)

def main():
    script_dir = Path(__file__).parent
    rft_path = script_dir / "data" / "rft_problems.jsonl"
    out_path = script_dir / "sema_artifacts" / "development" / "sema_eval_dataset.jsonl"

    if not rft_path.exists():
        print(f"ERROR: {rft_path} not found. Run extract_eval_tests.py + prepare_rft_problems.py first.")
        return

    count = 0
    with rft_path.open() as fin, out_path.open("w") as fout:
        for line in fin:
            if not line.strip():
                continue
            problem = json.loads(line)
            entry = {
                "messages": [
                    {"role": "system", "content": SYSTEM_PROMPT},
                    {"role": "user", "content": problem["prompt"]},
                ],
                "ground_truth": problem["expected"],
            }
            fout.write(json.dumps(entry) + "\n")
            count += 1

    print(f"Written {count} entries to {out_path}")


if __name__ == "__main__":
    main()
