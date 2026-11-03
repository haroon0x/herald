#!/usr/bin/env python3
"""Is the clarify policy present in the patient's residual stream at all?

Linear readout failed on the 0.6B. Two explanations remain:

  (a) the policy is not represented at that token -- it is a missing capability;
  (b) the policy is represented, but not in a linearly separable way.

These have opposite consequences. (a) rules out any projection-style weight edit.
(b) rules out a *rank-one* edit but leaves a real edit available.

This script separates them on the Qwen3.5-0.8B decision states, with the whole
pipeline inside the CV fold so nothing leaks:

  linear     -- logistic regression on standardised PCA components
  quadratic  -- logistic regression on degree-2 polynomial of the same components

and a permutation null for each. n=42 (10 positive), so everything is heavily
regularised and the null is what makes the number interpretable.

Usage:
    python probe_nl.py out/decide35.npz --label gold
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path

import numpy as np
from sklearn.decomposition import PCA
from sklearn.linear_model import LogisticRegression
from sklearn.model_selection import StratifiedKFold, cross_val_score
from sklearn.pipeline import Pipeline
from sklearn.preprocessing import PolynomialFeatures, StandardScaler

HERE = Path(__file__).resolve().parent


def make_pipeline(kind: str, components: int, seed: int = 0) -> Pipeline:
    steps = [
        ("scale", StandardScaler()),
        ("pca", PCA(n_components=components, random_state=seed)),
    ]
    if kind == "quadratic":
        steps.append(("poly", PolynomialFeatures(degree=2, include_bias=False)))
        steps.append(("scale2", StandardScaler()))
    steps.append(
        ("clf", LogisticRegression(C=0.05, max_iter=5000, class_weight="balanced"))
    )
    return Pipeline(steps)


def evaluate(x: np.ndarray, y: np.ndarray, kind: str, components: int) -> dict:
    folds = StratifiedKFold(n_splits=5, shuffle=True, random_state=0)
    scores = cross_val_score(
        make_pipeline(kind, components), x, y, cv=folds, scoring="accuracy"
    )
    return dict(mean=float(scores.mean()), std=float(scores.std()), folds=scores.tolist())


def permutation_null(x, y, kind, components, n_perm=20) -> np.ndarray:
    rng = np.random.default_rng(0)
    out = []
    for _ in range(n_perm):
        shuffled = rng.permutation(y)
        out.append(evaluate(x, shuffled, kind, components)["mean"])
    return np.array(out)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("npz", nargs="?", default=str(HERE / "out/decide35.npz"))
    parser.add_argument("--components", type=int, default=8)
    parser.add_argument("--perms", type=int, default=20)
    args = parser.parse_args()

    data = np.load(args.npz)
    y = data["gold"].astype(int)
    layers = sorted(
        (k for k in data.files if k.startswith("layer_")),
        key=lambda k: int(k.split("_")[1]),
    )
    majority = max(y.mean(), 1 - y.mean())
    print(f"n={len(y)} positives={int(y.sum())} majority={majority:.3f} "
          f"components={args.components}\n")
    print(f"{'layer':>6} {'linear':>8} {'quad':>8} {'null_lin':>9} {'null_quad':>10}")

    results = []
    for name in layers:
        x = data[name]
        lin = evaluate(x, y, "linear", args.components)
        quad = evaluate(x, y, "quadratic", args.components)
        null_lin = permutation_null(x, y, "linear", args.components, args.perms)
        null_quad = permutation_null(x, y, "quadratic", args.components, args.perms)
        index = int(name.split("_")[1])
        print(
            f"{index:>6} {lin['mean']:>8.3f} {quad['mean']:>8.3f} "
            f"{null_lin.mean():>9.3f} {null_quad.mean():>10.3f}"
        )
        results.append(
            dict(layer=index, linear=lin, quadratic=quad,
                 null_linear=float(null_lin.mean()),
                 null_quadratic=float(null_quad.mean()))
        )

    best_lin = max(results, key=lambda r: r["linear"]["mean"])
    best_quad = max(results, key=lambda r: r["quadratic"]["mean"])
    print(f"\nbest linear    layer {best_lin['layer']}: {best_lin['linear']['mean']:.3f} "
          f"(null {best_lin['null_linear']:.3f}, majority {majority:.3f})")
    print(f"best quadratic layer {best_quad['layer']}: {best_quad['quadratic']['mean']:.3f} "
          f"(null {best_quad['null_quadratic']:.3f}, majority {majority:.3f})")

    verdict = (
        "policy absent at this token -> no projection edit can work"
        if max(best_lin["linear"]["mean"], best_quad["quadratic"]["mean"]) <= majority
        else "signal present -> an edit is possible, but check the null first"
    )
    print(f"\nverdict: {verdict}")
    (HERE / "out/probe_nl35.json").write_text(
        json.dumps(dict(results=results, majority=majority, verdict=verdict), indent=2)
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
