## Summary

<!-- What does this change do, and why? One or two paragraphs. -->

## Linked issue

<!-- Every non-trivial PR should reference an issue. Use "Closes #123" or "Refs #123". -->

Closes #

## Safety checklist

Herald's safety model is enforced in code, not just documented. Check each box
or explain why it does not apply.

- [ ] I read [AGENTS.md](https://github.com/haroon0x/herald/blob/master/AGENTS.md) and the invariants still hold.
- [ ] This change does not give the model any execution authority; Rust still validates, authorizes, and executes.
- [ ] Message and provider content is still treated as untrusted data, and prompt-injection tests cover any new input path.
- [ ] Outbound actions still require explicit approval every time; no auto-send path was added.
- [ ] Writes remain idempotent and move through the action state machine; `uncertain` is reconciled, never retried blind.
- [ ] No credentials, tokens, message bodies, or personal data are logged.
- [ ] If this changed validation, policy, approval, or the state machine, new tests prove the safety property it relies on.

## Test plan

<!-- Commands run, and what they prove. Include new tests and manual checks. -->

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
```

## Screenshots

<!-- For CLI output or UI changes, paste before/after text or images. Remove secrets and personal data. -->
