# Weight-space programming pilot

A no-gradient experiment on `Qwen3-0.6B` and `Qwen3.5-0.8B`, asking whether an
LLM can read a classifier's weights, find a mechanism, and edit it to fix a
measured failure. The failure under study was the tier-1 classifier's
clarification decision on the 42-case set in `eval/commands.json`.

**Start with [FINDINGS.md](FINDINGS.md).** It has the result and the numbers.

## What it found

The clarify policy is linearly decodable at the **prompt** position
(5-fold CV 0.881 against a 0.762 majority baseline, permutation null 0.582) and
undecodable at the `needs_clarification` value, where no case carries a positive
margin. Emitting the boolean before the operation moved recall from 0/10 to
10/10 with identical rule text; the same rule text in the original field order
stayed at 0/10. So the obstacle was the field order, not the weights, and the
fix landed in `crates/herald-core/src/intent.rs`.

No weight edit was shipped: a constant direction at the output is a logit bias
and cannot separate classes, there is no direction at the decision token, and the
operation confusions that remained were singletons.

## Layout

| file | purpose |
| --- | --- |
| `FINDINGS.md` | results, method, reproduction commands |
| `run_wsp.py` | original harness: baseline / probe / apply, shared scoring |
| `eval35.py`, `prompt_*.py` | 42-case eval on the 0.8B, prompt variants |
| `decide35.py`, `probe_nl.py` | decision-token probe; linear vs quadratic CV |
| `reorder`/`calibrate`/`gate`.py | the experiment that produced the fix |
| `doctor_bench.py` | whether a small model can be the WSP *programmer* |
| `rescore.py` | eval audit: strict vs grammar-completed vs semantic |
| `out/*.json` | result files behind every number in FINDINGS.md |

## Running it

```bash
cd experiments/wsp
python3 -m venv .venv && .venv/bin/pip install -r requirements.txt
.venv/bin/python eval35.py --name baseline35 --dtype bf16
.venv/bin/python rescore.py out/baseline35.json
```

Model snapshots are not committed — `out/model` and `out/model35` are ignored.
Fetch them with `huggingface-cli download` into those paths, or point `--model`
at a local copy.
