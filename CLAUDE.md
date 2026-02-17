# CLAUDE.md — RustyClaw Project Context

## What is this?
RustyClaw is a personal AI assistant anyone can own. Fork of [rustyclaw](https://github.com/theonlyhennygod/rustyclaw) with multi-agent orchestration on top.

**Tagline:** "AI agents defined in markdown. Running from a single binary."

## Architecture
- **Language:** Rust
- **Channels:** Telegram, Discord, Slack, WhatsApp, IRC, Signal, iMessage
- **Providers:** OpenAI, Anthropic, OpenRouter, Ollama, Google Gemini, GitHub Copilot
- **Core features (upstream):** Security sandbox, memory system, daemon mode, SSH tunnels, onboarding wizard, tools (shell, file read/write, web fetch, etc.)
- **Our additions (in progress):** Multi-agent orchestration — agent definitions, registry, message bus, tool calling loop, AI-powered agent creation, skills system

## Building
```bash
cargo build --release
cargo test
cargo clippy
```

## Config
Config lives at `~/.rustyclaw/config.toml` (was `~/.rustyclaw/`).

## Key Design Docs
- `docs/MULTI_AGENT_V2_DESIGN.md` — comprehensive multi-agent design (2,400+ lines)
- `docs/DESIGN_REVIEW.md` — review findings from design phase

## Code Layout
- `src/agent/` — agent loop, tool execution
- `src/providers/` — LLM provider implementations
- `src/tools/` — built-in tools (shell, file ops, web, etc.)
- `src/channels/` — messaging channel integrations
- `src/security/` — sandboxing, path validation
- `src/memory/` — memory/context persistence
- `src/config/` — configuration schema and loading
- `src/skills/` — skill discovery and management
- `src/daemon/` — background daemon mode
- `src/gateway/` — webhook gateway server

## Conventions
- All code must pass `cargo clippy` with no warnings
- Tests required for new features
- Git author: `aira-bot <aira@teznewz.com>`
- Pre-commit hooks enforced

## Rename Context
This repo was freshly cloned from upstream rustyclaw. The rename from rustyclaw → rustyclaw needs to touch:
- `Cargo.toml` (package name, binary name)
- Config paths (`~/.rustyclaw/` → `~/.rustyclaw/`)
- Binary references in code and docs
- README branding
- Any hardcoded "rustyclaw" strings in source
