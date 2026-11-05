# Contrib

Optional extras for running Herald. Nothing here is required to use the CLI.

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
