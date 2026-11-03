#!/usr/bin/env python3
"""Calibrate the recovered clarify decision.

Reorder result: putting `needs_clarification` first takes recall from 0/10 to
10/10, but false positives go 1/32 -> 16/32. The decision is now being made; it
is not calibrated. A decision that is made but miscalibrated is a threshold
problem, not a capability problem.

This reads the true/false margin at the flag-first position for all 42 cases,
then sweeps a threshold: above it, clarify; below it, take the operation the
model produced in the reorder run. The point is to find whether a single scalar
threshold dominates the uncalibrated extreme.

Usage:
    python calibrate.py --results out/reorder35.json
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path

import numpy as np
import torch

import run_wsp as wsp
from eval35 import load
import prompt_reorder

HERE = Path(__file__).resolve().parent
OUT = HERE / "out"
FLAG_PREFIX = '{"needs_clarification": '


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--results", default=str(OUT / "reorder35.json"))
    parser.add_argument("--device", default="auto")
    args = parser.parse_args()

    torch.set_num_threads(8)
    cases = wsp.load_cases()
    results = {r["case_id"]: r for r in json.loads(Path(args.results).read_text())["results"]}
    wsp.CLASSIFIER_SYSTEM = prompt_reorder.REORDER_SYSTEM
    tok, model = load(str(OUT / "model35"), "bf16", args.device)
    texts = wsp.batch_texts(tok, cases)
    true_id = tok.encode(" true", add_special_tokens=False)[0]
    false_id = tok.encode(" false", add_special_tokens=False)[0]

    margins = []
    for case, text in zip(cases, texts):
        enc = tok(text + FLAG_PREFIX, return_tensors="pt", truncation=True, max_length=1024)
        enc = {k: v.to(model.device) for k, v in enc.items()}
        with torch.inference_mode():
            logits = model(**enc, logits_to_keep=1).logits[0, -1].float()
        margins.append(float(logits[true_id] - logits[false_id]))
    margins = np.array(margins)
    gold = np.array([c.clarify for c in cases], dtype=bool)
    gold_op = np.array([c.operation for c in cases])

    print("margin by gold label:")
    for value in (False, True):
        print(f"  clarify={value}: n={int((gold == value).sum())} mean={margins[gold == value].mean():+.3f}")
    print(f"  separation AUC = {auc(margins, gold):.3f}\n")

    print(f"{'threshold':>10} {'tp':>4} {'fp':>4} {'fn':>4} {'op_ok':>6} {'score':>7}")
    best = None
    for t in np.linspace(margins.min() - 0.1, margins.max() + 0.1, 60):
        gate = margins > t
        tp = int((gate & gold).sum())
        fp = int((gate & ~gold).sum())
        fn = int((~gate & gold).sum())
        # Non-gated cases fall through to the operation the model produced.
        op_ok = 0
        for i, case in enumerate(cases):
            if gate[i] and case.clarify:
                continue
            if not gate[i] and not case.clarify:
                op_ok += int(results[case.id]["operation"] == case.operation)
        score = tp + op_ok
        if best is None or score > best[1]:
            best = (float(t), score, tp, fp, fn, op_ok)
    for t in np.quantile(margins, np.linspace(0.05, 0.95, 19)):
        gate = margins > t
        tp = int((gate & gold).sum())
        fp = int((gate & ~gold).sum())
        fn = int((~gate & gold).sum())
        op_ok = sum(
            1
            for i, case in enumerate(cases)
            if not gate[i] and not case.clarify and results[case.id]["operation"] == case.operation
        )
        print(f"{t:>10.3f} {tp:>4} {fp:>4} {fn:>4} {op_ok:>6} {tp + op_ok:>7}")

    print(f"\nbest: threshold={best[0]:.3f} -> {best[1]}/42 "
          f"(tp={best[2]} fp={best[3]} fn={best[4]} op_ok={best[5]})")
    (OUT / "calibrate35.json").write_text(
        json.dumps(dict(margins=margins.tolist(), gold=gold.tolist(), best=best), indent=2)
    )
    return 0


def auc(scores: np.ndarray, labels: np.ndarray) -> float:
    pos, neg = scores[labels], scores[~labels]
    return float(
        ((pos[:, None] > neg[None, :]).sum() + 0.5 * (pos[:, None] == neg[None, :]).sum())
        / (len(pos) * len(neg))
    )


if __name__ == "__main__":
    raise SystemExit(main())
