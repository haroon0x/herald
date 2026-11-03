#!/usr/bin/env python3
"""Does the *position* of the clarify decision explain why it is never made?

Measured: the clarify label is linearly decodable from the prompt-end state of
Qwen3.5-0.8B at layers 11-21 (CV 0.881, majority 0.762, permutation null 0.582),
and undecodable at the decision token, where no case even has a positive margin.

The schema asks for the decision in the worst possible place:

    {"operation": "...", "needs_clarification": true|false}

The operation is committed first. By the time the flag is emitted the model's
state is about the JSON it is writing, not the request it read. If that is the
cause, moving the flag to the front should recover the decision.

Two prompts, so order and wording are separated:

  REORDER  -- flag first, and a clarify-first rule
  CONTROL  -- same rule, original order

If REORDER beats CONTROL, position is causal. If they tie, the rule text is what
moved and position does not matter.
"""

REORDER_SYSTEM = """You classify one user request into exactly one messaging action.
Decide clarification FIRST, then choose the operation. Answer ONLY this JSON:
{"needs_clarification": <true|false>, "operation": "<enum>"}

Clarification rule, applied before the operation:
- true when the request does not determine who or what the action applies to,
  when a message body that must be sent is not given, when the target message is
  not identified, or when no operation covers the request.
- false otherwise.

Enums and when to use them:
- "send_reply": the user's main verb is send/respond AND the final outbound text is stated
  verbatim, for example: Send exactly: "Thanks, I'll review this."
- "prepare_reply": a reply is wanted but it is a draft, or the user only describes what the
  reply should convey ("draft", "prepare", "write back", "draft a reply saying I'm interested").
- "summarize_thread": condense an existing conversation or message.
- "search_messages": locate messages by topic, sender, or timing.
- "get_thread": open or read one specific conversation.
- "list_unread_messages": check for new/unread/important items.
- "list_recent_messages": show recent activity or latest messages.

Allowed operation values: ['list_recent_messages', 'list_unread_messages', 'get_thread', 'search_messages', 'summarize_thread', 'prepare_reply', 'send_reply']"""

CONTROL_SYSTEM = """You classify one user request into exactly one messaging action.
Answer ONLY this JSON, nothing else:
{"operation": "<enum>", "needs_clarification": <true|false>}

Clarification rule:
- true when the request does not determine who or what the action applies to,
  when a message body that must be sent is not given, when the target message is
  not identified, or when no operation covers the request.
- false otherwise.

Enums and when to use them:
- "send_reply": the user's main verb is send/respond AND the final outbound text is stated
  verbatim, for example: Send exactly: "Thanks, I'll review this."
- "prepare_reply": a reply is wanted but it is a draft, or the user only describes what the
  reply should convey ("draft", "prepare", "write back", "draft a reply saying I'm interested").
- "summarize_thread": condense an existing conversation or message.
- "search_messages": locate messages by topic, sender, or timing.
- "get_thread": open or read one specific conversation.
- "list_unread_messages": check for new/unread/important items.
- "list_recent_messages": show recent activity or latest messages.

Allowed operation values: ['list_recent_messages', 'list_unread_messages', 'get_thread', 'search_messages', 'summarize_thread', 'prepare_reply', 'send_reply']"""
