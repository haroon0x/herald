# Contrib

Optional extras for running Herald. Nothing here is required to use the CLI.

## Package-manager manifests

The release workflow (`.github/workflows/release.yml`) publishes
`herald-v<version>-<target>.{tar.gz,zip}` archives and a `SHA256SUMS` file
next to them. The two manifests below point at those assets.

### Homebrew (`homebrew/herald.rb`)

macOS and Linux. Before the first release, fill in the four `sha256` values
from the release's `SHA256SUMS` (one per archive), then publish the formula in
a Homebrew tap owned by the project. The archives contain a single `herald`
binary, so `bin.install "herald"` is all the formula needs.

```bash
brew tap <org>/herald            # once the tap exists
brew install herald
```

### Scoop (`scoop/herald.json`)

Windows (x86_64). Before the first release, fill in the `hash` value with the
release's `SHA256SUMS` entry for
`herald-v1.0.0-x86_64-pc-windows-msvc.zip`, then publish the file in a Scoop
bucket owned by the project. `checkver`/`autoupdate` follow the GitHub release
tag and re-read `SHA256SUMS` for the new hash.

```powershell
scoop bucket add herald https://github.com/<org>/scoop-herald.git
scoop install herald
```

### Debian package (`debian/build-deb.sh`)

For Debian/Ubuntu (x86_64). The release workflow builds
`herald_<version>_amd64.deb` with this script and publishes it next to the
archives; `SHA256SUMS` covers it. Build it locally with:

```bash
cargo build --release -p herald-agent
sh contrib/debian/build-deb.sh target/release/herald 1.0.0 ./dist
sudo dpkg -i dist/herald_1.0.0_amd64.deb
```

### Signed release artifacts

`SHA256SUMS` is signed with [minisign](https://jedisct1.libsodium.org/) when
the release workflow has a key configured; the signature (`SHA256SUMS.sig`)
is published next to the archives. To enable it (one-time setup):

```bash
minisign -G -p herald.pub -s herald.key   # keep herald.key secret, offline
```

Add two repository secrets: `MINISIGN_SECRET_KEY` (the contents of
`herald.key`) and, if the key has a password, `MINISIGN_PASSWORD`. The public
key (`herald.pub`) is distributed out of band (repo README); users verify
before trusting the checksums:

```bash
HERALD_MINISIGN_PUB=herald.pub ./install.sh
```

Without `HERALD_MINISIGN_PUB`, `install.sh` checksums only (HTTPS), exactly
as documented in its header.

### Unsigned downloads

`install.sh` (repo root) downloads the matching archive for Linux/macOS and
verifies it against `SHA256SUMS` before installing. The checksum file itself
is HTTPS-only; see the note in `install.sh` for what that does and does not
guarantee.

## systemd user unit for `herald ui`

`systemd/herald-ui.service` runs `herald ui` as a systemd **user** service on
port 8787, restarting on failure. It assumes the `herald` binary is installed
at `%h/.local/bin/herald` (the default of `install.sh`).

Install and enable:

```bash
mkdir -p ~/.config/systemd/user ~/.local/bin
cp /path/to/herald ~/.local/bin/herald
cp contrib/systemd/herald-ui.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now herald-ui.service
```

To have the service start at boot without an active login session:

```bash
loginctl enable-linger "$USER"
```

On every start `herald ui` writes the full URL (including the per-run token)
to `~/.local/share/herald/ui-url`, mode 0600. To open the UI:

```bash
cat ~/.local/share/herald/ui-url
```

or read it from the journal:

```bash
journalctl --user -u herald-ui.service -n 20
```

The token changes on every restart; always use the current contents of the
file. `systemctl --user status herald-ui.service` shows whether it is up.
