# RustyClaw Operations Runbook

This runbook is for operators who maintain availability, security posture, and incident response.

Last verified: **February 18, 2026**.

## Scope

Use this document for day-2 operations:

- starting and supervising runtime
- health checks and diagnostics
- safe rollout and rollback
- incident triage and recovery

For first-time installation, start from [one-click-bootstrap.md](one-click-bootstrap.md).

## Runtime Modes

| Mode | Command | When to use |
|---|---|---|
| Foreground runtime | `rustyclaw daemon` | local debugging, short-lived sessions |
| Foreground gateway only | `rustyclaw gateway` | webhook endpoint testing |
| User service | `rustyclaw service install && rustyclaw service start` | persistent operator-managed runtime |

## Baseline Operator Checklist

1. Validate configuration:

```bash
rustyclaw status
```

2. Verify diagnostics:

```bash
rustyclaw doctor
rustyclaw channel doctor
```

3. Start runtime:

```bash
rustyclaw daemon
```

4. For persistent user session service:

```bash
rustyclaw service install
rustyclaw service start
rustyclaw service status
```

## Health and State Signals

| Signal | Command / File | Expected |
|---|---|---|
| Config validity | `rustyclaw doctor` | no critical errors |
| Channel connectivity | `rustyclaw channel doctor` | configured channels healthy |
| Runtime summary | `rustyclaw status` | expected provider/model/channels |
| Daemon heartbeat/state | `~/.rustyclaw/daemon_state.json` | file updates periodically |

## Logs and Diagnostics

### macOS / Windows (service wrapper logs)

- `~/.rustyclaw/logs/daemon.stdout.log`
- `~/.rustyclaw/logs/daemon.stderr.log`

### Linux (systemd user service)

```bash
journalctl --user -u rustyclaw.service -f
```

## Incident Triage Flow (Fast Path)

1. Snapshot system state:

```bash
rustyclaw status
rustyclaw doctor
rustyclaw channel doctor
```

2. Check service state:

```bash
rustyclaw service status
```

3. If service is unhealthy, restart cleanly:

```bash
rustyclaw service stop
rustyclaw service start
```

4. If channels still fail, verify allowlists and credentials in `~/.rustyclaw/config.toml`.

5. If gateway is involved, verify bind/auth settings (`[gateway]`) and local reachability.

## Safe Change Procedure

Before applying config changes:

1. backup `~/.rustyclaw/config.toml`
2. apply one logical change at a time
3. run `rustyclaw doctor`
4. restart daemon/service
5. verify with `status` + `channel doctor`

## Rollback Procedure

If a rollout regresses behavior:

1. restore previous `config.toml`
2. restart runtime (`daemon` or `service`)
3. confirm recovery via `doctor` and channel health checks
4. document incident root cause and mitigation

## Related Docs

- [one-click-bootstrap.md](one-click-bootstrap.md)
- [troubleshooting.md](troubleshooting.md)
- [config-reference.md](config-reference.md)
- [commands-reference.md](commands-reference.md)
