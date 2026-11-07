# 0005: Approval queue and idempotent sends

Date: 2026-11-04
Status: Accepted

## Context

The approval step is the only place where a human decides that a message should
exist. Two things have to be true for it to be worth anything:

1. The thing the user approves is the thing that gets sent.
2. A failure after the send does not produce a second copy.

Both are state problems, not prompt problems. The planner cannot help with
either, and a retry loop makes the second one worse.

## Decision

### The approval queue

Pending approvals live in SQLite, in a `pending_approvals` table added in schema
version 3 (the same migration adds `target_json` and `thread_id` to `actions`
so a target can be re-resolved and an uncertain send can be reconciled):

```sql
CREATE TABLE IF NOT EXISTS pending_approvals (
    action_id  TEXT PRIMARY KEY,
    provider   TEXT NOT NULL,
    target     TEXT NOT NULL,        -- serialized Target JSON
    thread_id  TEXT NOT NULL,        -- resolved, re-verified before send
    body       TEXT NOT NULL,        -- deleted on decision/expiry
    body_hash  TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
);
```

The row is written when a send is planned and policy allows it. The CLI prints
the provider, the resolved thread, the exact body, the hash, and the expiry.
`approve` loads the row by id; nothing else calls `send_reply`.

The body is retained only until the decision. `approve` and `reject` delete it,
and at startup expired pending rows are expired by the same cancel-and-audit path the
`approve` flow uses. The
hash stays in the `actions` table as `content_hash`, so the audit trail still
records what was approved without keeping the text.

### Idempotent sends

The action id is the client token. It is passed to `prepare_reply`, stored in
the approval row, sent as `X-Herald-Token`, and recorded in the Sent copy.

`send_reply` checks for the token before composing. A second call with the same
token returns the existing receipt instead of sending. After the send, the
adapter re-reads the Sent folder and sets `verified` only when the token or the
generated `Message-ID` is present.

### Uncertain, not failed

A timeout after the send means the message may exist. The action moves to
`uncertain`. `reconcile` calls `find_sent` and moves the action to `succeeded`
if the message is found, or `failed` if it is not. `reconcile` never calls
`send_reply`.

## Why this shape

- **A body that is not stored cannot be verified.** The approval has to keep
  the exact bytes, or `approve` has to re-derive them from the plan. Re-deriving
  means the model could produce a different body the second time. Storing is the
  safer option, with a deletion policy.
- **The resolved thread has to be in the row.** Re-resolving at approval time
  would let the target drift. Storing the resolution and re-checking it before
  send catches drift instead of hiding it.
- **The hash is the bridge.** It proves the body the user saw is the body being
  sent, and it identifies the sent copy during verification.
- **A token is the only portable idempotency key.** Message ids are assigned by
  the provider, and a timeout means we may not have one. The client token exists
  before the send, so it can be checked afterwards.
- **`uncertain` is a real state, not an error.** Treating a timeout as `failed`
  invites a retry that duplicates mail. Treating it as `succeeded` lies to the
  user. `uncertain` is the only honest option, and reconciliation is the only
  way out.

## Alternatives considered

| Alternative | Why not |
| --- | --- |
| Keep approvals in memory | A crash loses the queue, and the user cannot inspect it from another shell. |
| Do not store the body, re-plan on approve | The second plan can differ. The user approved bytes, not an intention. |
| Store the body forever | It is the most sensitive data in the system. Delete it on decision. |
| Retry a timeout once, then give up | A single retry is enough to duplicate a message. There is no safe automatic retry. |
| Rely on provider-side idempotency only | SMTP has no idempotency key. The header plus the Sent check is the portable version. |
| Mark a timeout as `failed` | The user would send again, producing the duplicate the design exists to prevent. |

## Consequences

- The queue is durable across restarts and visible from any shell.
- `approve` is idempotent in the sense that matters: the action state prevents a
  second execution, and the token prevents a second message if the state is
  bypassed by a bug.
- The Sent folder must be readable for verification. A provider that cannot
  read Sent cannot claim `verified = true`.
- The approval body is a retention liability. The TTL (72 hours by default) and
  the delete-on-decision rule bound it.
- Tests must cover: expired approval, changed target, body mismatch, token
  replay, timeout to `uncertain`, and reconcile in both directions.

## Open questions

- Should the TTL be per-provider rather than global? Not needed in v1; one
  global value is simpler to explain.
- Should `approvals` support a bulk reject? Not in v1. Bulk approve is
  deliberately absent.
