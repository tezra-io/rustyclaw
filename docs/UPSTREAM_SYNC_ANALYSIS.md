# Upstream Sync Analysis — zeroclaw → RustyClaw

**Fork point:** `8cf6c89`
**Upstream tip (FETCH_HEAD):** zeroclaw main
**Commits analyzed:** 259 filtered important commits (from 656 total)
**Date:** 2026-02-20

---

## Summary Counts

| Priority | Count |
|----------|-------|
| CRITICAL | 11 |
| HIGH VALUE | 24 |
| NICE TO HAVE | 14 |
| SKIP | ~210 (CI, docs, branding, org-specific) |

---

## CRITICAL — Security Fixes & Bugs We'd Hit

### Security Policy Hardening

| Commit | Description | Conflict Risk |
|--------|-------------|---------------|
| `290d971` | `fix(security)`: reject shell-unsafe chars in screenshot filename | Low — isolated to browser tool |
| `bbe5530` | `fix(security)`: disable automatic redirects in http_request tool | Low — independent |
| `a2986db` | `fix(security)`: enhance shell redirection blocking (`>`, `>>`) | **Medium** — we extended security policy for multi-agent; check `is_command_allowed` merge |
| `87dcd7a` | `fix(security)`: expand git argument sanitization (`-exec`, `git config` blocking) | **Medium** — same method, additive changes |
| `9ec1106` | `security`: fix argument injection in shell command validation (#465) | **Medium** — touches same `is_command_allowed` + `is_args_safe` |
| `ac33121` | `fix(security)`: add config file permission hardening | Low — file permissions on config write |
| `d7c1fd7` | `security(deps)`: remove vulnerable `xmas-elf` dependency via embuild | Low — just dep removal |

**What changed (confirmed from diff):**
- Added `<(` and `>(` process substitution blocking
- Added `tee` blocking (can write arbitrary files)
- Added `find -exec`/`-ok` blocking, `git config`/`-c` blocking
- Added `ToolOperation` enum + `enforce_tool_operation()` for cleaner per-tool policy
- Config file chmod hardening on write

**Our conflict:** Our multi-agent additions likely touched neither `is_args_safe` (new) nor `enforce_tool_operation` (new). Standard merge — verify no conflicts in `security/policy.rs`.

---

### Memory & Session Isolation

| Commit | Description | Conflict Risk |
|--------|-------------|---------------|
| `ebb78af` | `feat(memory)`: add `session_id` isolation to Memory trait (#530) | **HIGH** — changes `Memory` trait signature: `store()`, `recall()`, `list()` now take `Option<&str>` session_id |
| `bc18b8d` | `fix(memory)`: harden lucid recall timeout (120ms → 500ms for cold start) | **HIGH** — MAPLE touches memory; our composite memory must propagate `session_id` |

**What changed (confirmed from diff):**
- `Memory::store(key, content, category, session_id)` — added 4th param
- `Memory::recall(query, limit, session_id)` — added 3rd param
- `Memory::list(category, session_id)` — added 2nd param
- Applied to: `SqliteMemory`, `LucidMemory`, `MarkdownMemory`, `CompositeMemory`
- `LucidMemory` recall timeout: 120ms → 500ms (fixes premature fallback on cold start)

**Our conflict:** **MAPLE's `CompositeMemory` and ephemeral memory will need to implement the new signatures.** This is a required port — if we call upstream memory APIs, we'll have compile errors. Git status shows `src/memory/composite.rs` and `src/memory/ephemeral.rs` are already modified by us — manual merge needed.

---

### Agent Loop Bugs

| Commit | Description | Conflict Risk |
|--------|-------------|---------------|
| `f75f73a` | `fix(agent)`: preserve native tool-call fallbacks and history fidelity | **HIGH** — touches `run_tool_call_loop` which MAPLE probably extended |
| `0e5a785` | `fix(agent)`: use native format for tool result history in run_tool_call_loop | **HIGH** — same |
| `63602a2` | `fix(agent)`: use config-driven limits in `run_tool_call_loop` and `trim_history` | **Medium** — `max_tool_iterations` from config, not hardcoded |
| `3467d34` | `fix(agent)`: avoid duplicate text in markdown tool_call fallback | Medium |
| `508fb53` | `fix(provider)`: delegate native tool calling through `ReliableProvider` | Medium — ReliableProvider wraps our providers |

**What changed (confirmed from diff):**
- `run_tool_call_loop` now takes `max_tool_iterations: usize` param (from config)
- Native tool calls properly reconstructed with `tool_calls` array in history
- `build_native_assistant_history()` added for proper OpenAI-format tool_calls
- `scrub_credentials()` applied to tool output (redacts tokens/keys from results)
- GLM-style tool call parsing added: `toolname{...}` format inside `<tool_call>` tags

**Our conflict:** Our agent runner at `src/agent/registry.rs` and `src/tools/agent_manage.rs` likely call into `run_tool_call_loop` — verify param count.

---

### Cron Reliability

| Commit | Description | Conflict Risk |
|--------|-------------|---------------|
| `7de052c` | `fix(cron)`: add timeout and bounded execution for due jobs | **Medium** — scheduler.rs modified by us |
| `5f5cb27` | `fix(cron)`: handle `ALTER TABLE` race condition in schema migration | Low — DB migration |
| `21c5f58` | `perf(cron)`: wrap `record_run` INSERT+DELETE in explicit transaction | Low |
| `9967eeb` | `perf(cron)`: add composite index on `cron_runs(job_id, started_at)` | Low |

---

## HIGH VALUE — Features Worth Porting

### New Providers

| Commit | Description | Conflict Risk |
|--------|-------------|---------------|
| `9d0e299` | `feat`: dedicated GLM provider with JWT auth and GLM-4.7 support | Low — new file |
| `6062888` | `feat(providers)`: OVHcloud AI Endpoints as native provider | Low — new file |
| `8ad5b61` | `feat`: Astrai as a named provider (#486) | Low — new file |
| `77640e2` | `feat(provider)`: LM Studio provider alias | Low — additive |
| `007368d` | `feat(auth)`: subscription auth profiles and codex/claude flows | Medium — touches auth module |
| `39087a4` | `fix`: OpenAI Codex contract, SSE parsing, and default xhigh reasoning | Medium |

---

### Anthropic Prompt Caching

| Commit | Description | Conflict Risk |
|--------|-------------|---------------|
| `455eb3b` | `feat`: add prompt caching support to Anthropic provider | **Low** — purely in `anthropic.rs` |

**What changed (confirmed from diff):**
- System prompts >3KB auto-converted to blocks with `cache_control: ephemeral`
- Last tool definition in tool list gets cache_control
- Long conversations (>4 turns) get last-message caching
- `warmup()` implemented for Anthropic
- OAuth: setup tokens now get `anthropic-beta: oauth-2025-04-20` header

**Port priority:** High — reduces cost on long agentic conversations with MAPLE.

---

### Streaming Support

| Commit | Description | Conflict Risk |
|--------|-------------|---------------|
| `d94e78c` | `feat(streaming)`: add streaming support for LLM responses | **Medium** — adds `stream_chat` to Provider trait |
| `93538a7` | `fix(agent)`: relay final response as progressive chunks via `on_delta` | Medium |
| `118cd53` | `feat(channel)`: stream LLM responses to Telegram via draft message edits | Medium — channel trait |

**Note:** Our memory (#1113) confirmed we lack streaming. This is a large change — entire provider trait extension.

---

### New Tools

| Commit | Description | Conflict Risk |
|--------|-------------|---------------|
| `1757add` | `feat(tools)`: add `web_search_tool` for internet search | Low — new tool file |
| `e871c95` | `feat(tools)`: add JSON Schema cleaner for LLM compatibility | Low — new tool |
| `8279073` | `feat(tools)`: add native Pushover notification tool | Low — new file |
| `c04f285` | `feat(tools)`: expose custom memory categories in `memory_store` tool | **Medium** — touches memory tool, MAPLE added custom categories too |
| `6e53341` | `feat(agent)`: rule-based query classification for automatic model routing | Medium — new agent module |
| `df31359` | `feat(agent)`: scrub credentials from tool output (#532) | **Medium** — touches `run_tool_call_loop` output path |

---

### Channel Improvements

| Commit | Description | Conflict Risk |
|--------|-------------|---------------|
| `7e3f5ff` | `feat(channels)`: add Mattermost integration | Low — new channel |
| `55b3c2c` | `feat(channel)`: add Signal channel via signal-cli JSON-RPC daemon | Low — new channel |
| `ed71bce` | `feat(channels)`: add QQ Official channel via Tencent Bot SDK | Low — new channel |
| `b3b1679` | `feat(channels)`: Telegram typing indicator | Low |
| `c17a0ad` | `feat(channel)`: mention_only option for Telegram groups | Low |
| `5b5d9fe` | `feat(discord)`: add mention_only config for @-mention trigger | Low |
| `36062fb` | `feat(telegram)`: add forum topic support | Low |
| `dbebd48` | `refactor(channel)`: accept `SendMessage` struct in `Channel::send()` | **HIGH** — breaking API change: `send(&str, &str)` → `send(&SendMessage)` — all our channels need updating |
| `58958d9` | `fix`: add per-sender conversation history for channel messages | **Medium** — touches message handling |

**Critical note on `SendMessage` refactor:**
`Channel::send()` signature changed from `send(message: &str, recipient: &str)` to `send(message: &SendMessage)` where `SendMessage { content, recipient, subject }`. Also `ChannelMessage` gains `reply_target` field. Every channel (`cli`, `discord`, `dingtalk`, `telegram`, etc.) must be updated. Our custom channels (if any) need this too.

---

### Memory Improvements

| Commit | Description | Conflict Risk |
|--------|-------------|---------------|
| `483accc` | `feat(memory)`: configurable Postgres storage backend | Low — new backend, additive |
| `dd49417` | `perf(memory)`: wrap blocking SQLite calls in `spawn_blocking` | **Medium** — touches SqliteMemory internals, MAPLE uses SQLite |
| `dd45417` | `perf(memory)`: fold recall/vector/list optimizations into `spawn_blocking` refactor | Medium |
| `73e675d` | `feat(memory)`: optional SQLite connection open timeout | Low |
| `decea53` | `refactor(memory)`: keep default hybrid weights while adding relevance threshold | **Medium** — MAPLE modified recall weights |
| `8a1e7cc` | `fix(agent)`: use config `max_tool_iterations`, add memory relevance filtering, rebalance search weights | **HIGH** — conflicts likely with MAPLE's recall tuning |

---

### Observability

| Commit | Description | Conflict Risk |
|--------|-------------|---------------|
| `eba544d` | `feat(observability)`: Prometheus metrics backend with `/metrics` endpoint | Low — new backend |
| `8371f41` | `feat(observability)`: propagate optional `cost_usd` on agent end | Low |

---

### Cron Enhancements

| Commit | Description | Conflict Risk |
|--------|-------------|---------------|
| `fb2d1ce` | `feat`: cron job management tools and types (JobType enum, CronJobPatch, Schedule enum) | **HIGH** — massive cron refactor: `cron/mod.rs` split into `schedule`, `store`, `types` submodules. `CronJob` gains `prompt`, `schedule: Schedule` enum replacing `expression: String`. `scheduler.rs` modified by us |
| `3b01335` | `feat(providers)`: native tool calling for OpenAI-compatible providers | Medium |
| `d5a681e` | `chore(deps)`: bump cron from 0.12.1 to 0.15.0 | Medium — API changes |

**Note on cron refactor:** The upstream cron is a major structural change — `CronJob.expression: String` replaced with `CronJob.schedule: Schedule` enum (`Cron { expr, tz }`, `At { at }`, `Every { every_ms }`). New `JobType` enum exists now. Our `scheduler.rs` is already modified — this will be a significant merge.

---

### Auth & Onboarding

| Commit | Description | Conflict Risk |
|--------|-------------|---------------|
| `041ead6` | `feat(onboard)`: Anthropic OAuth setup-token support | Low — onboard wizard |
| `cba7d1a` | `fix(onboard)`: persist custom workspace selection across sessions | Low |
| `3c62b59` | `fix(copilot)`: add proper OAuth device-flow authentication | Low |

---

## NICE TO HAVE — Consider Later

| Commit | Description | Notes |
|--------|-------------|-------|
| `d756293` | `feat`: add `/clear` command | Simple CLI QoL |
| `b9e2dae` | `feat(doctor)`: enhanced diagnostics and config validation | Good for debugging |
| `1336c2f` | `feat(providers)`: `warmup()` for OpenAI, Anthropic, Gemini, Compatible, GLM | Perf on first call |
| `4c6f3c3` | `feat(repo)`: `.gitattributes` for cross-platform line endings | Low effort |
| `b1c04d8` | `feat(tooling)`: `.editorconfig`, `rustfmt.toml`, `clippy.toml` | Style consistency |
| `fbc26be` | `fix(policy)`: treat `git branch` listing as read-only | Correct categorization |
| `4f9c87f` | `fix(policy)`: standardize side-effect tool autonomy gates | Uses new `enforce_tool_operation` |
| `feaa4ab` | `feat(cli)`: `zeroclaw providers` command to list providers | Renamed to rustyclaw |
| `8724884` | `feat(ci)`: Criterion performance benchmarks | Upstream CI specific |
| `d148c72` | `fix(lark)`: align region endpoints and doctor config parity | Low impact |
| `0166f2d` | `fix(token)`: update token gen to use `rand::rng()` (deprecation) | Trivial fix |
| `02711b3` | `fix(git-ops)`: avoid panic truncating unicode commit messages | Already fixed in our util.rs |
| `3d3d471` | `fix(email)`: use proper MIME encoding for UTF-8 responses | Minor |
| `efa6e5a` | `feat(channel)`: add capabilities to system prompt | Hardware info, minor |

---

## SKIP — Not Relevant

- All CI/CD workflow changes (`ci:`, workflow YAML files) — zeroclaw-specific
- `feat(license)`: Apache 2.0 migration — we may have our own license
- `feat(python)`: zeroclaw-tools LangGraph companion — Python side project
- `feat`: zeroclaw-robot-kit crate — separate robotics crate
- Hardware/ESP32/Pico/GPIO/firmware features — tracked separately in our hardware docs
- `docs(readme)`, `docs(custom-providers)`, `docs(agent-guides)` — zeroclaw branding
- `chore(codeowners)`, `chore(pr)`, issue template changes — org governance
- `fix(ci)` commits (>20) — their CI infrastructure
- `fix(scripts)`: one-line install script — zeroclaw-branded
- `fix(docker)` commits — their Docker publishing

---

## Port Order (Recommended)

Given conflicts with our multi-agent + MAPLE additions, recommended porting sequence:

### Phase 1 — Must Do (Blocking / Safety)
1. **Memory trait `session_id` parameter** (`ebb78af`) — compile-breaking if we use upstream memory APIs; merge with our composite/ephemeral changes
2. **`Channel::send()` → `SendMessage` refactor** (`dbebd48`) — breaking channel API; needed before new channels
3. **Security policy hardening** (`9ec1106`, `a2986db`, `87dcd7a`, `290d971`, `bbe5530`) — apply atomically, low conflict
4. **Agent loop native tool call fixes** (`f75f73a`, `0e5a785`) — fix history corruption; high conflict with MAPLE changes, manual merge
5. **Credential scrubbing in tool output** (`df31359`) — security, touches loop we're already fixing

### Phase 2 — High Value Ports
6. **Anthropic prompt caching** (`455eb3b`) — pure provider change, immediately reduces cost
7. **Memory `spawn_blocking` + relevance threshold** (`dd45417`, `decea53`) — check MAPLE weight conflicts
8. **Cron refactor** (`fb2d1ce`) — large, do after scheduler.rs conflicts resolved
9. **Web search tool** (`1757add`) — net new capability, low risk
10. **GLM provider** (`9d0e299`) — net new, isolated

### Phase 3 — New Channels & Features
11. Mattermost, Signal, QQ channels
12. Streaming support (large PR, do separately)
13. Prometheus observability
14. Postgres memory backend

---

## Conflict Hotspots Summary

| File | Our Changes | Upstream Changes | Risk |
|------|-------------|-----------------|------|
| `src/memory/composite.rs` | MAPLE composite memory | `session_id` params on all methods | **HIGH** |
| `src/memory/ephemeral.rs` | MAPLE ephemeral store | `session_id` params | **HIGH** |
| `src/cron/scheduler.rs` | Our scheduler extensions | Full cron module refactor | **HIGH** |
| `src/agent/loop_.rs` | MAPLE recall integration | Native tool history, credential scrubbing, GLM parsing | **HIGH** |
| `src/service/mod.rs` | Our additions | Minor channel/daemon changes | Medium |
| `src/tools/agent_manage.rs` | Multi-agent tool | Memory category exposure | Medium |
| `src/security/policy.rs` | Agent sandboxing | `tee` block, `find`/`git` arg safety, `ToolOperation` | Medium |
| `src/providers/anthropic.rs` | Our provider config | Prompt caching, OAuth beta header | Low-Medium |
