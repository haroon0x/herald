#!/usr/bin/env python3
"""Cross-validated linear decodability of the gold clarification label.

Compares the prompt-end representation with the decision-token representation.
Includes a permutation baseline: with 42 cases and 10 positives, a CV score is
only meaningful if it beats both the majority class and shuffled labels.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path

import numpy as np

HERE = Path(__file__).resolve().parent
OUT = HERE / "out"


def load_npz(path: Path) -> np.ndarray:
    data = np.load(path)
    layers = sorted(data.files, key=lambda name: int(name.split("_")[1]))
    return np.stack([data[name] for name in layers])


def ridge_fit(x: np.ndarray, y: np.ndarray, lam: float) -> tuple[np.ndarray, float]:
    mean = x.mean(axis=0)
    y_mean = y.mean()
    xc = x - mean
    yc = y - y_mean
    gram = xc @ xc.T
    alpha = np.linalg.solve(gram + lam * np.eye(len(gram)), yc)
    w = xc.T @ alpha
    return w, float(y_mean - mean @ w)


def cv_predict(
    x: np.ndarray, y: np.ndarray, lam: float, folds: list[np.ndarray]
) -> np.ndarray:
    predictions = np.zeros(len(y), dtype=bool)
    for test in folds:
        train = np.setdiff1d(np.arange(len(y)), test)
        # standardise using training statistics only
        mean = x[train].mean(axis=0)
        scale = x[train].std(axis=0) + 1e-6
        xtr = (x[train] - mean) / scale
        xte = (x[test] - mean) / scale
        w, b = ridge_fit(xtr, y[train].astype(float), lam)
        predictions[test] = (xte @ w + b) > 0
    return predictions


def make_folds(n: int, k: int, seed: int) -> list[np.ndarray]:
    order = np.random.default_rng(seed).permutation(n)
    return [order[i::k] for i in range(k)]


def score_layer(x: np.ndarray, y: np.ndarray, seed: int = 0) -> tuple[float, float, int]:
    folds = make_folds(len(y), 5, seed)
    best = (0.0, 0.0)
    for lam in [1e-3, 1e-2, 1e-1, 1.0, 10.0, 100.0, 1000.0]:
        pred = cv_predict(x, y, lam, folds)
        acc = float((pred == y).mean())
        if acc > best[0]:
            best = (acc, lam)
    return best[0], best[1], 0


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("npz")
    parser.add_argument("--out", default=None)
    args = parser.parse_args()

    hidden = load_npz(Path(args.npz))
    rows = json.loads((OUT / "decision_probe.json").read_text())["rows"]
    y = np.array([row["gold_clarify"] for row in rows], dtype=bool)
    majority = max(y.mean(), 1 - y.mean())
    print(f"{args.npz}: layers={hidden.shape[0]} cases={hidden.shape[1]} d={hidden.shape[2]}")
    print(f"gold true={y.sum()} false={(~y).sum()} majority_acc={majority:.3f}")

    # Permutation baseline for the best layer.
    rng = np.random.default_rng(0)
    perm_scores = []
    for _ in range(20):
        yp = rng.permutation(y)
        acc, _, _ = score_layer(hidden[-1], yp, seed=0)
        perm_scores.append(acc)
    print(f"permuted-label acc: mean={np.mean(perm_scores):.3f} max={np.max(perm_scores):.3f}")

    results = []
    for index in range(hidden.shape[0]):
        acc, lam, _ = score_layer(hidden[index], y, seed=0)
        results.append((acc, index, lam))
        if index % 2 == 0:
            print(f"  hidden[{index:>2}] (decoder {index - 1:>2}): acc={acc:.3f} lam={lam}")

    results.sort(reverse=True)
    print("\ntop 5 layers:")
    for acc, index, lam in results[:5]:
        print(f"  hidden[{index}] (decoder {index - 1}): acc={acc:.3f} lam={lam}")

    if args.out:
        Path(args.out).write_text(
            json.dumps(
                {
                    "source": args.npz,
                    "majority": majority,
                    "permutation": {"mean": float(np.mean(perm_scores)), "max": float(np.max(perm_scores))},
                    "top": [
                        {"hidden_index": i, "accuracy": a, "lambda": l}
                        for a, i, l in results[:5]
                    ],
                },
                indent=2,
            )
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
