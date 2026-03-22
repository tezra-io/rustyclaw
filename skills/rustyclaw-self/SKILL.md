# RustyClaw — Self-Knowledge Skill

> Use this skill when someone asks what RustyClaw is, what it can do, how it works,
> its architecture, CLI commands, configuration, security model, or any question
> about RustyClaw's own capabilities and design.

**Version:** 0.1.7
**Minimum Rust:** 1.91

---

## What Is RustyClaw?

RustyClaw is a multi-agent AI runtime that pairs a high-performance **Rust core** with an **Elixir/OTP orchestration layer**. The Rust layer handles LLM providers, messaging channels, tool execution, security, and memory. The Elixir layer manages agent lifecycle, supervision, inter-agent messaging, and capability-based routing using OTP primitives.

RustyClaw was forked from [ZeroClaw](https://github.com/zeroclawlabs/zeroclaw) and extended with Elixir/OTP multi-agent orchestration.

---

## Core Architecture

### Two-Layer Design

```
┌─────────────────────────────────────────────────────┐
│                Elixir/OTP Layer                     │
│                                                     │
│  AgentRegistry ── AgentCoordinator                  │
│                       │                             │
│              AgentSupervisor ── SubAgentSession      │
│                   │                                 │
│           AgentServer (GenServer x N)               │
│                   │                                 │
│              RustBridge                             │
└───────────────────┬─────────────────────────────────┘
                    │ JSON / HTTP (localhost)
┌───────────────────▼─────────────────────────────────┐
│                  Rust Core                          │
│                                                     │
│  Channels (19)  Providers (15)  Tools (37)          │
│  Security       Memory          Gateway             │
│  Cron           Peripherals     Observability       │
└─────────────────────────────────────────────────────┘
```

### Rust Core Responsibilities

- **Channels** — 19 messaging platform integrations
- **Tools** — 37 built-in agent-callable capabilities
- **Providers** — 15 LLM provider backends
- **Security** — Policy engine, sandboxing, prompt guard, secrets, approval system
- **Memory** — Multiple storage backends (SQLite, Markdown, Qdrant, Postgres)
- **Gateway** — Axum HTTP server with webhooks, SSE, and WebSocket
- **Cron** — Task scheduling (cron expressions, one-shot, interval)
- **Observability** — Prometheus, OpenTelemetry, logging, runtime traces
- **Peripherals** — Hardware board interfaces (STM32, RPi GPIO, Arduino, ESP32)
- **Approval** — Human-in-the-loop approval system
- **Hooks** — Event-driven lifecycle hooks
- **Cost** — Token usage and cost tracking

### Elixir/OTP Layer Responsibilities

- **Agent lifecycle** — spawn, stop, restart agents (DynamicSupervisor)
- **Agent registry** — name → pid mapping (Elixir Registry, `:unique` mode)
- **Inter-agent messaging** — BEAM message passing (replaces custom AgentBus)
- **Capability-based routing** — find best agent for a task (AgentCoordinator)
- **Delegation ACL** — per-agent `delegates_to` allowlists
- **Session persistence** — ETS-backed session lifecycle (SubAgentSession)
- **Plugin system** — extensible plugin manager with worker supervision
- **Tool synthesis** — dynamic tool generation at runtime

### BTW Bridge (Rust ↔ Elixir)

The two layers communicate over a localhost HTTP bridge. The Rust gateway exposes endpoints; the Elixir layer calls them via `Req`. The bridge is the **only** coupling point between layers — kept narrow and well-typed.

Future option: migrate to Erlang Port (stdin/stdout JSON framing) if latency matters.

### Unified Daemon

`rustyclaw daemon` starts **both** layers as supervised child processes:
- Rust components: gateway, channels, heartbeat, scheduler
- Elixir orchestrator: agent lifecycle, plugins, tool synthesis
- Health checks every 15 seconds; auto-restarts with exponential backoff
- Degraded mode: if Elixir is unavailable, runs single-agent with a warning
- `--no-elixir` flag to explicitly skip the orchestrator

---

## Messaging Channels (19)

| # | Channel | Module |
|---|---------|--------|
| 1 | Telegram | `telegram` |
| 2 | Discord | `discord` |
| 3 | Signal | `signal` |
| 4 | Slack | `slack` |
| 5 | WhatsApp (Cloud API) | `whatsapp` |
| 6 | WhatsApp Web | `whatsapp_web` (feature flag) |
| 7 | Matrix | `matrix` (feature flag) |
| 8 | IRC | `irc` |
| 9 | iMessage | `imessage` |
| 10 | Email (IMAP/SMTP) | `email_channel` |
| 11 | Nostr | `nostr` |
| 12 | DingTalk | `dingtalk` |
| 13 | Lark/Feishu | `lark` (feature flag) |
| 14 | Linq (iMessage/RCS/SMS) | `linq` |
| 15 | Mattermost | `mattermost` |
| 16 | Nextcloud Talk | `nextcloud_talk` |
| 17 | QQ | `qq` |
| 18 | Wati (WhatsApp Business) | `wati` |
| 19 | ClawdTalk | `clawdtalk` |

Plus a CLI channel for interactive terminal sessions and a transcription module for voice/audio.

Channels use per-sender conversation history (max 50 messages), concurrent message processing, and exponential-backoff reconnection.

---

## LLM Providers (15)

Anthropic, OpenAI, Google Gemini, Ollama, AWS Bedrock, Azure OpenAI, Groq, Mistral, Cohere, Together, OpenRouter, DeepSeek, Fireworks, Cerebras, xAI.

Plus Chinese provider support: GLM (Zhipu), Qwen (Alibaba), MiniMax, Moonshot (Kimi), Qianfan (Baidu), and ZAI.

Provider selection precedence:
1. `RUSTYCLAW_PROVIDER` env var (always wins)
2. `PROVIDER` env var (legacy fallback)
3. `default_provider` in `config.toml`

---

## Tools (37)

### File & Search
- `file_read` — Read file contents
- `file_write` — Write/create files
- `file_edit` — Surgical text edits
- `glob_search` — Glob pattern file search
- `content_search` — Content/grep search across files
- `pdf_read` — Extract text from PDFs
- `image_info` — Image metadata and analysis

### Shell & System
- `shell` — Shell command execution (policy-gated)
- `git_operations` — Git operations
- `screenshot` — Screen capture

### Web
- `web_fetch` — Fetch and extract web content
- `web_search` — Web search
- `http_request` — HTTP API calls (domain-allowlisted)
- `browser` — Browser automation (multiple backends)
- `browser_open` — Open URLs in system browser

### Memory
- `memory_store` — Store memories
- `memory_recall` — Recall memories (hybrid vector + keyword search)
- `memory_forget` — Delete memories

### Scheduling
- `cron_add` — Add cron/scheduled tasks
- `cron_list` — List scheduled tasks
- `cron_remove` — Remove tasks
- `cron_run` — Manually trigger a task
- `cron_runs` — View task run history
- `cron_update` — Update task schedule
- `schedule` — One-shot scheduling

### Agent & Delegation
- `delegate` — Delegate tasks to sub-agents

### Skills
- `skill_create` — Create new skills
- `skill_edit` — Edit existing skills
- `skill_delete` — Delete skills
- `skill_patch` — Patch skill files

### Configuration
- `model_routing_config` — Configure model routing via natural language
- `proxy_config` — Configure proxy settings

### Hardware (feature-gated)
- `hardware_board_info` — Query board information
- `hardware_memory_map` — Read memory maps
- `hardware_memory_read` — Read hardware memory

### Integrations
- `composio` — Composio managed OAuth tools
- `pushover` — Push notifications
- `synth_proxy` — Proxy to synthesized tools
- `cli_discovery` — Discover CLI tools

---

## Plugin System

The plugin system runs inside the Elixir orchestrator and communicates with the Rust core via the BTW bridge.

### Plugin Architecture
```
Plugin Manager (Elixir)
  ├── Progress Tracker — loop detection, stuck worker detection
  ├── Retry Scheduler — exponential backoff, fallback routing
  ├── Task Orchestrator — dev session orchestration, quality gates
  ├── Task Queue — priority queue with auto-assignment
  └── Worker Supervisor — dynamic worker processes
```

### Plugin API
- HTTP API on port 4002
- Endpoints: `/health`, `/api/plugins`, `/api/tasks`
- Configured via Elixir application environment

---

## Agent Swarm / Multi-Agent System

### Elixir OTP Components

| Component | OTP Primitive | Role |
|-----------|--------------|------|
| `AgentRegistry` | Elixir `Registry` (`:unique`) | Maps agent name → pid |
| `AgentSupervisor` | `DynamicSupervisor` | Spawns/terminates agent processes, `:one_for_one` restart strategy |
| `AgentServer` | `GenServer` | One per agent instance; manages health, tasks, messaging |
| `AgentCoordinator` | `GenServer` | Capability routing, delegation ACL, strategies |
| `SubAgentSession` | ETS-backed struct | Session lifecycle: pending → active → completed/failed |
| `RustBridge` | `GenServer` | HTTP bridge to Rust core with retry/backoff |

### Agent Definitions
- Stored as YAML frontmatter + markdown files in `~/.rustyclaw/agents/*.md`
- Parsed with `YamlElixir` + validated with `NimbleOptions`
- Fields: name, model, capabilities, delegates_to, persistent

### Delegation Strategies
- `:sequential` — try agents in order
- `:fanout` — broadcast to all matching
- `:first_available` — route to first idle agent

### Health Model
3-state per agent: `:healthy | :degraded | :unhealthy`
Periodic health checks via `Process.send_after(self(), :health_check, 30_000)`.

### Sub-Agent Configuration (config.toml)
```toml
[agents.researcher]
provider = "openrouter"
model = "anthropic/claude-sonnet-4-6"
system_prompt = "You are a research assistant."
agentic = true
allowed_tools = ["web_search", "http_request", "file_read"]
max_iterations = 8
max_depth = 3
```

---

## Tool Synthesis Pipeline

Tool synthesis allows agents to create new tools on-the-fly when existing tools don't cover a need.

### Architecture
```
Tool Synthesis Engine (Elixir)
  ├── Synthesizer — generates tool code from natural language
  ├── Sandbox — isolated execution for untrusted tool code
  ├── Probation — tracks reliability during trial period
  ├── Registry (ETS) — stores definitions, schemas, metrics
  ├── Composer — tool composition and dependency tracking
  └── Improver — iterative refinement with versioning
```

### Tool Lifecycle
1. **Synthesis** — Agent requests a tool; Synthesizer generates code + schema
2. **Probation** — Registered but tracked; invocations counted for reliability
3. **Approval** — After sufficient successful invocations, promoted
4. **Active** — Fully available to all agents
5. **Suspension** — Suspended if it starts failing

### API
- HTTP on port 4001
- Endpoints: `/health`, `/api/tools`, `/api/tools/:name`, `/api/tools/:name/approve`, `/api/tools/:name/suspend`

---

## Security Model

### Defense Layers

1. **Autonomy Levels** — `read_only`, `supervised`, `full`
2. **Workspace Boundaries** — `workspace_only` restricts file access to workspace
3. **Command Allowlists** — explicit `allowed_commands` for shell execution
4. **Forbidden Paths** — system paths and sensitive dotdirs blocked by default
5. **Sandboxing** — OS-level process isolation (Docker, Firejail, Bubblewrap, Landlock)
6. **Policy Engine** — deny-by-default for channels, domains, and tools
7. **Prompt Guard** — prompt injection detection and defense
8. **Content Scanner** — threat category classification with severity levels
9. **Leak Detector** — credential and secret leak detection
10. **Sentinel Engine** — sanitization and redaction of sensitive data
11. **Human-in-the-Loop Approval** — approval gating for medium/high-risk operations

### OTP (One-Time Password) Security
- TOTP-based gating for sensitive actions and domains
- Configurable gated actions: `shell`, `file_write`, `browser_open`, `browser`, `memory_forget`
- Domain categories: `banking`, `medical`, `government`, `identity_providers`
- Token TTL and cache windows configurable

### Emergency Stop (E-Stop)
Multi-level emergency stop system:
- `kill-all` — Stop all agent activity
- `network-kill` — Block all network access
- `domain-block` — Block specific domain patterns
- `tool-freeze` — Freeze specific tools
- Persistent state (survives restart), fail-closed on corruption
- OTP required to resume (configurable)

### Pairing & Authentication
- Gateway requires pairing by default (`require_pairing = true`)
- Device pairing for channel authentication
- Secret store with encryption at rest (OS keychain integration)

### Audit Logging
- Security-relevant events recorded for forensic review
- Structured audit events with types and timestamps

---

## Memory System

### Backends
| Backend | Storage | Features |
|---------|---------|----------|
| `sqlite` | Local SQLite DB | Default, fast, hybrid vector + keyword search |
| `lucid` | SQLite + workspace files | Enhanced recall with workspace awareness |
| `markdown` | Flat `.md` files | Human-readable, git-friendly |
| `qdrant` | Qdrant vector DB | Dedicated vector search |
| `postgres` | PostgreSQL (feature flag) | Scalable, remote-capable |
| `none` | No storage | Ephemeral mode |

### Features
- **Auto-save** — Persists user-stated inputs (assistant outputs excluded)
- **Hybrid search** — Configurable vector weight (0.7) + keyword weight (0.3)
- **Embeddings** — OpenAI or custom endpoint, configurable model and dimensions
- **Categories** — Memory entries classified by category
- **Session isolation** — Per-session memory context
- **Response cache** — Caches recent responses
- **Memory hygiene** — Cleanup and retention management
- **Scanning** — Scanned/indexed memory for fast recall

### Embedding Routes
```toml
[memory]
embedding_model = "hint:semantic"

[[embedding_routes]]
hint = "semantic"
provider = "openai"
model = "text-embedding-3-small"
dimensions = 1536
```

---

## Autonomy Levels

| Level | Shell | File Write | Approval |
|-------|-------|-----------|----------|
| `read_only` | ❌ | ❌ | N/A |
| `supervised` | ✅ (gated) | ✅ (gated) | Required for medium-risk |
| `full` | ✅ | ✅ | Skipped for medium-risk |

Additional controls:
- `max_actions_per_hour` — per-policy action budget (default: 20)
- `max_cost_per_day_cents` — spending guardrail (default: 500)
- `auto_approve` — tool operations always auto-approved
- `always_ask` — tool operations that always require approval
- `allowed_roots` — additional directories allowed outside workspace

---

## Hardware & Peripherals

### Supported Boards
- **STM32 Nucleo** (nucleo-f401re) — Serial + probe-rs flashing
- **Raspberry Pi GPIO** (rpi-gpio) — Native GPIO access
- **Arduino** (arduino-uno) — Serial + upload
- **ESP32** — Serial communication

### Features
- USB device discovery and introspection
- Chip identification via probe-rs / ST-Link
- Firmware flashing (STM32, Arduino)
- Datasheet RAG — index PDF schematics for AI-assisted pin lookups
- Boards become agent tools when enabled

### Configuration
```toml
[peripherals]
enabled = true
datasheet_dir = "docs/datasheets"

[[peripherals.boards]]
board = "nucleo-f401re"
transport = "serial"
path = "/dev/ttyACM0"
baud = 115200
```

---

## Observability

### Backends
| Backend | Purpose |
|---------|---------|
| `noop` | No-op (disabled) |
| `log` | Structured logging |
| `prometheus` | Prometheus metrics export |
| `otel` / `opentelemetry` / `otlp` | OpenTelemetry OTLP HTTP export |
| `verbose` | Detailed console output |
| `multi` | Compose multiple backends |

### Runtime Traces
- JSONL format for debugging tool-call failures and malformed payloads
- Modes: `none`, `rolling` (capped), `full`
- Query with `rustyclaw doctor traces`

### Cost Tracking
- Per-request cost estimates
- Daily and monthly spending limits
- Warning thresholds with configurable percentages

---

## Full CLI Reference

### Core Commands
| Command | Description |
|---------|-------------|
| `rustyclaw onboard` | Quick setup (provider + key) |
| `rustyclaw onboard --interactive` | Full wizard (provider, channels, security, memory, etc.) |
| `rustyclaw onboard --channels-only` | Reconfigure channels only |
| `rustyclaw agent` | Interactive agent chat session |
| `rustyclaw agent -m "..."` | Single-shot message |
| `rustyclaw agent -p <provider>` | Use specific provider |
| `rustyclaw agent --model <model>` | Use specific model |
| `rustyclaw agent --peripheral <board:path>` | Attach hardware peripheral |
| `rustyclaw gateway` | HTTP/WebSocket gateway server |
| `rustyclaw gateway -p <port>` | Gateway on specific port |
| `rustyclaw daemon` | Full runtime (gateway + channels + scheduler + Elixir) |
| `rustyclaw daemon --no-elixir` | Degraded single-agent mode |
| `rustyclaw status` | System status and config |
| `rustyclaw doctor` | Run diagnostics |
| `rustyclaw providers` | List supported providers |

### Service Management
| Command | Description |
|---------|-------------|
| `rustyclaw service install` | Install as OS service (launchd/systemd) |
| `rustyclaw service start` | Start the service |
| `rustyclaw service stop` | Stop the service |
| `rustyclaw service restart` | Restart the service |
| `rustyclaw service status` | Check service status |
| `rustyclaw service uninstall` | Remove from login items |

### Emergency Stop
| Command | Description |
|---------|-------------|
| `rustyclaw estop` | Engage emergency stop |
| `rustyclaw estop --level kill-all` | Kill all agent activity |
| `rustyclaw estop --level network-kill` | Block network access |
| `rustyclaw estop --level domain-block --domain "*.example.com"` | Block domains |
| `rustyclaw estop --level tool-freeze --tool shell` | Freeze specific tools |
| `rustyclaw estop status` | View estop state |
| `rustyclaw estop resume` | Resume from estop |

### Scheduling
| Command | Description |
|---------|-------------|
| `rustyclaw cron list` | List scheduled tasks |
| `rustyclaw cron add '<expr>' '<message>'` | Add cron task |
| `rustyclaw cron add-at <timestamp> '<message>'` | One-shot at specific time |
| `rustyclaw cron add-every <ms> '<message>'` | Interval task |
| `rustyclaw cron once <duration> '<message>'` | One-shot after duration |
| `rustyclaw cron pause <id>` | Pause a task |
| `rustyclaw cron update <id> --expression '<expr>'` | Update task |

### Channels
| Command | Description |
|---------|-------------|
| `rustyclaw channel list` | List configured channels |
| `rustyclaw channel add` | Add a channel interactively |
| `rustyclaw channel remove <name>` | Remove a channel |
| `rustyclaw channel doctor` | Diagnose channel issues |

### Memory
| Command | Description |
|---------|-------------|
| `rustyclaw memory list` | List stored memories |
| `rustyclaw memory get <key>` | Get a specific memory |
| `rustyclaw memory stats` | Memory statistics |
| `rustyclaw memory clear --category <cat>` | Clear memories by category |

### Models & Auth
| Command | Description |
|---------|-------------|
| `rustyclaw models list` | List available models |
| `rustyclaw auth login --provider <name>` | Manage provider auth |

### Tools (Synthesized)
| Command | Description |
|---------|-------------|
| `rustyclaw synth list` | List synthesized tools |
| `rustyclaw synth inspect <name>` | Inspect a tool |
| `rustyclaw synth approve <name>` | Approve probationary tool |
| `rustyclaw synth suspend <name>` | Suspend failing tool |
| `rustyclaw synth delete <name>` | Delete a tool |

### Skills
| Command | Description |
|---------|-------------|
| `rustyclaw skills list` | List installed skills |
| `rustyclaw skills install <name>` | Install a skill |

### Hardware
| Command | Description |
|---------|-------------|
| `rustyclaw hardware discover` | Discover USB devices |
| `rustyclaw hardware introspect <port>` | Introspect a device |
| `rustyclaw hardware info --chip <chip>` | Get chip info |
| `rustyclaw peripheral list` | List peripherals |
| `rustyclaw peripheral add <board> <path>` | Add a peripheral |
| `rustyclaw peripheral flash --port <port>` | Flash firmware |

### Configuration & Secrets
| Command | Description |
|---------|-------------|
| `rustyclaw config schema` | Print JSON Schema for config |
| `rustyclaw secrets set <key> <value>` | Store secret in keychain |
| `rustyclaw secrets get <key>` | Retrieve secret |
| `rustyclaw secrets list` | List stored secrets |
| `rustyclaw secrets delete <key>` | Delete secret |

### Other
| Command | Description |
|---------|-------------|
| `rustyclaw completions <shell>` | Generate shell completions (bash/zsh/fish/powershell/elvish) |
| `rustyclaw migrate` | Migrate from other runtimes |
| `rustyclaw integrations` | Browse 50+ integrations |

---

## Configuration (`~/.rustyclaw/config.toml`)

### Core Keys
```toml
default_provider = "anthropic"          # Provider ID or alias
default_model = "claude-sonnet-4-6"     # Model ID
default_temperature = 0.7               # 0.0–2.0
```

### Key Sections
| Section | Purpose |
|---------|---------|
| `[agent]` | Agent behavior (max iterations, history, parallel tools) |
| `[autonomy]` | Security level, workspace bounds, command allowlists |
| `[security.otp]` | OTP gating for sensitive actions |
| `[security.estop]` | Emergency stop configuration |
| `[memory]` | Backend, auto-save, embeddings, vector weights |
| `[gateway]` | HTTP server bind address, port, pairing |
| `[browser]` | Browser automation backend and settings |
| `[http_request]` | HTTP tool domain allowlist |
| `[observability]` | Telemetry backend and OTLP settings |
| `[hardware]` | Hardware transport and probe config |
| `[peripherals]` | Board configurations |
| `[channels_config.*]` | Per-channel settings (tokens, allowlists) |
| `[agents.*]` | Sub-agent definitions (provider, model, tools) |
| `[skills]` | Skill loading and open-skills config |
| `[composio]` | Composio OAuth integration |
| `[cost]` | Token/cost tracking and limits |
| `[identity]` | Identity format (openclaw or aieos) |
| `[multimodal]` | Image handling settings |
| `[runtime]` | Runtime options (reasoning toggle) |
| `[[model_routes]]` | Task-hint → provider/model routing |
| `[[embedding_routes]]` | Embedding provider routing |
| `[query_classification]` | Auto-route messages by content patterns |
| `[heartbeat]` | Heartbeat monitor config |
| `[reliability]` | Backoff and retry settings |

### Config Resolution Order
1. `RUSTYCLAW_WORKSPACE` env override
2. `~/.rustyclaw/active_workspace.toml` marker
3. Default `~/.rustyclaw/config.toml`

---

## Key Extension Points (Traits)

| Trait | Location | Purpose |
|-------|----------|---------|
| `Provider` | `src/providers/traits.rs` | LLM inference (chat, streaming, tool calling) |
| `Channel` | `src/channels/traits.rs` | Messaging platform (send, listen, health) |
| `Tool` | `src/tools/traits.rs` | Agent capability (execute, schema) |
| `Memory` | `src/memory/traits.rs` | Persistence (store, recall, forget) |
| `Observer` | `src/observability/traits.rs` | Telemetry collection |
| `RuntimeAdapter` | `src/runtime/traits.rs` | Platform abstraction |
| `Peripheral` | `src/peripherals/traits.rs` | Hardware board interface |
| `HookHandler` | `src/hooks/traits.rs` | Lifecycle event hooks |
| `Sandbox` | `src/security/traits.rs` | OS-level process isolation |

---

## Skills System

Skills are user-defined or community-built capabilities stored in `~/.rustyclaw/workspace/skills/<name>/SKILL.md`.

### Skill Structure
- `SKILL.md` — Main skill instructions (loaded into system prompt)
- Optional `SKILL.toml` — Manifest with metadata, tool definitions, prompts
- Skills can define tools (shell, HTTP, script types)

### Open Skills
- Community repository: `open-skills` (opt-in via `skills.open_skills_enabled`)
- Security audit on install: rejects symlinks, unsafe scripts, risky payloads
- Prompt injection modes: `full` (inline) or `compact` (name/description only)

---

## Workspace Layout

```
~/.rustyclaw/
├── config.toml          # Main configuration
├── workspace/
│   ├── AGENTS.md        # Agent behavior instructions
│   ├── SOUL.md          # Agent personality/identity
│   ├── USER.md          # User profile
│   ├── TOOLS.md         # Local tool notes
│   ├── IDENTITY.md      # Agent identity
│   ├── MEMORY.md        # Long-term curated memory
│   ├── HEARTBEAT.md     # Heartbeat checklist
│   ├── BOOTSTRAP.md     # First-run instructions
│   ├── sessions/        # Chat sessions
│   ├── memory/          # Daily memory files
│   ├── state/           # Runtime state
│   ├── cron/            # Cron task data
│   └── skills/          # Installed skills
├── agents/              # Agent definition files (YAML+MD)
└── estop-state.json     # Emergency stop state
```

---

## Project Status

| Component | Status |
|-----------|--------|
| Rust core (channels, providers, tools, security, memory, gateway) | **Stable** |
| Unified daemon (Rust + Elixir single-command startup) | **Stable** |
| Elixir orchestration (agent lifecycle, registry, coordination) | In progress |
| Rust-Elixir bridge (HTTP) | In progress |
| Plugin system (Elixir-side) | In progress |
| Tool synthesis (dynamic tool generation) | In progress |
