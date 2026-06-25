"""
Sema evaluator for RFT — scores model completions by comparing to expected output.

The evaluator extracts the answer from the model's completion and compares it
to the ground truth. No code execution needed (the training data already has
verified expected outputs from the Sema VM).
"""

import re
import os
import logging
from typing import Optional
from eval_protocol.models import EvaluateResult, EvaluationRow, MetricResult, Message
from eval_protocol.pytest import SingleTurnRolloutProcessor, evaluation_test

logger = logging.getLogger(__name__)

JSONL_PATH = os.path.abspath(
    os.path.join(os.path.dirname(__file__), "../../../development/sema_eval_dataset.jsonl")
)


def normalize_sema_value(s: str) -> str:
    """Normalize a Sema printed value for comparison."""
    s = s.strip()
    # Remove surrounding quotes if present
    if s.startswith('"') and s.endswith('"'):
        s = s[1:-1]
    # Collapse whitespace
    s = re.sub(r"\s+", " ", s)
    return s


def extract_answer(completion: str, ground_truth: str) -> Optional[str]:
    """Extract the answer from a model completion.

    The model may:
    1. Output just the value: "3"
    2. Output with explanation: "The result is 3"
    3. Wrap in code blocks: ```sema\n3\n```
    4. Say "=> 3" or "Result: 3"
    """
    if not completion:
        return None

    text = completion.strip()

    # Try extracting from code blocks first
    code_match = re.search(r"```(?:sema|lisp|scheme|clj)?\s*\n?(.*?)```", text, re.DOTALL)
    if code_match:
        text = code_match.group(1).strip()

    # If the entire text is just the value (common for eval tasks)
    if "\n" not in text and len(text) < 200:
        return text

    # Try "=> value" pattern
    arrow_match = re.search(r"=>\s*(.+?)(?:\n|$)", text)
    if arrow_match:
        return arrow_match.group(1).strip()

    # Try "Result: value" or "The result is value"
    result_match = re.search(r"(?:result|answer|output|value)\s*(?:is|:)\s*(.+?)(?:\n|$)", text, re.IGNORECASE)
    if result_match:
        return result_match.group(1).strip()

    # Last line might be the answer
    lines = [l.strip() for l in text.split("\n") if l.strip()]
    if lines:
        return lines[-1]

    return text


@evaluation_test(
    input_dataset=[JSONL_PATH],
    completion_params=[{"temperature": 0.0, "model": "fireworks_ai/accounts/fireworks/models/qwen3-8b"}],
    max_dataset_rows=5,
    passed_threshold=0.0,
    rollout_processor=SingleTurnRolloutProcessor(),
    mode="pointwise",
)
def test_sema_eval(row: EvaluationRow, **kwargs) -> EvaluationRow:
    """Evaluate Sema expression evaluation tasks."""

    # Get the model's completion (messages[2] is the assistant response)
    messages = row.messages
    completion = ""
    for msg in reversed(messages):
        if msg.role == "assistant":
            completion = msg.content
            break

    ground_truth = str(row.ground_truth) if row.ground_truth else ""

    predicted = extract_answer(completion, ground_truth)
    gt_normalized = normalize_sema_value(ground_truth)

    if predicted is None:
        score = 0.0
        reason = "No answer found in completion"
    else:
        pred_normalized = normalize_sema_value(predicted)
        if pred_normalized == gt_normalized:
            score = 1.0
            reason = f"Correct: {pred_normalized}"
        elif gt_normalized in pred_normalized:
            score = 0.5
            reason = f"Partial: expected {gt_normalized!r}, got {pred_normalized!r} (contains answer)"
        else:
            score = 0.0
            reason = f"Wrong: expected {gt_normalized!r}, got {pred_normalized!r}"

    row.evaluation_result = EvaluateResult(
        score=score,
        is_score_valid=True,
        reason=reason,
    )
    return row
