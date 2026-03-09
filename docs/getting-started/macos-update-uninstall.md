# macOS Update and Uninstall Guide

This page documents supported update and uninstall procedures for RustyClaw on macOS (OS X).

Last verified: **February 22, 2026**.

## 1) Check current install method

```bash
which rustyclaw
rustyclaw --version
```

Typical locations:

- Homebrew: `/opt/homebrew/bin/rustyclaw` (Apple Silicon) or `/usr/local/bin/rustyclaw` (Intel)
- Cargo/bootstrap/manual: `~/.cargo/bin/rustyclaw`

If both exist, your shell `PATH` order decides which one runs.

## 2) Update on macOS

### A) Homebrew install

```bash
brew update
brew upgrade rustyclaw
rustyclaw --version
```

### B) Clone + bootstrap install

From your local repository checkout:

```bash
git pull --ff-only
./bootstrap.sh --prefer-prebuilt
rustyclaw --version
```

If you want source-only update:

```bash
git pull --ff-only
cargo install --path . --force --locked
rustyclaw --version
```

### C) Manual prebuilt binary install

Re-run your download/install flow with the latest release asset, then verify:

```bash
rustyclaw --version
```

## 3) Uninstall on macOS

### A) Stop and remove background service first

This prevents the daemon from continuing to run after binary removal.

```bash
rustyclaw service stop || true
rustyclaw service uninstall || true
```

Service artifacts removed by `service uninstall`:

- `~/Library/LaunchAgents/com.rustyclaw.daemon.plist`

### B) Remove the binary by install method

Homebrew:

```bash
brew uninstall rustyclaw
```

Cargo/bootstrap/manual (`~/.cargo/bin/rustyclaw`):

```bash
cargo uninstall rustyclaw || true
rm -f ~/.cargo/bin/rustyclaw
```

### C) Optional: remove local runtime data

Only run this if you want a full cleanup of config, auth profiles, logs, and workspace state.

```bash
rm -rf ~/.rustyclaw
```

## 4) Verify uninstall completed

```bash
command -v rustyclaw || echo "rustyclaw binary not found"
pgrep -fl rustyclaw || echo "No running rustyclaw process"
```

If `pgrep` still finds a process, stop it manually and re-check:

```bash
pkill -f rustyclaw
```

## Related docs

- [One-Click Bootstrap](../one-click-bootstrap.md)
- [Commands Reference](../commands-reference.md)
- [Troubleshooting](../troubleshooting.md)
