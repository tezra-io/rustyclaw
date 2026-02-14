# CLAUDE.md — RustyClaw

## Project
RustyClaw: ultra-lightweight personal AI assistant with multi-agent support. Single Rust binary, markdown-defined agents, tamper-proof memory.

## Stack
- Rust 2021 edition (1.75+)
- Async runtime: **tokio** (full features)
- HTTP: **reqwest** (rustls-tls)
- Serialization: **serde** + **serde_json** + **serde_yaml**
- CLI: **clap** (derive)
- Logging: **tracing** + **tracing-subscriber** + **tracing-appender**
- Error handling: **thiserror** + **anyhow**
- WebSocket: **tokio-tungstenite**
- Crypto: **sha2** (memory integrity)
- Telegram: **teloxide**
- Cron: **cron** crate

## Architecture
- `src/agent/` — core agent loop, context builder, memory, ledger, multi-agent (cluster, supervisor, router, definition parser)
- `src/bus/` — async message bus (inbound/outbound/inter-agent channels)
- `src/channels/` — Telegram, Discord channel implementations
- `src/providers/` — LLM providers (OpenAI-compatible, OpenRouter, Anthropic, etc.)
- `src/tools/` — tool system (filesystem, shell, web, message, cron, spawn)
- `src/cli/` — clap-based CLI with agent management commands
- `src/config/` — JSON config loading with env var overrides
- `src/cron/` — internal job scheduler
- `src/session/` — conversation session management

## Code Style
- `cargo fmt` — always
- `cargo clippy -- -D warnings` — zero warnings policy
- Error types via `thiserror` in `src/error.rs`
- Async everywhere for I/O
- `Arc<dyn Trait>` for shared tool/provider instances
- Inline `#[cfg(test)] mod tests` for unit tests
- Document all public types and methods

## Quality Gates
- `cargo fmt` before every commit
- `cargo clippy -- -D warnings` — must pass
- `cargo test` — must pass
- `cargo build --release` — must succeed
- Pre-commit hooks: fmt + clippy + test

## Git
- Branch: `main`
- Commit style: `feat:`, `fix:`, `refactor:`, `test:`, `docs:`
- Push to `origin` (github.com/tezra-io/rustyclaw, private)

## Key Design Docs
Read these in `../projects/rustyclaw/` for context:
- `MULTI_AGENT_DESIGN.md` — multi-agent architecture
- `DESIGN_REVIEW.md` — 4-reviewer design feedback
- `PHASE1_REVIEW.md` — 5-reviewer code inspection
- `ERROR_SCENARIOS.md` — failure modes and handling
- `POSITIONING.md` — product positioning
- `PHASE_1_TASKS.md` — Phase 1 task breakdown

## Memory System
- Tamper-proof append-only JSONL ledger with SHA-256 hash chain
- Auto-segmented compaction at 10k entries per file
- Per-agent isolated memory (workers can't read each other's ledger)
- Master can read all agent ledgers
- `AsyncMemoryLedger` wrapper for tokio-safe file I/O

## Multi-Agent
- Master-worker hierarchy via tokio mpsc channels
- Agent definitions: markdown files with YAML frontmatter in `~/.rustyclaw/agents/`
- Supervisor with lifecycle states, crash recovery, exponential backoff
- Delegation router: explicit naming > keyword matching > master handles
- Tool scoping per agent via `ToolRegistry::scoped()`

## Config
- Format: JSON (`~/.rustyclaw/config.json`)
- Env var overrides for all API keys and core settings
- One API key per provider, shared across all agents
