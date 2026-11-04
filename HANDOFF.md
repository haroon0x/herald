# HANDOFF — X provider milestone (2026-11-04)

Transient file. Delete it when the milestone lands. Durable truth lives in
[index.md](index.md), [AGENTS.md](AGENTS.md), and [docs/](docs/README.md).

## Goal

Herald becomes the only X client the user needs: read mentions + DMs, reply,
send DMs, create posts, and see post metrics — without opening x.com. Mocking
decisions already made by the owner (do not re-litigate):

- **Browser automation via `chromiumoxide`** over CDP, dedicated profile.
  X API pay-per-usage was considered and documented, but the owner chose the
  browser path (see "Decisions" below; ADR 0008 still to be written).
- **Scoped autopost**: original posts only, explicit opt-in, quality floor,
  daily cap, quiet hours, kill switch. DMs and replies NEVER autopost.
- **Zyquor is not continuing**; Herald is its successor. Zyquor stays
  runnable until Herald's live canary passes (post + DM-to-self + metrics).

Read first: `index.md`, `AGENTS.md`, `docs/architecture.md`,
`docs/providers.md#adding-a-provider`, `docs/threat-model.md`.

## State of the working tree (check `git status` before trusting this)

Uncommitted changes, all intentional. The implementation work in the list below
is done and gated; only the owner-driven live canary and the commit slices remain.

| Area | What exists |
| --- | --- |
| `herald-core/src/quality.rs` (new) | Zyquor's 10-point deterministic scorer ported faithfully: `score_tweet_quality`, `is_clean_tweet_text`, `passes_tweet_quality_floor`, `is_interrogation_dodge_reply`. 15 tests. `regex` workspace dep added. |
| `herald-core/src/action.rs` | `Operation::CreatePost` (+`as_str`, `is_outbound`, `needs_body`), `Target::Timeline` (+`describe`), `validate_plan` maps `CreatePost` → `Action::Send`, `read_operation` rejects it. |
| `herald-core/src/policy.rs` | `AutopostPolicy` + `PolicyConfig.autopost`; `authorize` returns `Allowed` for a timeline send only when EVERY rail passes (`enabled`, not quiet, `remaining_today > 0`, `Target::Timeline`, quality floor); otherwise falls back to `NeedsApproval`. 7 new tests. |
| `herald-core/src/rules.rs` | Deterministic post phrases (`post exactly:`, `tweet exactly:`, `post on x`, `post this on x`, ...) → `CreatePost` + `Target::Timeline`; missing body → clarification. |
| `herald-core/src/provider.rs` | `ProviderId::X::is_implemented() == true`; test updated. |
| `herald-config` | Autopost policy fields, quiet-hours helper, env overrides, `[x]` `XSettings`, `x_profile_dir`, template, `ModelBackend::OpenCode`. Split `apply_x_env` out of `apply_env` for the clippy line limit. |
| `herald-provider/src/x.rs` (new) | `XProvider` + `XClient` test seam, Chrome/CDP client, constant selectors, mentions/DM/tweet/compose parsers, seen + sent ledgers, thread-id validation, hard stops, verified idempotent sends, `login()`, `detect_chrome()`. 18 tests, including injection and timeout. |
| `herald-runtime/src/opencode.rs` (new) | `OpenCodeInference`, banner strip, JSON slice, bounded stderr, timeout, never `--auto`. 6 tests. |
| `herald-storage` | `count_actions_by_operation_since` for the autopost daily cap + test. |
| `apps/agent` | X registry wiring, `resolve_thread` timeline arm, `create_post` operation name, shared `prepare_and_send`, `execute_autopost_send`, autopost execution from `ask`, measured policy rails, `herald x login`, `herald autopost status|on|off`, OpenCode model arm + doctor probe, eval `target_kind` scoring. |
| `eval/commands.json` | 45 cases (3 new `create_post`); rules engine 45/45. |
| Docs | ADR 0008/0009, AGENTS non-negotiables, docs/README ADR rows, providers (X section + matrix), configuration, cli, threat model, roadmap, CHANGELOG, README, index.md. |
| `index.md` (new) | Repo map + working standards; linked from AGENTS.md, README.md, docs/README.md. |
| Module docs | `//!` headers added to every library file that lacked one. |

