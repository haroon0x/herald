# Herald desktop shell (Tauri v2)

A thin wrapper around `herald ui`: on startup it spawns the `herald` binary with
`ui --port 0`, reads the token-carrying URL from the child's first stdout line,
and opens that URL in a WebView. The page, loopback server, approval queue, and
the CLI's safety properties are unchanged — the shell adds no commands, no IPC,
and no second execution path. See [ADR 0010](../../docs/decisions/0010-local-web-ui.md).

This crate is its own Cargo workspace on purpose: it links WebKitGTK on Linux,
which the herald workspace's CI does not install.

## Build prerequisites (Linux)

```bash
sudo apt install libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf
```

## Run (development)

```bash
# in the herald repo root, build the CLI first so the shell can find it
cargo build -p herald-agent

# then, from this directory
HERALD_BIN=../target/debug/herald cargo run
```

Without `HERALD_BIN`, the shell looks for a `herald` binary next to its own
executable, then on `PATH` (e.g. after `cargo install --path ../agent`).

## Notes

- **Not in the herald workspace.** Its dependency graph (Tauri, WebKit) is
  deliberately isolated; `cargo build` / `cargo test` in the repo root never
  compiles it.
- **Release bundling is not set up yet** (`bundle.active: false` in
  `tauri.conf.json`). Bundling for a distributable build — icons, the `herald`
  binary as a Tauri sidecar, updater — is the follow-up once dev builds work.
- **Migrating to a native UI later** is per screen: each page behavior already
  maps to one `herald --json` command, so a screen becomes one IPC command
  behind the same approval queue. See ADR 0010.
