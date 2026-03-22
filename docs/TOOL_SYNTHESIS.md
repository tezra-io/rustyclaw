# Tool Synthesis Guide

RustyClaw's tool synthesis engine dynamically generates, validates, and manages tools at runtime. It runs inside the Elixir/OTP orchestration layer.

## Overview

Tool synthesis allows agents to create new tools on-the-fly when existing tools don't cover a specific need. Synthesized tools go through a probation period where their reliability is measured before full approval.

## Architecture

```
Tool Synthesis Engine (Elixir)
  ├── Synthesizer — generates tool code from natural language descriptions
  ├── Sandbox — isolated execution environment for untrusted tool code
  ├── Probation — tracks tool reliability during trial period
  ├── Registry (ETS) — stores tool definitions, schemas, and metrics
  ├── Composer — tool composition and dependency tracking
  └── Improver — iterative tool refinement with versioning
```

The synthesis API runs on port 4001 by default.

## Tool Lifecycle

1. **Synthesis** — Agent requests a new tool; Synthesizer generates code + schema
2. **Probation** — Tool is registered but tracked; invocations counted for reliability
3. **Approval** — After sufficient successful invocations, tool can be promoted
4. **Active** — Fully available to all agents
5. **Suspension** — Tool can be suspended if it starts failing

## CLI Commands

```bash
# List all synthesized tools with status and invocation counts
rustyclaw synth list

# Inspect a specific tool (schema, metrics)
rustyclaw synth inspect <tool-name>

# Promote a probationary tool to active
rustyclaw synth approve <tool-name>

# Suspend a failing tool
rustyclaw synth suspend <tool-name>

# Remove a synthesized tool
rustyclaw synth delete <tool-name>
```

## API Endpoints

The synthesis HTTP API is served by Bandit on port 4001:

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/health` | GET | Synthesis engine health check |
| `/api/tools` | GET | List all synthesized tools |
| `/api/tools/:name` | GET | Get tool details |
| `/api/tools/:name/approve` | POST | Approve a tool |
| `/api/tools/:name/suspend` | POST | Suspend a tool |

## Configuration

Tool synthesis behavior is configured via the Elixir application environment:

```elixir
config :rustyclaw_orchestrator,
  synth_api_port: 4001
```

## Design Details

See [TOOL_SYNTHESIS_DESIGN.md](TOOL_SYNTHESIS_DESIGN.md) for the full design document including sandbox isolation, probation thresholds, and composition strategies.