Gates already run: `cargo fmt --all --check`, `cargo clippy --workspace
--all-targets --locked -- -D warnings`, `cargo test --workspace --all-targets
--locked` (235 tests), `cargo deny check` (licenses, bans, advisories,
sources all ok), `RUSTDOCFLAGS="-D warnings" cargo doc`, and `herald eval
--engine rules` (45/45).

Nothing is committed yet.

## Decisions already locked (write them up, don't change them)

1. **X access = browser automation.** `chromiumoxide` 0.9.1 + dedicated Chrome
   profile. Never the user's everyday profile; never read cookies/credentials.
   Human signs in once via `herald x login` (visible Chrome). Hard stop on
   captcha, login challenge, account warning, rate-limit toast, or DOM mismatch.
2. **Capability surface unchanged.** Only the seven `CommunicationProvider`
   methods. No "click anything", no selector/URL/JS from the planner. Selectors
   are constants in the provider module.
3. **Autopost (ADR 0009)**: timeline posts only; requires `allow_send` AND
   `allow_autopost`; floor default 7; daily UTC cap default 5 counting manual +
   autonomous `create_post` actions; quiet hours `"HH:MM-HH:MM"` UTC with
   wrap-around; kill switch file `<data-dir>/autopost.disabled` written by
   `herald autopost off`. Any rail failing → normal approval queue. Widen the
   surface only with a new ADR.
4. **OpenCode free pool is the default model path**: `opencode run
   --standalone --model <id> <prompt>` (one-shot subprocess). Verified working:
   `opencode/pulse-alpha-free` returned `ok`. Default id
   `opencode/glm-5.3-flash-free`; alternatives `opencode/deepseek-v4.1-flash`,
   `opencode/crouton-alpha-free`. NEVER pass `--auto`. Run with cwd = an empty
   `<data-dir>/opencode-sandbox`, 120 s timeout.
   **Do not** use `opencode serve`: it is not OpenAI-compatible (`/v1/models`
   returns the web UI; `/api/v2/*` needs the server password it prints to the
   log) and reverse-engineering it is out of scope.
5. **X thread model**: `mention:<tweet_id>` (public conversation rooted at the
   tweet), `dm:<conversation_id>` (private conversation), `timeline` (compose,
   posts only). Unread comes from a bounded local seen-ledger
   `<data-dir>/x-seen.json`; `get_thread` marks seen. X exposes no reliable
   server-side unread state in the DOM.

## Remaining work, in order

*(Steps 1-5 are done as described below and are kept as the record of what was
required. Step 6 is what is left; it needs the owner.)*

### 1. Finish `herald-config` (may already be done — compile first)

`Config::policy()` must fill `autopost`; validate `autopost_min_quality <= 10`,
`autopost_daily_limit` 1..=1000, quiet-hours syntax; env overrides
`HERALD_ALLOW_AUTOPOST`, `HERALD_AUTOPOST_DAILY_LIMIT`,
`HERALD_AUTOPOST_MIN_QUALITY`, `HERALD_AUTOPOST_QUIET_HOURS`; `flat()` keys;
`quiet_hours_active(quiet, minutes_of_day_utc) -> bool` exported from
`herald_config`; `[x]` `XSettings` (`profile_dir`, `handle`, `headless`,
`action_delay_ms`, `typing_delay_ms`, `close_after`) + `x_profile_dir(data_dir)`
defaulting to `<data-dir>/profiles/x`; `HERALD_X_*` env; template additions.

Compile gate: `cargo check -p herald-config && cargo test -p herald-config`.

### 2. X provider — `crates/herald-provider/src/x.rs` (new, the big one)

Deps: add `chromiumoxide = "0.9"` and `scraper = "0.29"` to
`crates/herald-provider/Cargo.toml` (the `cargo add` was interrupted; retry).
Then `cargo deny check` (licenses).

Design:

