# nanobot (Rust)

A lightweight personal AI assistant framework. Rust port of [nanobot](https://github.com/HKUDS/nanobot).

## Features

- Multi-channel support: Telegram, Discord, Slack, WhatsApp, Email, and more
- Multi-provider LLM: OpenRouter, Anthropic, OpenAI, DeepSeek, vLLM, etc.
- Tool system: file I/O, shell exec, web search/fetch, scheduling
- Memory: persistent long-term + daily notes
- Skills: markdown-based capability extensions
- Cron: scheduled agent tasks
- Subagents: background task execution

## Quick Start

```bash
# Build
cargo build --release

# Initialize config
./target/release/nanobot onboard

# Chat (single message)
nanobot agent -m "Hello!"

# Chat (interactive)
nanobot agent

# Start gateway (Telegram, Discord, etc.)
nanobot gateway
```

## Configuration

Config lives at `~/.nanobot/config.json`. Compatible with the Python version.

```json
{
  "providers": {
    "openrouter": {
      "apiKey": "sk-or-..."
    }
  },
  "agents": {
    "defaults": {
      "model": "anthropic/claude-sonnet-4-5"
    }
  },
  "channels": {
    "telegram": {
      "enabled": true,
      "token": "BOT_TOKEN"
    }
  }
}
```

## Architecture

```
nanobot gateway
    ├── MessageBus (tokio mpsc)
    │   ├── inbound:  Channel → Agent
    │   └── outbound: Agent → Channel
    ├── AgentLoop
    │   ├── ContextBuilder (system prompt + history + memory + skills)
    │   ├── LLM Provider (OpenAI-compatible HTTP)
    │   ├── ToolRegistry (filesystem, shell, web, message, spawn, cron)
    │   └── SessionManager (JSONL persistence)
    ├── ChannelManager
    │   ├── TelegramChannel (teloxide long polling)
    │   ├── DiscordChannel (Gateway WebSocket)
    │   └── ...more channels
    ├── CronService (scheduled jobs)
    └── HeartbeatService (periodic wake)
```

## Development

```bash
cargo fmt --check
cargo clippy -- -D warnings
cargo test
```
