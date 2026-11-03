#!/usr/bin/env python3
"""Doctor step 2: is the prompt-end clarify direction *causal* at the output?

The probe found the clarify policy linearly decodable at the prompt-end state of
layers ~11-21 (CV 0.88 vs 0.76 majority, permutation null 0.58). At the decision
token it is gone (max 0.76 = majority, and no case has a positive margin).

Two readings:

  (a) the representation is computed and then discarded before the boolean is
      written -- an obstacle, which is a legitimate WSP target;
  (b) the probe is picking up a correlate of length/wording that has no causal
      role.

This script separates them by adding the prompt-end difference direction back
into the residual at the decision token, layer by layer, and asking whether the
boolean flips. If it flips, the representation is real and merely not routed. If
nothing flips at any layer or scale, the probe was correlational and no edit on
this path can work.

Intervention scale is expressed as a fraction of the mean activation norm at that
layer, so alpha is comparable across layers.

Usage:
    python steer35.py --layers 11 15 19 20 23 --alphas 0.5 1.0 2.0
"""

from __future__ import annotations

import argparse
import json
import re
from pathlib import Path

import numpy as np
import torch

import run_wsp as wsp
from eval35 import load

HERE = Path(__file__).resolve().parent
OUT = HERE / "out"
MARKER = '"needs_clarification": '
LAYER_PATH = "model.language_model.layers.{}"


def prefix_for(raw: str, fallback_op: str) -> str:
    index = raw.find(MARKER)
    if index >= 0:
        return raw[: index + len(MARKER)]
    match = re.search(r'"operation"\s*:\s*"([a-z_]+)"', raw)
    op = match.group(1) if match else fallback_op
    return f'{{"operation": "{op}", "needs_clarification": '


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--layers", type=int, nargs="+", default=[11, 15, 19, 20, 23])
    parser.add_argument("--alphas", type=float, nargs="+", default=[0.5, 1.0, 2.0])
    parser.add_argument("--max-length", type=int, default=900)
    args = parser.parse_args()

    torch.set_num_threads(8)
    cases = wsp.load_cases()
    baseline = json.loads((OUT / "baseline35.json").read_text())["results"]
    by_result = {row["case_id"]: row for row in baseline}

    states = np.load(OUT / "prompt35.npz")
    gold = states["gold"].astype(bool)

    # Direction per layer: difference of class means, unit norm, plus the mean
    # activation norm used to scale the intervention.
    directions: dict[int, torch.Tensor] = {}
    norms: dict[int, float] = {}
    for layer in args.layers:
        x = states[f"layer_{layer}"]
        d = x[gold].mean(axis=0) - x[~gold].mean(axis=0)
        norms[layer] = float(np.linalg.norm(x, axis=1).mean())
        directions[layer] = torch.tensor(d / (np.linalg.norm(d) + 1e-9), dtype=torch.bfloat16)

    tok, model = load(str(OUT / "model35"), "bf16")
    true_id = tok.encode(" true", add_special_tokens=False)[0]
    false_id = tok.encode(" false", add_special_tokens=False)[0]
    texts = wsp.batch_texts(tok, cases)
    prompts = []
    for case, text in zip(cases, texts):
        raw = by_result[case.id]["raw"] or ""
        full = text + prefix_for(raw, case.operation)
        prompts.append(
            tok(full, return_tensors="pt", truncation=True, max_length=args.max_length)
        )

    def run(layer: int | None, alpha: float) -> np.ndarray:
        handle = None
        if layer is not None and alpha != 0.0:
            target = model.get_submodule(LAYER_PATH.format(layer - 1))
            scale = alpha * norms[layer]

            delta = directions[layer].to(torch.bfloat16) * scale

            def hook(module, inputs, output):
                # The decoder layer is called with batch=1, so its output is
                # (seq, hidden); handle the batched layout too, for safety.
                hidden = output[0] if isinstance(output, tuple) else output
                if hidden.dim() == 3:
                    hidden[:, -1, :] += delta
                else:
                    hidden[-1, :] += delta
                return None

            handle = target.register_forward_hook(hook)
        margins = []
        try:
            for enc in prompts:
                with torch.inference_mode():
                    logits = model(**enc, logits_to_keep=1).logits[0, -1].float()
                margins.append(float(logits[true_id] - logits[false_id]))
        finally:
            if handle is not None:
                handle.remove()
        return np.array(margins)

    base = run(None, 0.0)
    print(f"baseline: clarify margin>0 on {int((base > 0).sum())}/10 clarify cases, "
          f"{int((base[~gold] > 0).sum())}/32 non-clarify cases")
    print(f"{'layer':>5} {'alpha':>6} {'clarify>0':>10} {'nonclar>0':>10} {'tp':>4} {'fp':>4} {'score':>6}")

    rows = []
    for layer in args.layers:
        for alpha in args.alphas:
            margins = run(layer, alpha)
            tp = int((margins[gold] > 0).sum())
            fp = int((margins[~gold] > 0).sum())
            # Semantic score: a true on a clarify case is a pass; on a non-clarify
            # case the operation still has to be right, so count those as lost.
            score = tp + int((~gold).sum()) - fp
            print(f"{layer:>5} {alpha:>6.2f} {tp:>10} {fp:>10} {tp:>4} {fp:>4} {score:>6}")
            rows.append(dict(layer=layer, alpha=alpha, tp=tp, fp=fp,
                             margins=margins.tolist()))
        print()

    (OUT / "steer35.json").write_text(json.dumps(dict(base=base.tolist(), rows=rows), indent=2))
    print(f"wrote {OUT / 'steer35.json'}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