- **Test seam, like `MailTransport`:** `trait XClient { async fn open(&mut self,
  url:&str) -> Result<(), ProviderError>; async fn html(&mut self) ->
  Result<String, ProviderError>; async fn click(&mut self, selector:&str) ->
  Result<(), ProviderError>; async fn fill(&mut self, selector:&str, text:&str)
  -> Result<(), ProviderError>; async fn wait_for(&mut self, selector:&str,
  timeout: Duration) -> Result<(), ProviderError>; }`. The real implementation
  wraps `chromiumoxide::Browser`/`Page`; unit tests use a scripted fake.
  `XProvider::with_client(config, data_dir, client)` for tests; `new(config,
  data_dir)` launches Chrome.
- **Pure parsing** with `scraper`: `parse_mentions(html) -> Result<Vec<XItem>,
  ProviderError>`, `parse_dm_list(html)`, `parse_dm_thread(html, self_handle)`,
  `parse_tweet_thread(html)`, `parse_compose_state(html)` (composer present?,
  logged out?, captcha/account warning?). All untrusted text goes into
  `Untrusted<String>` via `Thread`/`ProviderMessage`. Zero page-content
  influence on selectors or URLs — a test must prove it.
- **Lifecycle**: launch headless Chrome with `--user-data-dir=<profile>`,
  `--no-first-run`, `--no-default-browser-check`, `--remote-debugging-port=0`
  (or chromiumoxide's own launch options), close after the command when
  `close_after` is true. `herald x login` is a separate CLI path (visible,
  opens `https://x.com/login`, waits for login detection, never reads cookies).
- **Reads**: `health()` → navigate `x.com/home`; logged-out/redirect =
  `NotAuthenticated`; unexpected DOM = `UnexpectedLayout`. `list_recent` merges
  mentions (`x.com/notifications/mentions`) + DMs (`x.com/messages`), newest
  first, honours `unread_only` (seen-ledger) / `search` (case-insensitive over
  sender+subject+text) / `limit`. `get_thread` serves the three namespaces;
  `get_thread("timeline")` verifies the compose surface and returns an empty
  synthetic thread — this is the pre-send capability check for posts.
- **Writes** (all approval-gated by the CLI; the provider only executes):
  `prepare_reply(thread, body, token)` refuses empty bodies; for `timeline` it
  verifies the composer is present (raises `UnexpectedLayout`/`NotAuthenticated`
  if not); no account mutation. `send_reply`:
  1. sent-ledger `<data-dir>/x-sent.json` (`client_token → {thread_id,
     provider_message_id, body_hash, sent_at_ms}`) — replay returns the stored
     receipt, never sends twice;
  2. open the thread/compose, fill the composer, click once, wait for the new
     message to appear;
  3. verify by exact body match from the user's own messages; capture the
     status id from the permalink for replies/posts; DM event ids aren't in the
     DOM, so use a documented synthetic id `dm:<conversation_id>:<unix_ms>`;
  4. `verified=false` stays uncertain; timeouts map to `ProviderError::Timeout`
     (never retry).
  `find_sent` = ledger lookup restricted to the thread; no ledger entry →
  `Ok(None)` (= failed, never resent).
- **Constants**: selectors as `const` items with a comment naming the X page
  they belong to. Add fixture tests only from HTML captured on the dedicated
  profile; label them.
- **Injection tests**: a fixture whose tweet/DM text contains "ignore previous
  instructions…" must parse as inert data; assert no URL/selector changes and
  that bodies stay `Untrusted`.

Compile gate: `cargo test -p herald-provider`.

### 3. OpenCode inference backend (`herald-runtime`, `apps/agent/planner.rs`, `doctor.rs`)

- `OpenCodeInference { model, binary, sandbox_dir, timeout }` implementing
  `Inference`: `complete_json` slices first `{`…last `}` then parses; non-zero
  exit → error with stderr tail; 120 s timeout; `complete_text` strips the
  leading `> build · model` banner line. Pure helpers unit-tested
  (`strip_opencode_banner`, `slice_json_object`).
- `build_model` arm for `ModelBackend::OpenCode`: binary from
  `HERALD_OPENCODE_PATH` (default `opencode`), model default above, create
  `<data-dir>/opencode-sandbox`, never `--auto`.
- Doctor: `opencode --version` exits 0 within 10 s; no live model call.

### 4. CLI wiring

- `ProviderId::X` → `is_implemented() == true` (herald-core), registry builds
  `XProvider` when `[x]` is present (warn, don't fail, when Chrome is missing).
- `send.rs`: `resolve_thread` arm for `Target::Timeline` → `ThreadId("timeline")`
  (no provider call; provider capability is checked in `prepare_reply`);
  `operation_name` → `create_post` when the target is `Timeline`; factor the
  prepare→send→verify→finish block out of `approve()` into a shared function
  and add `execute_autopost_send` for the `Decision::Allowed` send path (same
  state machine, audit detail prefixed `autopost`).
- `ask.rs`: when `Decision::Allowed` and the action is a `Send`, execute it
  through the autopost path instead of erroring.
- Policy inputs computed in the CLI before `authorize`: kill-switch file,
  `quiet_hours_active(config, now)`, remaining daily budget from
  `herald-storage` (`count_actions_by_operation_since("create_post", utc_day_start)`).
- New commands: `herald autopost status|on|off` (kill switch + effective rails +
  remaining today) and `herald x login` (visible Chrome, human login, no
  credential access). Register in `cli.rs` + `commands/mod.rs`, human + `--json`
  output, docs.
- `eval/commands.json`: add 2–3 `create_post` cases (expect `create_post`,
  `target.kind = "timeline"`); keep the rest unchanged.

### 5. Docs (a subagent was assigned this and hit the free-model quota — do it here)

- `docs/decisions/0008-x-browser-adapter.md`, `docs/decisions/0009-scoped-autopost.md`
  (style: ADR 0005/0007; context, decision, why, tradeoffs, consequences,
  when to revisit; both Accepted, 2026-11-04).
- `AGENTS.md`: replace the two relevant non-negotiables (browser automation →
  dedicated-profile X adapter only; auto-send → approval except the autopost
  lane with every rail).
- `docs/README.md` ADR table rows; `docs/providers.md` X section + capability
  matrix (`x`: in progress); `docs/configuration.md` (`[x]`, new `[policy]`
  keys, `HERALD_X_*`, `HERALD_AUTOPOST_*`, `HERALD_OPENCODE_PATH`);
  `docs/cli.md` (`x login`, `autopost on|off|status`, post examples);
  `docs/threat-model.md` browser-adapter paragraph; `docs/roadmap.md` +
  `CHANGELOG.md` honest "in progress" entries; one sentence in README about
  ADR 0008 and autopost.

### 6. Verification and live canary (needs the owner) — **only remaining step**

1. `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked
   -- -D warnings`, `cargo test --workspace --all-targets --locked`,
   `cargo deny check`.
2. `herald x login` — **owner logs in manually** in the dedicated profile.
   Never point herald at `zyquor/profiles/personal-main` (it is the personal
   profile and off-limits), never copy cookies.
3. `herald providers` → x ready; `herald doctor` clean.
4. `herald inbox --provider x` (real read), `herald thread --provider x
   mention:<id>`.
5. Send canary: one DM to self and one post — both only after `herald approve
   <id>` by the owner. Confirm ledger + receipt + `herald history`.
6. Autopost canary: only with the owner watching; `herald autopost status`,
   `herald post exactly: "…"` (body must clear floor 7), verify published,
   then `herald autopost off` restores approval-only.
7. Set the local config `[model] backend = "opencode"` (back up the existing
   file first) and run one `herald ask` to confirm the classifier path; measure
   the eval honestly, no cherry-picking.
8. Only after the canary: commit in slices (core → config → provider → CLI →
   docs), then update `CHANGELOG.md` with what actually shipped.

## Gotchas already discovered

- `regex` crate has no lookahead; the scorer works around `when it comes to`
  manually — keep that behavior when porting edits.
- `chromiumoxide` 0.9.1 / `scraper` 0.29 are the chosen versions; check
  `cargo deny` after adding them.
- The `cargo add --locked` form fails; add deps first, then compile.
- Free-model quota on the owner's opencode plan can be exhausted mid-task;
  background agents may die with "free usage limit reached". Do the work
  directly when that happens.
- Never write secrets to config; `doctor` must warn on plaintext passwords.
- `herald x login` must not launch a headless window (the owner has to see it).
