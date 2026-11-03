#!/usr/bin/env python3
"""Audit an eval run under three scoring regimes.

Why: `run_wsp.score` treats a missing `needs_clarification` key as an invalid
JSON object, so the whole case fails -- and it then also reports `operation` as
None, which hides the fact that the operation pick was right. In production
Herald decodes with a GBNF grammar that makes the field mandatory, so that
failure mode cannot occur.

Reporting only the strict number conflates two different things:

  strict    -- what the generate-then-parse harness scores (field required)
  completed -- missing flag read as `false`, i.e. what the grammar guarantees
  semantic  -- right clarify decision, and right operation when not clarifying

This script re-parses the raw completion, so it recovers the operation that the
strict scorer discarded.

Usage:
    python rescore.py out/baseline.json out/baseline35.json
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

import run_wsp as wsp

OPERATIONS = set(wsp.OPERATIONS)


def parse_raw(raw: str) -> tuple[str | None, bool | None, bool]:
    """Return (operation, flag, json_valid) from a raw completion."""
    candidate = wsp.json_slice(raw or "")
    if candidate is None:
        return None, None, False
    try:
        parsed = json.loads(candidate)
    except json.JSONDecodeError:
        return None, None, False
    if not isinstance(parsed, dict):
        return None, None, False
    op = parsed.get("operation")
    flag = parsed.get("needs_clarification")
    return (
        op if isinstance(op, str) else None,
        flag if isinstance(flag, bool) else None,
        True,
    )


def rescore(path: Path) -> dict:
    payload = json.loads(path.read_text())
    cases = {case.id: case for case in wsp.load_cases()}
    rows = []
    strict = completed = semantic = 0
    for result in payload["results"]:
        case = cases[result["case_id"]]
        op, flag, json_valid = parse_raw(result.get("raw") or "")
        missing_flag = flag is None

        strict_ok = bool(result.get("passed"))
        # A grammar-enforced decoder always emits the boolean, so a missing key
        # on a non-clarify case is a serialisation artifact, not a decision.
        completed_ok = strict_ok or (
            missing_flag
            and json_valid
            and op == case.operation
            and not case.clarify
        )
        semantic_ok = (
            flag is True
            if case.clarify
            else (flag is False or (missing_flag and json_valid))
            and op == case.operation
        )

        strict += strict_ok
        completed += completed_ok
        semantic += semantic_ok
        rows.append(
            dict(
                case_id=result["case_id"],
                gold_clarify=case.clarify,
                gold_operation=case.operation,
                op=op,
                flag=flag,
                json_valid=json_valid,
                strict=strict_ok,
                completed=completed_ok,
                semantic=semantic_ok,
                raw=(result.get("raw") or "").strip()[:100],
            )
        )
    return dict(
        path=str(path),
        n=len(rows),
        strict=strict,
        completed=completed,
        semantic=semantic,
        rows=rows,
    )


def main() -> int:
    if len(sys.argv) < 2:
        print(__doc__)
        return 2
    for arg in sys.argv[1:]:
        report = rescore(Path(arg))
        print(
            f"\n{report['path']}  n={report['n']}\n"
            f"  strict    {report['strict']:>2}/{report['n']}\n"
            f"  completed {report['completed']:>2}/{report['n']}  "
            f"(+{report['completed'] - report['strict']} from grammar-enforced flag)\n"
            f"  semantic  {report['semantic']:>2}/{report['n']}"
        )
        flips = [r for r in report["rows"] if r["semantic"] and not r["strict"]]
        if flips:
            print("  strict says fail, decision was right:")
            for r in flips:
                print(
                    f"    {r['case_id']:<26} clarify={r['gold_clarify']!s:<5} "
                    f"op={r['op']} raw={r['raw']!r}"
                )
        wrong = [r for r in report["rows"] if not r["semantic"]]
        if wrong:
            print(f"  genuine decision failures ({len(wrong)}):")
            for r in wrong:
                kind = (
                    "over-clarified"
                    if r["flag"] is True and not r["gold_clarify"]
                    else "under-clarified"
                    if r["gold_clarify"]
                    else "wrong operation"
                )
                print(
                    f"    {r['case_id']:<26} {kind:<15} "
                    f"want={str(r['gold_operation']):<24} got={r["op"]} flag={r["flag"]}"
                )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
