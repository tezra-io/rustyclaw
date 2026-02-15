# 🦀 RustyClaw

AI agents defined in markdown. Running from a single binary.

A personal AI assistant you actually own — local-first, privacy-preserving, multi-agent. No Python runtime, no cloud dependency, no data leaving your machine unless you say so.

## Quick Start

```bash
# Build
cargo build --release

# Interactive onboarding
rustyclaw onboard

# Chat
rustyclaw agent -m "Hello!"

# Interactive mode
rustyclaw agent

# Run with channels (Telegram, Discord)
rustyclaw gateway
```

## What It Does

- **Multi-agent orchestration** — define agents in markdown with YAML frontmatter, master-worker delegation, crash recovery
- **Tamper-proof memory** — append-only JSONL ledger with SHA-256 hash chain, per-agent isolation
- **Semantic search** — local embeddings via all-MiniLM-L6-v2 (ONNX), OpenAI as optional fallback
- **Multi-provider LLM** — OpenRouter, vLLM, Anthropic, any OpenAI-compatible API
- **Tool system** — file ops, shell exec, web search/fetch, cron, sub-agent spawning
- **Channels** — CLI, Telegram, Discord (Slack, WhatsApp planned)
- **Cron scheduling** — persistent scheduled tasks with agent execution
- **Single binary** — ~5MB stripped, no runtime dependencies

## Agent Definitions

Agents are markdown files in `~/.rustyclaw/agents/`:

```markdown
---
name: researcher
model: anthropic/claude-sonnet-4-20250514
tools: [web_search, web_fetch, filesystem]
delegates_to: [writer]
---

# Researcher

You are a research agent. Find information and delegate writing tasks.
```

## Configuration

Config lives at `~/.rustyclaw/config.json`:

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
  },
  "embeddings": {
    "provider": "local"
  }
}
```

## Architecture

```
src/
├── agent/       # Core loop, context, memory, ledger, multi-agent (cluster, supervisor, router)
├── bus/         # Async message bus (inbound/outbound/inter-agent via tokio mpsc)
├── channels/    # Telegram, Discord
├── cli/         # Clap CLI + onboarding wizard + agent management
├── config/      # JSON config with env var overrides
├── cron/        # Job scheduler
├── embeddings/  # Local MiniLM (ONNX) + OpenAI provider, registry with fallback
├── providers/   # LLM providers (OpenAI-compatible, OpenRouter, Anthropic)
├── session/     # Conversation management
└── tools/       # Filesystem, shell, web, message, cron, spawn
```

## Memory System

- Append-only JSONL with SHA-256 hash chain — no updates, no deletes, tamper-evident
- Auto-segmented compaction at 10k entries per file
- Per-agent isolated ledgers (workers can't read each other's memory)
- Semantic search via local embeddings (384-dim, no API key required)
- Facts versioned with tombstone redaction (old versions preserved)

## Building

```bash
# Requires: Rust 1.75+, ONNX Runtime (for embeddings)
# macOS: brew install onnxruntime
export ORT_DYLIB_PATH=/opt/homebrew/opt/onnxruntime/lib/libonnxruntime.dylib

cargo build --release
```

## License

MIT
