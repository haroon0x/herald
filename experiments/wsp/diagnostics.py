#!/usr/bin/env python3
"""Reproduce the causal diagnostics from the WSP pilot.

The probe direction separates failing from passing prompts, but separation is
not causation. This script measures whether projecting that direction out of a
layer's `down_proj` actually changes the next-token logits, and whether it
changes behavior on a small subset at several strengths.

Writes `out/diagnostics.json`. No gradients are used anywhere.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

import numpy as np
import torch

import run_wsp as wsp

HERE = Path(__file__).resolve().parent
OUT = HERE / "out"

SUBSET = ["check-mail", "draft-pass", "send-no-body", "draft-sam-thursday", "search-invoice"]


def edits(model, direction: torch.Tensor, layers, alpha: float, originals):
    for layer, original in zip(model.model.layers, originals):
        layer.mlp.down_proj.weight.data.copy_(original)
    for index in layers:
        down = model.model.layers[index].mlp.down_proj
        with torch.no_grad():
            projection = direction @ down.weight.data
            down.weight.data.sub_(alpha * torch.outer(direction, projection))


def main() -> int:
    torch.set_num_threads(8)
    cases = {case.id: case for case in wsp.load_cases()}
    tokenizer, model = wsp.load_model(str(OUT / "model"))
    direction = torch.tensor(np.load(OUT / "direction-L27.npy"), dtype=torch.float32)

    # 1. Does the edit move the next-token logits, and where?
    anchor = cases["check-mail"]
    encoded = tokenizer(
        wsp.batch_texts(tokenizer, [anchor])[0],
        return_tensors="pt",
        truncation=True,
        max_length=1024,
    )
    with torch.inference_mode():
        base = model(**encoded).logits[0, -1].float()
    base_top = int(base.argmax())

    layer_rows = []
    for layer in range(len(model.model.layers)):
        down = model.model.layers[layer].mlp.down_proj
        original = down.weight.data.clone()
        with torch.no_grad():
            projection = direction @ down.weight.data
            down.weight.data.sub_(torch.outer(direction, projection))
        with torch.inference_mode():
            logits = model(**encoded).logits[0, -1].float()
        layer_rows.append(
            {
                "layer": layer,
                "max_logit_delta": float((logits - base).abs().max()),
                "top1_changed": int(logits.argmax()) != base_top,
            }
        )
        down.weight.data.copy_(original)

    # 2. Does a stronger or distributed edit change behavior on a subset?
    subset = [cases[case_id] for case_id in SUBSET]
    originals = [layer.mlp.down_proj.weight.data.clone() for layer in model.model.layers]
    configs = [
        {"label": "layer-27-alpha-2", "layers": [27], "alpha": 2.0},
        {"label": "layer-27-alpha-5", "layers": [27], "alpha": 5.0},
        {"label": "layer-27-alpha-10", "layers": [27], "alpha": 10.0},
        {"label": "all-layers-alpha-1", "layers": list(range(28)), "alpha": 1.0},
        {"label": "all-layers-alpha-2", "layers": list(range(28)), "alpha": 2.0},
    ]

    sweep = []
    for config in configs:
        edits(model, direction, config["layers"], config["alpha"], originals)
        results = wsp.generate_results(tokenizer, model, subset, batch_size=1)
        sweep.append(
            {
                "config": config,
                "results": [result.__dict__ for result in results],
            }
        )
        print(f"{config['label']}: " + ", ".join(
            f"{result.case_id}={result.needs_clarification}" for result in results
        ))

    for layer, original in zip(model.model.layers, originals):
        layer.mlp.down_proj.weight.data.copy_(original)

    payload = {
        "anchor": anchor.id,
        "layer_logit_delta": sorted(
            layer_rows, key=lambda row: row["max_logit_delta"], reverse=True
        ),
        "subset_sweep": sweep,
    }
    (OUT / "diagnostics.json").write_text(json.dumps(payload, indent=2))
    print(json.dumps(payload["layer_logit_delta"][:5], indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
