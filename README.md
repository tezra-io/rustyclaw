# 🦀 RustyClaw

An ultra-lightweight personal AI assistant — [nanobot](https://github.com/HKUDS/nanobot) rewritten in Rust.

⚡ Fast, secure, single binary. No Python runtime needed.

## Quick Start

```bash
# Build
cargo build --release

# Initialize config
./target/release/rustyclaw onboard

# Chat (single message)
rustyclaw agent -m "Hello!"

# Chat (interactive)
rustyclaw agent

# Run with channels (Telegram, Discord)
rustyclaw gateway
```

## Configuration

Config lives at `~/.rustyclaw/config.json`.

```json
{
  "providers": {
    "openrouter": {
      "apiKey": "sk-or-v1-xxx"
    }
  },
  "agents": {
    "defaults": {
      "model": "anthropic/claude-sonnet-4-20250514"
    }
  }
}
```

## Channels

| Channel | Status |
|---------|--------|
| CLI | ✅ Interactive + single message |
| Telegram | ✅ Long polling |
| Discord | ✅ Gateway WebSocket |
| Slack | 🔜 Planned |
| WhatsApp | 🔜 Planned |

## Features

- 🔧 **Tool use** — file ops, shell exec, web search/fetch, cron, sub-agents
- 💬 **Multi-provider** — OpenRouter, vLLM, any OpenAI-compatible API
- 📅 **Cron scheduling** — persistent scheduled tasks
- 💾 **Memory** — file-based persistent memory
- 🔒 **Security** — sandboxed shell, secret zeroization, input validation

## Building

```bash
cargo build --release
```

Single binary, no runtime dependencies. ~5MB stripped.

## License

MIT
