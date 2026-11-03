#!/usr/bin/env python3
"""Use the readout as the gate the model cannot make explicitly.

Three measurements now line up:

  * the clarify label is 0.881 CV-decodable from the prompt-end state (layer 20);
  * the model's own boolean has 0/10 recall when emitted second, 10/10 when
    emitted first, but 16/32 false positives -- and its margin cannot fix that
    (AUC 0.634);
  * so the representation is a *better* decision surface than the generation.

This script tests that directly: fit the probe out-of-fold, gate clarification on
its probability, and score the 42 cases using the operation the model actually
produced. Out-of-fold means no case contributes to its own prediction, so the
number is an honest estimate rather than a fit.

If this beats every generation-based variant, the conclusion is that the right
use of the weights is reading them, not editing them -- the half of WSP the blog
itself shows working.

Usage:
    python gate.py --layer 20 --results out/reorder35.json
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path

import numpy as np
from sklearn.linear_model import LogisticRegression
from sklearn.model_selection import StratifiedKFold
from sklearn.pipeline import Pipeline
from sklearn.preprocessing import StandardScaler

import run_wsp as wsp

HERE = Path(__file__).resolve().parent
OUT = HERE / "out"


def out_of_fold(x: np.ndarray, y: np.ndarray, folds: int = 5) -> np.ndarray:
    prob = np.zeros(len(y))
    splitter = StratifiedKFold(n_splits=folds, shuffle=True, random_state=0)
    for train, test in splitter.split(x, y):
        pipe = Pipeline(
            [
                ("scale", StandardScaler()),
                ("clf", LogisticRegression(C=0.05, max_iter=5000, class_weight="balanced")),
            ]
        )
        pipe.fit(x[train], y[train])
        prob[test] = pipe.predict_proba(x[test])[:, 1]
    return prob


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--layer", type=int, default=20)
    parser.add_argument("--results", default=str(OUT / "reorder35.json"))
    parser.add_argument("--features", default=str(OUT / "prompt35.npz"))
    args = parser.parse_args()

    cases = wsp.load_cases()
    npz = np.load(args.features)
    x = npz[f"layer_{args.layer}"]
    y = npz["gold"].astype(int)
    produced = {
        r["case_id"]: r.get("operation")
        for r in json.loads(Path(args.results).read_text())["results"]
    }

    prob = out_of_fold(x, y)
    print(f"layer {args.layer} out-of-fold probability, n={len(y)}, positives={y.sum()}\n")
    print(f"{'thresh':>7} {'tp':>3} {'fp':>3} {'fn':>3} {'op_ok':>6} {'score':>6}")
    best = None
    for t in np.linspace(0.05, 0.95, 37):
        gate = prob > t
        tp = int((gate & (y == 1)).sum())
        fp = int((gate & (y == 0)).sum())
        fn = int((~gate & (y == 1)).sum())
        op_ok = sum(
            1
            for i, case in enumerate(cases)
            if not gate[i] and not case.clarify and produced[case.id] == case.operation
        )
        score = tp + op_ok
        if best is None or score > best[1]:
            best = (float(t), score, tp, fp, fn, op_ok)
        if abs(t * 100 % 10) < 1e-9:
            print(f"{t:>7.2f} {tp:>3} {fp:>3} {fn:>3} {op_ok:>6} {score:>6}")

    t, score, tp, fp, fn, op_ok = best
    print(f"\nbest gate: t={t:.2f} -> {score}/42  tp={tp} fp={fp} fn={fn} op_ok={op_ok}")
    print(f"  vs model-as-is (reorder5 run): 26/42")
    print(f"  vs baseline 0.8B:              26/42 semantic / 21/42 strict")

    # How separable are the out-of-fold probabilities themselves?
    pos, neg = prob[y == 1], prob[y == 0]
    sep = float(
        ((pos[:, None] > neg[None, :]).sum() + 0.5 * (pos[:, None] == neg[None, :]).sum())
        / (len(pos) * len(neg))
    )
    print(f"  out-of-fold AUC:               {sep:.3f}")
    (OUT / "gate35.json").write_text(
        json.dumps(
            dict(layer=args.layer, prob=prob.tolist(), gold=y.tolist(),
                 best=best, auc=sep),
            indent=2,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
