#!/usr/bin/env python3
"""Run the 42-case classifier eval on Qwen3.5-0.8B (the blog's model)."""

from __future__ import annotations

import argparse
import json
from pathlib import Path

import torch
from transformers import AutoTokenizer, Qwen3_5ForConditionalGeneration

import run_wsp as wsp

HERE = Path(__file__).resolve().parent
OUT = HERE / "out"


def device(choice: str = "auto") -> str:
    if choice != "auto":
        return choice
    return "cuda" if torch.cuda.is_available() else "cpu"


def load(path: str, dtype_name: str = "bf16", where: str = "auto"):
    tok = AutoTokenizer.from_pretrained(path, local_files_only=True)
    tok.padding_side = "left"
    if tok.pad_token is None:
        tok.pad_token = tok.eos_token
    target = device(where)
    # bf16 is native on Ampere+; fp32 stays available for the CPU path.
    dtype = torch.bfloat16 if dtype_name == "bf16" else torch.float32
    model = Qwen3_5ForConditionalGeneration.from_pretrained(
        path, dtype=dtype, local_files_only=True
    )
    model.to(target)
    model.eval()
    print(f"device={target} dtype={dtype} cuda={torch.cuda.is_available()}", flush=True)
    return tok, model


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", default=str(OUT / "model35"))
    parser.add_argument("--name", default="baseline35")
    parser.add_argument("--limit", type=int, default=0)
    parser.add_argument("--batch-size", type=int, default=1)
    parser.add_argument("--system", choices=["orig", "fixed", "reorder", "reorderctl", "cal", "shipped"], default="orig")
    parser.add_argument("--max-new", type=int, default=48)
    parser.add_argument("--dtype", choices=["bf16", "fp32"], default="bf16")
    parser.add_argument("--device", default="auto")
    args = parser.parse_args()

    torch.set_num_threads(8)
    cases = wsp.load_cases()
    if args.limit:
        cases = cases[: args.limit]
    if args.system == "fixed":
        import prompt_fix

        wsp.CLASSIFIER_SYSTEM = prompt_fix.FIXED_SYSTEM
    elif args.system == "reorder":
        import prompt_reorder

        wsp.CLASSIFIER_SYSTEM = prompt_reorder.REORDER_SYSTEM
    elif args.system == "reorderctl":
        import prompt_reorder

        wsp.CLASSIFIER_SYSTEM = prompt_reorder.CONTROL_SYSTEM
    elif args.system == "cal":
        import prompt_cal

        wsp.CLASSIFIER_SYSTEM = prompt_cal.CAL_SYSTEM
    elif args.system == "shipped":
        import prompt_shipped

        wsp.CLASSIFIER_SYSTEM = prompt_shipped.SHIPPED_SYSTEM

    tok, model = load(args.model, args.dtype, args.device)
    texts = wsp.batch_texts(tok, cases)
    results = []
    for index, (case, text) in enumerate(zip(cases, texts)):
        enc = tok(text, return_tensors="pt", truncation=True, max_length=1024)
        enc = {k: v.to(model.device) for k, v in enc.items()}
        with torch.inference_mode():
            output = model.generate(
                **enc,
                max_new_tokens=args.max_new,
                do_sample=False,
                pad_token_id=tok.pad_token_id,
            )
        generated = output[0, enc["input_ids"].shape[1] :]
        raw = tok.decode(generated.cpu(), skip_special_tokens=True)
        result = wsp.score(case, raw)
        results.append(result)
        OUT.mkdir(parents=True, exist_ok=True)
        (OUT / f"{args.name}.partial.json").write_text(
            json.dumps({"summary": wsp.summarize(results, cases),
                        "results": [r.__dict__ for r in results]}, indent=2)
        )
        flag = "PASS" if result.passed else "FAIL"
        print(f"{flag} {case.id:<24} op={result.operation} clarify={result.needs_clarification} raw={raw[:80]!r}", flush=True)

    payload = wsp.save_results(args.name, results, cases)
    print(json.dumps(payload["summary"], indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
