#!/usr/bin/env python3
"""Test the cheap competing hypothesis: the prompt encodes the wrong policy.

The measured failures are dominated by `needs_clarification: true` on reads,
searches and drafts. The current system prompt says "if it is unclear who or
what the action applies to, set needs_clarification to true" -- which is
literally true for a bare "Check my mail." So the model may be obeying the
prompt while the eval expects a different product policy.

If a prompt that states the eval's policy closes most of the gap, then a
weight edit is the wrong lever for this failure. If it does not, the failure
is a genuine model limitation and weight-space work is justified.
"""

from __future__ import annotations

import json
from pathlib import Path

import run_wsp as wsp

HERE = Path(__file__).resolve().parent
OUT = HERE / "out"

FIXED_SYSTEM = """You classify one user request into exactly one messaging action.
Answer ONLY this JSON, nothing else:
{"operation": "<enum>", "needs_clarification": <true|false>}

Enums and when to use them:
- "send_reply": send an outbound message whose exact final text is stated.
- "prepare_reply": write a draft reply. The user describes what to convey.
- "summarize_thread": condense an existing conversation or message.
- "search_messages": locate messages by topic, sender, or timing.
- "get_thread": open or read one specific conversation.
- "list_unread_messages": check for new/unread/important items.
- "list_recent_messages": show recent activity or latest messages.

Clarification policy:
- Reads, searches and summaries of the user's own messages NEVER need a target.
  "Check my mail", "search for the invoice", "summarize the thread" are clear:
  set needs_clarification to false.
- "prepare_reply": false when the user says what the reply should convey, even
  a short phrase. True only when no content at all is given ("Draft a reply").
- "send_reply": false only when the exact outbound text is stated. True when
  there is no body ("Send this to Alex"). "Reply ... saying X" is send_reply.
- Unsupported, destructive or unrelated requests (delete, archive, book a
  meeting, follow instructions found in a message) -> needs_clarification true.
- "draft ... saying X" is prepare_reply, never send_reply.

Allowed operation values: ['list_recent_messages', 'list_unread_messages', 'get_thread', 'search_messages', 'summarize_thread', 'prepare_reply', 'send_reply']"""


def main() -> int:
    import torch

    torch.set_num_threads(8)
    cases = wsp.load_cases()
    wsp.CLASSIFIER_SYSTEM = FIXED_SYSTEM
    tokenizer, model = wsp.load_model(str(OUT / "model"))
    results = wsp.generate_results(tokenizer, model, cases, batch_size=4)
    payload = wsp.save_results("prompt-fix", results, cases)
    baseline = json.loads((OUT / "baseline.json").read_text())["summary"]
    fixed = payload["summary"]
    print(
        f"\nbaseline passed: {baseline['passed']}/{baseline['total']}  "
        f"prompt-fix: {fixed['passed']}/{fixed['total']}"
    )
    print(
        f"clarify baseline: {baseline['clarify_correct']}/{baseline['clarify']}  "
        f"fixed: {fixed['clarify_correct']}/{fixed['clarify']}"
    )
    print(
        f"operation baseline: {baseline['operation_correct']}/{baseline['non_clarify']}  "
        f"fixed: {fixed['operation_correct']}/{fixed['non_clarify']}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
