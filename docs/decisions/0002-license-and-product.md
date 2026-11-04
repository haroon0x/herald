# 0002: License and product shape

Date: 2026-11-02, decided 2026-11-04
Status: Accepted — AGPL-3.0-or-later

## Decision

The Herald core is licensed **AGPL-3.0-or-later**. The workspace already
declares `license = "AGPL-3.0-or-later"` and the repository ships the AGPL text
as `LICENSE`.

A paid tier remains possible. It is **not part of v1**, no code is gated behind
it, and it cannot change a safety default. If a Pro tier ever ships, the free
core keeps the same approval requirement, the same local inference default, and
the same provider capabilities.

## Why AGPL

- **The trust argument.** A local agent that holds mail credentials and can
  send as the user has to be inspectable. A closed-source build asks for a level
  of trust that this category of software has not earned.
- **Reciprocity.** AGPL keeps improvements available to users, including when
  someone runs Herald as a network service. That is the failure mode a
  permissive license does not address: a hosted fork that takes the adapters,
  the eval set, and the safety work and gives nothing back.
- **The moat is not the source.** The durable assets are the provider adapters,
  the eval corpus, the failure taxonomy, and the reliability work. All of those
  ship in the open under AGPL and still confer an advantage, because they are
  expensive to reproduce and easy to verify.
- **Distribution.** A local-first tool has to be easy to obtain and audit.
  Source-available is not the same as open source; AGPL is an OSI-approved
  license with a clear compliance story.

## Alternatives considered

| Option | Why not |
| --- | --- |
| MIT or Apache-2.0 | Maximizes adoption, but allows a closed hosted fork with no reciprocity. The safety work would be the first thing stripped. |
| Source-available, custom license | Not open source, which contradicts the trust argument and complicates packaging. |
| Dual license with a commercial option | Premature. It requires contributor agreements, a sales surface, and legal work that v1 does not need. |
| Closed core with an open client | A local app that cannot be audited is the thing this project exists to avoid. |

## Consequences

- Downstream users who modify and distribute Herald must publish their changes
  under the same terms. Running it locally for yourself has no such obligation.
- Some companies ban AGPL in their dependency policies. That is a real adoption
  cost, accepted in exchange for reciprocity.
- The repository must keep `LICENSE`, the Cargo `license` field, and the README
  badge in agreement. `cargo-deny` enforces the dependency side.

## What remains

1. **Trademark search for "Herald."** Not done. A name search and, if needed, a
   rename must happen before a public announcement. The code and docs use
   "Herald" as a working name.
2. **Pro tier scope.** Not defined and not in v1. If it ships, the boundary is
   sync, packaging convenience, and support. It cannot include auto-send,
   weaker approval, or a cloud default for inference.
3. **Contributor licensing.** Not decided. A DCO is the simplest option and
   matches the AGPL choice; a CLA would only be needed for dual licensing.

Until 1 and 2 are resolved, the README and this record describe the license as
settled and the product shape as open.
