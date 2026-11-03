#!/usr/bin/env python3
"""Probe the request representation *before* the model commits to an operation.

If the gold clarification policy is decodable at the prompt end but not at the
decision token, then the model knows the answer and loses it while emitting the
operation. That would make the failure a routing/readout problem, not a missing
capability, and would tell us where a weight edit has to act.
"""

from __future__ import annotations

import json
from pathlib import Path

import numpy as np
import torch

import run_wsp as wsp

HERE = Path(__file__).resolve().parent
OUT = HERE / "out"


def main() -> int:
    torch.set_num_threads(8)
    cases = wsp.load_cases()
    tokenizer, model = wsp.load_model(str(OUT / "model"))
    texts = wsp.batch_texts(tokenizer, cases)

    per_layer: list[list[np.ndarray]] = []
    for text in texts:
        encoded = tokenizer(text, return_tensors="pt", truncation=True, max_length=1024)
        with torch.inference_mode():
            output = model(**encoded, output_hidden_states=True)
        if not per_layer:
            per_layer = [[] for _ in output.hidden_states]
        for index, state in enumerate(output.hidden_states):
            per_layer[index].append(state[0, -1, :].float().cpu().numpy())

    hidden = [np.stack(chunk) for chunk in per_layer]
    np.savez_compressed(
        OUT / "prompt_hidden.npz",
        **{f"layer_{i}": states for i, states in enumerate(hidden)},
    )
    print(f"wrote prompt_hidden.npz: {len(hidden)} layers x {len(cases)} cases")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
