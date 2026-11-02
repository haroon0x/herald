# 0002: License and product shape (open decision)

Date: 2026-11-02
Status: Open — placeholder chosen

Working choice: **open-source core under AGPL-3.0, paid "Pro" tier for extras**
(end-to-end encrypted sync across machines, polished voice input, narrowly
scoped auto-send policy builder, priority support). Reasoning:

- A client-side agent that handles personal communication is exactly the kind
  of software people refuse to run as a closed-source black box. Trust is the
  product; you cannot sell trust that has no receipts.
- The moat is not the Rust code (anyone can rewrite it). It is the safe
  provider adapters, the eval set, and reliability — all of which ship in the
  open and still confer a first-mover advantage.
- A closed-source local app is hard to distribute and undercuts the
  local-first principle. A purely free project has no revenue to sustain
  model hosting, adapters, and updates.

To confirm before any public release:

1. AGPL vs. Apache/MIT for the core (AGPL deters hosted clones; MIT maximizes
   adoption).
2. Trademark and name search for "Herald".
3. Pro scope defined so the free core is never crippled in a way that violates
   the safety promise (safety defaults are identical in both tiers).

