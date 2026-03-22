# Plugin Development Guide

RustyClaw's plugin system runs inside the Elixir/OTP orchestration layer. Plugins extend RustyClaw with custom capabilities, integrations, and automation workflows.

## Prerequisites

- Elixir >= 1.17 and OTP installed
- RustyClaw daemon running with the Elixir orchestrator enabled (`rustyclaw daemon`)

## Plugin Architecture

Plugins are managed by the Elixir orchestrator and communicate with the Rust core via the BTW bridge:

```
Plugin Manager (Elixir)
  ├── Progress Tracker — loop detection, stuck worker detection
  ├── Retry Scheduler — exponential backoff, fallback routing
  ├── Task Orchestrator — dev session orchestration, quality gates
  ├── Task Queue — priority queue with auto-assignment
  └── Worker Supervisor — dynamic worker processes
```

The plugin API runs on port 4002 by default.

## CLI Commands

```bash
# Check plugin system status
rustyclaw doctor          # shows orchestrator health including plugins

# Check daemon status
rustyclaw status          # shows orchestrator and BTW bridge status
```

## Plugin API Endpoints

The plugin HTTP API is served by Bandit on port 4002:

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/health` | GET | Plugin system health check |
| `/api/plugins` | GET | List loaded plugins |
| `/api/tasks` | GET | List queued/active tasks |

## Configuration

Plugin behavior is configured via the Elixir application environment in `elixir/rustyclaw_orchestrator/config/`:

```elixir
config :rustyclaw_orchestrator,
  plugin_api_port: 4002,
  rust_bridge: [base_url: "http://localhost:4200"]
```

## Development

See [AGENT_PLUGIN_SYSTEM_DESIGN.md](AGENT_PLUGIN_SYSTEM_DESIGN.md) for the full design document including plugin lifecycle, worker management, and retry strategies.
