#!/usr/bin/env python3
"""
Merge the RFT-trained LoRA adapter into the Qwen3-8B base model,
then export as a HuggingFace model directory that Ollama can import.

Since Ollama's ADAPTER directive expects GGUF format, we merge the LoRA
into the base model weights and create a new model from the merged result.
"""

import os
import sys
import torch
from pathlib import Path

ADAPTER_DIR = Path(__file__).parent / "lora-adapter"
OUTPUT_DIR = Path(__file__).parent / "merged-model"

def main():
    from peft import PeftModel
    from transformers import AutoModelForCausalLM, AutoTokenizer

    print("Loading base model Qwen/Qwen3-8B...")
    # Load in float16 to save memory
    base_model = AutoModelForCausalLM.from_pretrained(
        "Qwen/Qwen3-8B",
        torch_dtype=torch.float16,
        device_map="cpu",  # CPU merge to avoid GPU memory issues
        trust_remote_code=True,
    )
    tokenizer = AutoTokenizer.from_pretrained("Qwen/Qwen3-8B", trust_remote_code=True)

    print(f"Loading LoRA adapter from {ADAPTER_DIR}...")
    model = PeftModel.from_pretrained(base_model, str(ADAPTER_DIR))

    print("Merging LoRA weights into base model...")
    model = model.merge_and_unload()

    print(f"Saving merged model to {OUTPUT_DIR}...")
    OUTPUT_DIR.mkdir(exist_ok=True)
    model.save_pretrained(str(OUTPUT_DIR), safe_serialization=True)
    tokenizer.save_pretrained(str(OUTPUT_DIR))

    print(f"Done! Merged model saved to {OUTPUT_DIR}")
    print(f"Files:")
    for f in sorted(OUTPUT_DIR.iterdir()):
        print(f"  {f.name} ({f.stat().st_size / 1e6:.1f} MB)")

if __name__ == "__main__":
    main()
