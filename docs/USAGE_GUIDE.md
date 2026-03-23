# RustyClaw Usage Guide

> Everything you need to know to use RustyClaw as a user, operator, and developer.

---

## Table of Contents

1. [Quick Start](#quick-start)
2. [Core Concepts](#core-concepts)
3. [CLI Commands](#cli-commands)
4. [Channels (Messaging)](#channels)
5. [Providers & Models](#providers--models)
6. [Tools](#tools)
7. [Memory System](#memory-system)
8. [Skills](#skills)
9. [Multi-Agent (Elixir/OTP)](#multi-agent)
10. [Daemon & Services](#daemon--services)
11. [Security](#security)
12. [Scheduling & Cron](#scheduling--cron)
13. [Hardware & Peripherals](#hardware--peripherals)
14. [Configuration Reference](#configuration-reference)
15. [Troubleshooting](#troubleshooting)

---

## Quick Start

### Install

```bash
# Clone and build
git clone https://github.com/tezra-io/rustyclaw.git
cd rustyclaw
./zeroclaw_install.sh --guided

# Or quick setup (non-interactive)
./zeroclaw_install.sh --prefer-prebuilt
rustyclaw onboard --api-key "sk-..." --provider openrouter
```

### First Chat

```bash
# One-shot message
rustyclaw agent -m "Hello, RustyClaw!"

# Interactive session
rustyclaw agent

# Use a specific provider/model
rustyclaw agent -p anthropic --model claude-sonnet-4-6
```

### Start the Daemon (Full Runtime)

```bash
# Start everything: gateway, channels, scheduler, heartbeat, Elixir
rustyclaw daemon

# Without Elixir (single-agent mode)
rustyclaw daemon --no-elixir

# Check status
rustyclaw status
```

---

## Core Concepts

### Architecture

RustyClaw is a **two-layer** system:

```
┌─────────────────────────────────────────────┐
│           Elixir/OTP Layer                  │
│  Agent lifecycle, routing, plugins,         │
│  tool synthesis, inter-agent messaging      │
└──────────────────┬──────────────────────────┘
                   │ HTTP bridge (localhost)
┌──────────────────▼──────────────────────────┐
│              Rust Core                      │
│  Channels, Providers, Tools, Security,      │
│  Memory, Gateway, Cron, Peripherals         │
└─────────────────────────────────────────────┘
```

- **Rust Core** — The fast, lean runtime. Handles all I/O, LLM calls, tool execution, and security.
- **Elixir/OTP Layer** — Optional. Adds multi-agent orchestration, dynamic tool generation, and plugin system.
- **BTW Bridge** — HTTP bridge between the two layers (ports 4001-4002).

### Workspace

Your workspace lives at `~/.rustyclaw/workspace/` by default:

```
~/.rustyclaw/
├── config.toml          # Main configuration
├── active_workspace.toml # Points to active config dir
├── workspace/
│   ├── AGENTS.md        # Agent behavior instructions
│   ├── SOUL.md          # Agent personality
│   ├── USER.md          # User profile
│   ├── TOOLS.md         # Local tool notes
│   ├── IDENTITY.md      # Agent identity
│   ├── MEMORY.md        # Long-term curated memory
│   ├── HEARTBEAT.md     # Heartbeat checklist
│   ├── BOOTSTRAP.md     # First-run (delete after setup)
│   ├── sessions/        # Chat sessions
│   ├── memory/          # Daily memory files
│   ├── state/           # Runtime state
│   ├── cron/            # Cron task data
│   └── skills/          # Installed skills
├── agents/              # Agent definition files (YAML+MD)
└── logs/                # Daemon logs
```

---

## CLI Commands

### Core

| Command | Description |
|---------|-------------|
| `rustyclaw onboard` | Quick setup (provider + key) |
| `rustyclaw onboard --interactive` | Full wizard (9 steps) |
| `rustyclaw onboard --channels-only` | Reconfigure channels only |
| `rustyclaw agent` | Interactive chat session |
| `rustyclaw agent -m "..."` | Single-shot message |
| `rustyclaw agent -p <provider>` | Use specific provider |
| `rustyclaw agent --model <model>` | Use specific model |
| `rustyclaw gateway` | HTTP/WebSocket server only |
| `rustyclaw gateway -p <port>` | Gateway on specific port |
| `rustyclaw daemon` | Full runtime (everything) |
| `rustyclaw daemon --no-elixir` | Degraded single-agent mode |
| `rustyclaw status` | System status |
| `rustyclaw doctor` | Run diagnostics |
| `rustyclaw providers` | List supported providers |

### Service Management

```bash
rustyclaw service install    # Install as launchd/systemd service
rustyclaw service start      # Start service
rustyclaw service stop       # Stop service
rustyclaw service restart    # Restart
rustyclaw service status     # Check status
rustyclaw service uninstall  # Remove service
```

### Models

```bash
rustyclaw models list                          # List cached models
rustyclaw models list --provider anthropic     # List for specific provider
rustyclaw models refresh                       # Fetch latest models from provider
rustyclaw models refresh --force               # Force refresh (ignore cache)
rustyclaw models refresh --all                 # Refresh all providers
rustyclaw models set "claude-sonnet-4-6"       # Set default model
rustyclaw models status                        # Show current model + cache info
```

### Memory

```bash
rustyclaw memory list                    # List stored memories
rustyclaw memory get <key>               # Get specific memory
rustyclaw memory stats                   # Memory statistics
rustyclaw memory clear --category <cat>  # Clear by category
```

### Scheduling

```bash
rustyclaw cron list                              # List all tasks
rustyclaw cron add '0 9 * * *' 'Good morning!'  # Cron expression
rustyclaw cron add-at 2026-03-23T09:00:00 'Msg' # One-shot at time
rustyclaw cron add-every 3600000 'Hourly check'  # Every N ms
rustyclaw cron once 20m 'Remind me'              # One-shot after duration
rustyclaw cron pause <id>                        # Pause a task
rustyclaw cron update <id> --expression '...'    # Update schedule
```

### Channels

```bash
rustyclaw channel list       # List configured channels
rustyclaw channel add        # Add channel interactively
rustyclaw channel remove X   # Remove a channel
rustyclaw channel doctor     # Diagnose channel issues
```

### Skills

```bash
rustyclaw skills list              # List installed skills
rustyclaw skills install <name>    # Install a skill
```

### Synthesized Tools (Elixir)

```bash
rustyclaw synth list           # List synthesized tools
rustyclaw synth inspect <name> # Inspect a tool
rustyclaw synth approve <name> # Promote from probation
rustyclaw synth suspend <name> # Suspend failing tool
rustyclaw synth delete <name>  # Delete a tool
```

### Hardware

```bash
rustyclaw hardware discover              # Discover USB devices
rustyclaw hardware introspect <port>     # Introspect device
rustyclaw hardware info --chip <chip>    # Get chip info
rustyclaw peripheral list                # List peripherals
rustyclaw peripheral add <board> <path>  # Add peripheral
rustyclaw peripheral flash --port <port> # Flash firmware
```

### Security

```bash
# Emergency Stop
rustyclaw estop                                        # Engage e-stop
rustyclaw estop --level kill-all                       # Kill all activity
rustyclaw estop --level network-kill                   # Block all network
rustyclaw estop --level domain-block --domain "*.bad"  # Block domains
rustyclaw estop --level tool-freeze --tool shell       # Freeze tools
rustyclaw estop status                                 # View e-stop state
rustyclaw estop resume                                 # Resume (OTP required)

# Secrets
rustyclaw secrets set <key> <value>  # Store in keychain
rustyclaw secrets get <key>          # Retrieve
rustyclaw secrets list               # List stored
rustyclaw secrets delete <key>       # Delete

# Auth
rustyclaw auth login --provider <name>  # Provider auth (OAuth/device flow)
```

### Other

```bash
rustyclaw completions zsh     # Generate shell completions
rustyclaw migrate             # Migrate from other runtimes
rustyclaw integrations        # Browse 50+ integrations
rustyclaw config schema       # Print JSON Schema for config
```

---

## Channels

RustyClaw supports **19 messaging channels**:

| Channel | Setup | Notes |
|---------|-------|-------|
| **Telegram** | Bot token from @BotFather | Most mature, supports streaming |
| **Discord** | Bot token + guild ID | Message content intent required |
| **Signal** | signal-cli daemon (HTTP) | DM or group scope |
| **Slack** | Bot + App token | Socket mode or webhook |
| **WhatsApp Cloud** | Meta Business API | Access token + phone ID |
| **WhatsApp Web** | QR/pair code | Feature flag, no Meta API needed |
| **Matrix** | Homeserver + access token | E2EE support |
| **IRC** | Server + nick + channels | TLS, SASL, NickServ auth |
| **iMessage** | macOS only | Full Disk Access required |
| **Email** | IMAP/SMTP | — |
| **Nostr** | Private key + relays | NIP-04 & NIP-17 DMs |
| **DingTalk** | Client ID + Secret | Stream mode |
| **Lark/Feishu** | App ID + Secret | WebSocket or webhook |
| **Linq** | API token | iMessage/RCS/SMS |
| **Mattermost** | Bot token + channel | — |
| **Nextcloud Talk** | App token + webhook | OCS API |
| **QQ** | App ID + Secret | Tencent QQ Bot |
| **Wati** | — | WhatsApp Business |
| **ClawdTalk** | — | Native protocol |

### Adding a Channel

```bash
# Interactive
rustyclaw onboard --channels-only

# Or edit config.toml directly:
[channels_config.telegram]
bot_token = "your-bot-token"
allowed_users = ["your-telegram-user-id"]
```

### Channel Security

Every channel has an **allowlist**. Only listed users can interact:
- Telegram: `allowed_users` (username or numeric ID)
- Discord: `allowed_users` (Discord user IDs)
- Signal: `allowed_from` (phone numbers, E.164)
- Use `"*"` for open access (not recommended in production)

---

## Providers & Models

### Supported Providers (15+)

| Tier | Providers |
|------|-----------|
| **Recommended** | OpenRouter, Venice, Anthropic, OpenAI, Gemini |
| **Fast** | Groq, Fireworks, Together AI, NVIDIA NIM |
| **Gateway** | Vercel AI, Cloudflare AI, Amazon Bedrock |
| **Specialized** | Moonshot/Kimi, GLM/Zhipu, MiniMax, Qwen, Qianfan, Z.AI, Cohere |
| **Local** | Ollama, llama.cpp, vLLM, SGLang, Osaurus |
| **Custom** | Any OpenAI-compatible API |

### Model Selection

```toml
# config.toml
default_provider = "openrouter"
default_model = "anthropic/claude-sonnet-4.6"
default_temperature = 0.7
```

### Model Routing (Auto-Select by Task)

```toml
[[model_routes]]
hint = "coding"
provider = "anthropic"
model = "claude-sonnet-4-6"

[[model_routes]]
hint = "creative"
provider = "openai"
model = "gpt-5.2"
```

### Provider Resolution Order

1. `RUSTYCLAW_PROVIDER` env var (always wins)
2. `PROVIDER` env var (legacy)
3. `default_provider` in config.toml

---

## Tools

RustyClaw has **37 built-in tools** the agent can use:

### File & Search
- `file_read` / `file_write` / `file_edit` — File operations
- `glob_search` — Find files by pattern
- `content_search` — Grep across files
- `pdf_read` — Extract PDF text
- `image_info` — Image analysis

### Shell & System
- `shell` — Execute commands (policy-gated)
- `git_operations` — Git operations
- `screenshot` — Screen capture

### Web
- `web_fetch` — Fetch web content
- `web_search` — Web search
- `http_request` — API calls (domain-allowlisted)
- `browser` / `browser_open` — Browser automation

### Memory
- `memory_store` / `memory_recall` / `memory_forget`

### Scheduling
- `cron_add` / `cron_list` / `cron_remove` / `cron_run` / `cron_update` / `schedule`

### Agent
- `delegate` — Delegate to sub-agents (requires Elixir layer)

### Skills
- `skill_create` / `skill_edit` / `skill_delete` / `skill_patch`

### Hardware (feature-gated)
- `hardware_board_info` / `hardware_memory_map` / `hardware_memory_read`

### Integrations
- `composio` — 1000+ OAuth app integrations
- `pushover` — Push notifications
- `cli_discovery` — Discover CLI tools

### Tool Security

Tools are gated by the **autonomy level**:

| Level | Shell | File Write | Approval |
|-------|-------|-----------|----------|
| `read_only` | ❌ | ❌ | N/A |
| `supervised` | ✅ gated | ✅ gated | Required for medium-risk |
| `full` | ✅ | ✅ | Skipped |

Configure allowed commands:
```toml
[autonomy]
level = "supervised"
workspace_only = true
allowed_commands = ["git", "npm", "cargo", "python3"]
```

---

## Memory System

### Backends

| Backend | Best For |
|---------|----------|
| `sqlite` | Default — fast, local, hybrid search |
| `lucid` | Enhanced — SQLite + workspace awareness |
| `markdown` | Human-readable, git-friendly |
| `qdrant` | Dedicated vector search |
| `postgres` | Scalable, remote |
| `none` | Ephemeral (no persistence) |

### Configuration

```toml
[memory]
backend = "sqlite"
auto_save = true
embedding_provider = "none"         # or "openai" for vector search
embedding_model = "text-embedding-3-small"
vector_weight = 0.7
keyword_weight = 0.3
min_relevance_score = 0.4
conversation_retention_days = 30

# Hygiene (SQLite/Lucid only)
hygiene_enabled = true
archive_after_days = 7
purge_after_days = 30
```

### How Memory Works

1. **Auto-save** — User messages are automatically stored (assistant outputs excluded)
2. **Hybrid search** — Combines vector similarity + keyword matching
3. **Categories** — Memories are classified for organized recall
4. **Session isolation** — Each session has its own context
5. **Workspace files** — `MEMORY.md` is your curated long-term memory

### Embedding Routes

```toml
[[embedding_routes]]
hint = "semantic"
provider = "openai"
model = "text-embedding-3-small"
dimensions = 1536
```

---

## Skills

Skills are modular capabilities installed in `~/.rustyclaw/workspace/skills/`.

### Structure

```
skills/
└── my-skill/
    ├── SKILL.md          # Instructions (loaded into system prompt)
    └── SKILL.toml        # Optional manifest with tool definitions
```

### Creating a Skill

```bash
# Via CLI
rustyclaw skills create my-skill

# Or manually create the directory + SKILL.md
```

### SKILL.md Format

```markdown
---
name: my-skill
description: What this skill does
allowed-tools: Bash(curl:*), Bash(jq:*)
---

# My Skill

Instructions for how to use this skill...
```

### Open Skills (Community)

```toml
[skills]
open_skills_enabled = true    # Opt-in to community skills
# open_skills_dir = "/path"   # Optional custom directory
```

### Built-in Self Skill

RustyClaw ships with a `rustyclaw-self` skill embedded in the binary. It's installed to `workspace/skills/rustyclaw-self/` during onboarding and contains full documentation of RustyClaw's architecture, commands, and capabilities.

---

## Multi-Agent

The Elixir/OTP layer enables multi-agent orchestration. **Requires Elixir >= 1.17.**

### Agent Definitions

Create agent files in `~/.rustyclaw/agents/`:

```yaml
# ~/.rustyclaw/agents/researcher.md
---
name: researcher
model: anthropic/claude-sonnet-4-6
capabilities:
  - research
  - web_search
persistent: false
---

You are a research assistant. Search the web and synthesize findings.
```

### Config-Based Agents

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

### Delegation Strategies

- **Sequential** — Try agents in order
- **Fanout** — Broadcast to all matching agents
- **First Available** — Route to first idle agent

### OTP Components

| Component | Role |
|-----------|------|
| `AgentRegistry` | Name → PID mapping |
| `AgentSupervisor` | Spawn/terminate agents |
| `AgentServer` | Per-agent GenServer |
| `AgentCoordinator` | Capability routing + delegation ACL |
| `SubAgentSession` | Session lifecycle (ETS) |
| `RustBridge` | HTTP bridge to Rust core |

### Tool Synthesis

Agents can create new tools on-the-fly:

1. Agent requests a tool that doesn't exist
2. Synthesizer generates code + schema
3. Tool enters **probation** (tracked reliability)
4. After enough successful runs → promoted to **active**
5. If it starts failing → **suspended**

API on port 4001:
```
GET  /health
GET  /api/tools
POST /api/tools/:name/approve
POST /api/tools/:name/suspend
```

### Plugin System

API on port 4002:
```
GET  /health
GET  /api/plugins
GET  /api/tasks
```

---

## Daemon & Services

### Starting the Daemon

```bash
# Full runtime
rustyclaw daemon

# Specific host/port
rustyclaw daemon --host 0.0.0.0 --port 8080

# Without Elixir
rustyclaw daemon --no-elixir
```

### What the Daemon Runs

| Component | Purpose | Auto-restart |
|-----------|---------|:---:|
| Gateway | HTTP/WS server | ✅ |
| Channels | All messaging platforms | ✅ |
| Heartbeat | Periodic checks | ✅ |
| Scheduler | Cron tasks | ✅ |
| Elixir OTP | Multi-agent orchestrator | ✅ |
| State Writer | daemon_state.json (5s) | ✅ |

All components have exponential backoff on failure. If Elixir crashes, Rust core continues in degraded mode.

### Install as System Service

```bash
# Install (creates launchd plist on macOS, systemd unit on Linux)
rustyclaw service install

# Manage
rustyclaw service start
rustyclaw service stop
rustyclaw service restart
rustyclaw service status
```

### Health Monitoring

```bash
# Gateway health endpoint
curl http://127.0.0.1:42617/health

# Prometheus metrics
curl http://127.0.0.1:42617/metrics

# Daemon state file
cat ~/.rustyclaw/daemon_state.json
```

### Gateway API

```
POST /pair          # Pair new client (X-Pairing-Code header)
POST /webhook       # Send message: {"message": "your prompt"}
GET  /api/*         # REST API (bearer token required)
GET  /ws/chat       # WebSocket agent chat
GET  /health        # Health check
GET  /metrics       # Prometheus metrics
GET  /              # Web dashboard
```

---

## Security

### Defense Layers

1. **Autonomy Levels** — `read_only` / `supervised` / `full`
2. **Workspace Boundaries** — Restrict file access to workspace
3. **Command Allowlists** — Explicit shell command allow list
4. **Forbidden Paths** — System paths blocked by default
5. **Sandboxing** — Docker, Firejail, Bubblewrap, Landlock
6. **Policy Engine** — Deny-by-default for channels, domains, tools
7. **Prompt Guard** — Injection detection
8. **Content Scanner** — Threat classification
9. **Leak Detector** — Credential/secret leak detection
10. **Sentinel Engine** — Sanitization and redaction
11. **Approval System** — Human-in-the-loop gating

### OTP (One-Time Password) Gating

```toml
[security.otp]
enabled = true
gated_actions = ["shell", "file_write", "browser", "memory_forget"]
gated_domains = ["banking", "medical", "government"]
```

### Emergency Stop

Multi-level e-stop:
- `kill-all` — Stop all agent activity
- `network-kill` — Block all network
- `domain-block` — Block specific domains
- `tool-freeze` — Freeze specific tools

State persists across restarts. OTP required to resume.

### Pairing

Gateway requires pairing by default:
```toml
[gateway]
require_pairing = true
```

First connection gets a one-time pairing code displayed in the daemon output.

---

## Scheduling & Cron

### Config

```toml
[cron]
enabled = true

[heartbeat]
enabled = true
interval_minutes = 30
target = "telegram"        # Delivery channel
to = "your-telegram-id"   # Delivery target
```

### Heartbeat

The heartbeat system periodically runs tasks defined in `HEARTBEAT.md`:

```markdown
# HEARTBEAT.md
- Check email for urgent messages
- Review calendar for upcoming events
- Check weather if outdoor activity planned
```

### Cron Tasks

```bash
# Daily at 9 AM
rustyclaw cron add '0 9 * * *' 'Morning briefing'

# Every hour
rustyclaw cron add-every 3600000 'Hourly status check'

# One-shot in 20 minutes
rustyclaw cron once 20m 'Reminder: call dentist'

# One-shot at specific time
rustyclaw cron add-at 2026-03-23T14:00:00 'Meeting prep'
```

---

## Hardware & Peripherals

### Supported Boards

| Board | Transport | Use Case |
|-------|-----------|----------|
| Raspberry Pi GPIO | Native | Direct GPIO access |
| STM32 Nucleo | Serial + probe-rs | Microcontroller dev |
| Arduino | Serial + upload | Prototyping |
| ESP32 | Serial | IoT |

### Configuration

```toml
[hardware]
enabled = true

[peripherals]
enabled = true
datasheet_dir = "docs/datasheets"

[[peripherals.boards]]
board = "nucleo-f401re"
transport = "serial"
path = "/dev/ttyACM0"
baud = 115200
```

### Features
- USB device discovery
- Chip identification (probe-rs / ST-Link)
- Firmware flashing
- **Datasheet RAG** — Index PDF schematics for AI pin lookups
- Boards become agent tools when enabled

### CLI

```bash
rustyclaw hardware discover
rustyclaw peripheral list
rustyclaw peripheral add nucleo-f401re /dev/ttyACM0
rustyclaw peripheral flash --port /dev/ttyACM0
```

---

## Configuration Reference

### File: `~/.rustyclaw/config.toml`

```toml
# Core
default_provider = "openrouter"
default_model = "anthropic/claude-sonnet-4.6"
default_temperature = 0.7

# Agent behavior
[agent]
max_iterations = 25          # Max tool-call loops
max_history_messages = 50    # Conversation history cap
parallel_tool_calls = true   # Allow parallel tool execution

# Security
[autonomy]
level = "supervised"         # read_only | supervised | full
workspace_only = true        # Restrict to workspace
allowed_commands = ["git", "cargo", "npm"]
max_actions_per_hour = 20
max_cost_per_day_cents = 500

# Gateway
[gateway]
port = 8080
host = "127.0.0.1"
require_pairing = true

# Memory
[memory]
backend = "sqlite"
auto_save = true

# Channels
[channels_config]
cli = true
message_timeout_secs = 300

[channels_config.telegram]
bot_token = "..."
allowed_users = ["..."]

# Heartbeat
[heartbeat]
enabled = true
interval_minutes = 30

# Cron
[cron]
enabled = true

# Observability
[observability]
backend = "log"              # noop | log | prometheus | otel | verbose | multi

# Cost tracking
[cost]
daily_limit_cents = 500
warning_threshold_percent = 80

# Browser
[browser]
backend = "playwright"       # or "chromium", "selenium"

# HTTP tool
[http_request]
allowed_domains = ["api.github.com", "*.example.com"]

# Tunnel (expose to internet)
[tunnel]
provider = "none"            # cloudflare | tailscale | ngrok | custom

# Composio (managed OAuth)
[composio]
enabled = false

# Identity format
[identity]
format = "openclaw"          # or "aieos"

# Runtime
[runtime]
reasoning = false            # Enable reasoning/thinking mode
```

### Config Resolution Order

1. `RUSTYCLAW_WORKSPACE` env var
2. `~/.rustyclaw/active_workspace.toml` marker
3. `~/.rustyclaw/config.toml` (default)

### Environment Variables

| Variable | Purpose |
|----------|---------|
| `RUSTYCLAW_PROVIDER` | Override provider |
| `RUSTYCLAW_WORKSPACE` | Override workspace path |
| `RUSTYCLAW_CONFIG_DIR` | Override config directory |
| `OPENROUTER_API_KEY` | OpenRouter key |
| `ANTHROPIC_API_KEY` | Anthropic key |
| `OPENAI_API_KEY` | OpenAI key |
| `GEMINI_API_KEY` | Google Gemini key |
| `OLLAMA_API_KEY` | Ollama key (remote only) |

---

## Troubleshooting

### Channels Show Inactive

1. Check `active_workspace.toml` isn't pointing to a stale dir:
   ```bash
   cat ~/.rustyclaw/active_workspace.toml
   ```
   Fix: `echo 'config_dir = "/Users/you/.rustyclaw"' > ~/.rustyclaw/active_workspace.toml`

2. Check your config actually has channels:
   ```bash
   grep -A3 "channels_config" ~/.rustyclaw/config.toml
   ```

3. Restart: `rustyclaw service restart` or `launchctl stop/start com.rustyclaw.daemon`

### Skills Not Found

Skills load from `~/.rustyclaw/workspace/skills/`. Check:
```bash
ls ~/.rustyclaw/workspace/skills/
```
If empty, copy skills from the source repo or re-run onboarding.

### Elixir Health Check Failed

Non-critical — Rust core continues in degraded mode. To fix:
```bash
# Check Elixir is installed
elixir --version  # Need >= 1.17

# Compile orchestrator
cd /path/to/rustyclaw/elixir/rustyclaw_orchestrator
mix deps.get && mix compile
```

### LLM Errors

```bash
# Check provider is reachable
rustyclaw doctor

# Check API key
rustyclaw models refresh --force

# Try a different provider
rustyclaw agent -p openrouter -m "test"
```

### Gateway Not Responding

```bash
# Find the actual port
grep "port" ~/.rustyclaw/config.toml

# Test health
curl http://127.0.0.1:<port>/health

# Check logs
tail -50 ~/.rustyclaw/logs/daemon.stderr.log
tail -50 ~/.rustyclaw/logs/daemon.stdout.log
```

### Reset Everything

```bash
# Nuclear option — re-onboard from scratch
rustyclaw onboard --interactive --force
```

---

## Extension Points (For Developers)

### Rust Traits

| Trait | File | Purpose |
|-------|------|---------|
| `Provider` | `src/providers/traits.rs` | LLM backend |
| `Channel` | `src/channels/traits.rs` | Messaging platform |
| `Tool` | `src/tools/traits.rs` | Agent capability |
| `Memory` | `src/memory/traits.rs` | Storage backend |
| `Observer` | `src/observability/traits.rs` | Telemetry |
| `Peripheral` | `src/peripherals/traits.rs` | Hardware board |
| `HookHandler` | `src/hooks/traits.rs` | Lifecycle hooks |
| `Sandbox` | `src/security/traits.rs` | Process isolation |

### Adding a New Provider

1. Implement the `Provider` trait
2. Register in `src/providers/mod.rs`
3. Add to curated models in `src/onboard/wizard.rs`
4. Add env var mapping in `provider_env_var()`

### Adding a New Channel

1. Implement the `Channel` trait
2. Add config struct to `src/config/schema.rs`
3. Register in `src/channels/mod.rs`
4. Add to onboarding wizard

---

*Last updated: 2026-03-22*
*Generated from RustyClaw source code analysis*
