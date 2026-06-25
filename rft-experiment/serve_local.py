#!/usr/bin/env python3
"""
Serve the fine-tuned Sema model locally using the HuggingFace transformers library.
Provides an OpenAI-compatible API at http://localhost:8000/v1/chat/completions
so the benchmark.py script can test it without changes.
"""

import json
import os
import sys
import torch
from pathlib import Path
from http.server import HTTPServer, BaseHTTPRequestHandler

MODEL_DIR = Path(__file__).parent / "merged-model"
SYSTEM_PROMPT = Path(__file__).parent / "system_prompt.txt"

def load_model():
    from transformers import AutoModelForCausalLM, AutoTokenizer
    print(f"Loading model from {MODEL_DIR}...", flush=True)
    tokenizer = AutoTokenizer.from_pretrained(str(MODEL_DIR), trust_remote_code=True)
    model = AutoModelForCausalLM.from_pretrained(
        str(MODEL_DIR),
        torch_dtype=torch.float16,
        device_map="cpu",
        trust_remote_code=True,
    )
    print("Model loaded!", flush=True)
    return model, tokenizer

class InferenceHandler(BaseHTTPRequestHandler):
    model = None
    tokenizer = None

    def do_POST(self):
        content_length = int(self.headers.get("Content-Length", 0))
        body = self.rfile.read(content_length)
        data = json.loads(body)

        messages = data.get("messages", [])
        temperature = data.get("temperature", 0.0)
        max_tokens = data.get("max_tokens", 2048)

        # Build prompt from messages
        prompt_parts = []
        for msg in messages:
            role = msg.get("role", "user")
            content = msg.get("content", "")
            if role == "system":
                prompt_parts.append(f"<|im_start|>system\n{content}<|im_end|>")
            elif role == "user":
                prompt_parts.append(f"<|im_start|>user\n{content}<|im_end|>")
            elif role == "assistant":
                prompt_parts.append(f"<|im_start|>assistant\n{content}<|im_end|>")
        prompt_parts.append("<|im_start|>assistant\n/no_think\n")
        prompt = "\n".join(prompt_parts)

        # Generate — Qwen3 uses /no_think token to disable thinking mode
        inputs = self.tokenizer(prompt, return_tensors="pt").to(self.model.device)
        with torch.no_grad():
            outputs = self.model.generate(
                **inputs,
                max_new_tokens=max_tokens,
                temperature=max(temperature, 0.01),
                do_sample=temperature > 0,
                pad_token_id=self.tokenizer.eos_token_id,
                eos_token_id=self.tokenizer.convert_tokens_to_ids("<|im_end|>"),
            )

        # Decode only the new tokens
        input_len = inputs["input_ids"].shape[1]
        full_response = self.tokenizer.decode(outputs[0][input_len:], skip_special_tokens=True)

        # Strip thinking traces if present (Qwen3 <think>...</think>)
        if "<think>" in full_response:
            # Remove everything up to and including </think>
            think_end = full_response.find("</think>")
            if think_end != -1:
                response = full_response[think_end + len("</think>"):].strip()
            else:
                response = full_response.replace("<think>", "").strip()
        else:
            response = full_response

        response = response.split("<|im_end|>")[0].strip()
        # Take only the first line (the actual answer, before any trailing tags)
        response = response.split("\n")[0].strip()

        result = {
            "choices": [{"message": {"role": "assistant", "content": response}}],
            "usage": {
                "prompt_tokens": input_len,
                "completion_tokens": len(outputs[0]) - input_len,
            },
        }

        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(json.dumps(result).encode())

    def log_message(self, format, *args):
        pass  # Suppress default logging


def main():
    model, tokenizer = load_model()
    InferenceHandler.model = model
    InferenceHandler.tokenizer = tokenizer

    port = int(os.environ.get("PORT", 8001))
    server = HTTPServer(("0.0.0.0", port), InferenceHandler)
    print(f"\nSema fine-tuned model serving on http://localhost:{port}/v1/chat/completions")
    print(f"Test with: curl -X POST http://localhost:{port}/v1/chat/completions ...")
    server.serve_forever()


if __name__ == "__main__":
    main()
