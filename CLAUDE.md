# RustyClaw

## Project
This is the upstream RustyClaw repository, a Rust AI agent runtime. We are extending it with an Elixir/OTP orchestration layer for multi-agent capabilities while keeping the Rust core responsible for channels, tools, providers, security, memory, gateway, cron, and observability.

## How to Work

### Planning
- Plan mode for any non-trivial task (3+ steps or architectural decisions)
- Detailed specs upfront — good plan = 1-shot implementation
- State assumptions explicitly before coding. If multiple interpretations exist, surface them instead of picking silently.
- If the request is ambiguous, ask. If a simpler approach exists, say so.
- For multi-step work, write a short plan in `step -> verify` form.
- If something goes sideways, STOP and re-plan

### Test-First (Mandatory)
1. Write failing tests that define correct behavior
2. Make them pass
3. Refactor while green

"Write failing tests, then make them pass" — not "implement this feature."

### Verification
1. Write failing tests
2. Implement to pass them
3. Typecheck: `cargo check`
4. Full test suite: `cargo test`
5. Lint: `cargo clippy --all-targets -- -D warnings`

Never mark done without proving it works.

## Code Rules (Non-Negotiable)

1. **Linear flow.** Max 2 nesting levels. Top to bottom.
2. **Bound loops.** Explicit max on retries, polls, recursion. Define cap behavior.
3. **Small functions.** 40-60 lines max. One job per function.
4. **Own resources.** Open → close on every path, including errors.
5. **Narrow state.** No module globals. Pass deps explicitly.
6. **Assert assumptions.** Guards and validation on every public function. Fail loud.
7. **Never swallow errors.** No bare `rescue`. No `{:error, _} -> :ok`. Log, raise, or return.
8. **Visible side effects.** I/O obvious at call site. Separate pure from effectful.
9. **Minimal indirection.** Readable > elegant. One layer of abstraction max.
10. **Surgical changes only.** Touch only what the request requires. Do not refactor adjacent code, comments, or formatting unless the task needs it. Remove only the dead code your change creates.
11. **Warnings = errors.** Linters, typecheckers, analyzers are hard gates. Zero warnings.

## Conventions
- Prefer explicit types, small modules, and `Result`-based error flow.
- Avoid `unwrap`/`expect` in library paths unless the invariant is truly impossible to violate.

## Commands
```sh
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
```

## Docs
- `docs/ELIXIR_ORCHESTRATION_DESIGN.md` — orchestration architecture and implementation plan
- `docs/session-bridge-design.md` — session bridge design and integration points
- `docs/commands-reference.md` — CLI commands
- `docs/providers-reference.md` — LLM provider configuration
- `docs/channels-reference.md` — channel setup
- `docs/config-reference.md` — configuration schema
- `docs/operations-runbook.md` — operational procedures
- `docs/troubleshooting.md` — debugging guide

## Don'ts
- Don't commit without running tests
- Don't implement without failing tests first
- Don't add abstractions you weren't asked for
- Don't silently choose among ambiguous interpretations
- Don't improve adjacent code that wasn't part of the request
- Don't assume intent on ambiguous bugs — ask

## Principles
- Simplest correct solution
- If 200 lines could be 50, rewrite it
- Find root causes, no band-aids
- Minimal blast radius
- Own mistakes — write a rule to prevent repeating

## Known Pitfalls
- Update this section every time the repo teaches you the same lesson twice.

---
_Every mistake is a rule waiting to be written._

## Preserved Project-Specific Notes
These notes came from the previous `CLAUDE.md`. Keep the template above as the primary operating guide, and use the preserved context below where it is still relevant.

Instructions for Claude agents working on this codebase.

## 1) What This Is
This is the **upstream RustyClaw** repository — a Rust AI agent runtime. The internal package name is `rustyclaw`, the local directory is `rustyclaw`.

RustyClaw is currently a **single-agent** system. We are extending it with an **Elixir/OTP orchestration layer** for multi-agent capabilities. The Rust core stays intact; Elixir replaces the agent lifecycle, registry, message bus, delegation, and supervision that were previously hand-rolled in Rust (~4,500 LOC, now removed and backed up at `rustyclaw-rust-backup/`).

**Read `docs/ELIXIR_ORCHESTRATION_DESIGN.md` before any orchestration work.** It contains the full architecture, subsystem breakdown, implementation order, and anti-patterns.

## 2) Architecture Overview
```
┌─────────────────────────────────────────────────┐
│              Elixir/OTP Layer (NEW)              │
│                                                  │
│  AgentRegistry ── AgentSupervisor ── AgentServer │
│  AgentCoordinator ── RustBridge ── SubAgentSession│
│                                                  │
│  Owns: agent lifecycle, multi-agent routing,     │
│        delegation, supervision, message passing  │
└──────────────────┬──────────────────────────────-┘
                   │ Erlang Port or HTTP (localhost)
┌──────────────────▼──────────────────────────────-┐
│              Rust/RustyClaw Core (KEEP)            │
│                                                   │
│  Channels (19): Telegram, Discord, Signal, etc.   │
│  Tools (37): shell, file, web, memory, browser    │
│  Providers (15): Anthropic, OpenAI, Gemini, etc.  │
│  Security: policy, pairing, secrets, sandbox      │
│  Memory: SQLite, markdown, Qdrant, Postgres       │
│  Gateway: Axum HTTP server, webhooks, SSE, WS     │
│  + approval, cron, daemon, observability, etc.    │
└───────────────────────────────────────────────────┘
```

**Rust stays responsible for:** channels, tool execution, LLM providers, security/approval, memory, gateway, cron, peripherals, observability.

**Elixir takes over:** agent lifecycle (spawn/stop/restart), agent registry, inter-agent messaging (BEAM message passing replaces `AgentBus`), capability-based routing, delegation ACL, supervisor trees (OTP replaces manual restart logic), session persistence (ETS replaces `SubAgentRegistry`).

## 3) Repository Map
### Rust Core (`src/`)

| Path | Purpose |
|------|---------|
| `src/main.rs` | CLI entrypoint, command routing (~68K) |
| `src/lib.rs` | Module exports, command enums |
| `src/agent/` | Single-agent orchestration loop (LLM calls, tool execution) |
| `src/providers/` | 15 LLM providers (Anthropic, OpenAI, Gemini, Ollama, Bedrock, etc.) |
| `src/channels/` | 19 messaging platforms (Telegram, Discord, Signal, Slack, etc.) |
| `src/tools/` | 37 tools (shell, file I/O, web, memory, browser, cron, hardware) |
| `src/memory/` | Memory backends (SQLite, markdown, Qdrant, Postgres) |
| `src/security/` | Policy, pairing, secrets, sandboxing, prompt guard |
| `src/gateway/` | Axum HTTP server, webhooks, SSE, WebSocket |
| `src/config/` | Configuration schema and loading |
| `src/daemon/` | Long-running service supervisor |
| `src/cron/` | Task scheduling (cron, one-shot, interval) |
| `src/observability/` | Prometheus, OpenTelemetry, logging |
| `src/peripherals/` | Hardware boards (STM32, RPi GPIO, Arduino) |
| `src/runtime/` | Platform abstraction (native, Docker) |
| `src/approval/` | Human-in-the-loop approval system |
| `src/hooks/` | Event-driven lifecycle hooks |
| `src/skills/` | User-defined skill manifests |
| `src/rag/` | Hardware datasheet retrieval |
| `src/auth/` | Authentication backends |
| `src/cost/` | Token usage and cost tracking |

### Key Traits (Extension Points)

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

### Elixir Orchestration (`elixir/rustyclaw_orchestrator/`)

| Path | Purpose |
|------|---------|
| `lib/rustyclaw_orchestrator.ex` | Top-level API (spawn, delegate, list, stop, sessions) |
| `lib/rustyclaw_orchestrator/application.ex` | OTP Application, supervision tree |
| `lib/rustyclaw_orchestrator/agent_definition.ex` | YAML frontmatter + markdown parser, NimbleOptions validation |
| `lib/rustyclaw_orchestrator/agent_server.ex` | GenServer per agent (health, tasks, messaging) |
| `lib/rustyclaw_orchestrator/agent_supervisor.ex` | DynamicSupervisor wrapper (spawn/stop/list) |
| `lib/rustyclaw_orchestrator/agent_coordinator.ex` | Capability routing, delegation ACL, strategies |
| `lib/rustyclaw_orchestrator/sub_agent_session.ex` | ETS-backed session lifecycle (pending/active/completed/failed) |
| `lib/rustyclaw_orchestrator/rust_bridge.ex` | HTTP bridge to Rust core with retry/backoff |

See `docs/ELIXIR_ORCHESTRATION_DESIGN.md` for the full architecture.

## 4) Build and Test
### Rust

```bash
# Format, lint, test (the standard validation cycle)
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test

# Full CI (if Docker available)
./dev/ci.sh all

# Install locally
cargo install --path .
```

Minimum Rust version: 1.87 (see `Cargo.toml` `rust-version`).

Release profile optimizes for binary size (`opt-level = "z"`, LTO, strip).

### Elixir (once scaffolded)

```bash
# Standard Elixir validation
mix deps.get
mix compile --warnings-as-errors
mix test
mix format --check-formatted
mix credo --strict
```

### Smoke Test (E2E)

```bash
# Requires at least one provider key: OPENROUTER_API_KEY or ANTHROPIC_API_KEY
OPENROUTER_API_KEY=sk-... ./scripts/smoke-test.sh
```

Builds the release binary, boots the gateway on a random port with pairing disabled,
validates `GET /health` returns 200, and runs a chat round-trip via `POST /webhook`.
Tests each available provider if multiple keys are set. Exit 0 = all pass.

## 5) Engineering Principles
These are implementation constraints, not suggestions.

- **KISS**: Prefer explicit control flow over meta-programming. Keep error paths obvious.
- **YAGNI**: No speculative config keys, trait methods, or feature flags without a concrete caller.
- **DRY + Rule of Three**: Duplicate small local logic for clarity. Extract only after three stable repetitions.
- **SRP**: One concern per module. Extend via trait implementation, not cross-module rewrites.
- **Fail Fast**: Explicit errors for unsupported states. Never silently broaden permissions.
- **Secure by Default**: Deny-by-default. Never log secrets. Minimize network/filesystem/shell scope.
- **Determinism**: Reproducible commands, deterministic tests, locked dependencies.
- **Reversibility**: Small scope changes, clear rollback paths, no mixed mega-patches.

## 6) Agent Instructions (READ THIS)
### Before Any Work

1. **Read `docs/ELIXIR_ORCHESTRATION_DESIGN.md`** — it is the source of truth for orchestration architecture.
2. **Read this file** — understand what stays in Rust vs what moves to Elixir.
3. **Explore `src/` before editing** — understand existing patterns, traits, and factory wiring.

### For Rust Changes

- Extend via trait implementation + factory registration. Don't cross subsystem boundaries.
- Keep dependency direction inward: concrete integrations depend on traits/config, not on each other.
- High-risk paths: `src/security/`, `src/runtime/`, `src/gateway/`, `src/tools/`.

## ⛔ MANDATORY PRE-COMMIT GATE (DO NOT SKIP)
**Before EVERY `git commit`, run the full gate for the code you changed. No exceptions.**

A pre-commit hook enforces this, but if you're using `--no-verify` or committing programmatically, run these manually:

### Rust changes (any `.rs` file modified):
```bash
cargo fmt                                  # Fix formatting
cargo clippy --all-targets -- -D warnings  # Zero warnings allowed
cargo test --quiet                         # All tests must pass
```

### Elixir changes (any file under `elixir/`):
```bash
cd elixir/rustyclaw_orchestrator
mix format                          # Fix formatting
mix compile --warnings-as-errors    # Zero warnings allowed
mix credo --strict                  # Linting must pass
mix test --quiet                    # All tests must pass
```

### Both changed? Run both gates.

**If any gate fails: fix it before committing. Do NOT commit with known failures.**
**Do NOT use `git commit --no-verify` to bypass the hook.**

### For Elixir Changes

- **Always check hexdocs before implementing.** Run `mix hex.docs online <package>` or fetch the hexdocs URL. Do NOT guess OTP APIs.
- **Never hallucinate Elixir APIs.** If you're unsure whether a function exists, look it up. GenServer, DynamicSupervisor, Registry, and ETS all have specific APIs — don't invent callbacks or options.
- Use OTP primitives (GenServer, DynamicSupervisor, Registry) — do NOT reinvent supervision or process tracking.
- Do NOT use global state. Pass pids/names as arguments.
- Do NOT use `Process.register/2` globally. Use `Registry` for namespaced lookups.
- Do NOT call `GenServer.call` with `:infinity` timeout.
- Reference implementations to study before writing orchestration code:
  - [SwarmEx](https://github.com/nrrso/swarm_ex) — minimal OTP agent pattern
  - [Quoracle](https://github.com/shelvick/quoracle) — production Elixir agent system
  - [AgentSessionManager](https://github.com/nshkrdotcom/agent_session_manager) — session lifecycle

### For the Rust-Elixir Bridge

- Start with HTTP (Rust exposes localhost endpoints, Elixir calls via `Req`).
- Migrate to Erlang Port (stdin/stdout JSON framing) if latency matters.
- The bridge is the only coupling point between layers. Keep it narrow and well-typed.

## 7) What Was Removed (and Why)
The following Rust modules were our custom multi-agent additions. They have been removed from this repo (backed up at `rustyclaw-rust-backup/`) and will be replaced by Elixir:

| Removed | Elixir Replacement |
|---------|-------------------|
| `agent/bus.rs` | BEAM message passing |
| `agent/coordinator.rs` | AgentCoordinator GenServer |
| `agent/capability_registry.rs` | Elixir Registry |
| `agent/registry.rs` | Elixir Registry + filesystem loader |
| `agent/runner.rs` | AgentServer + AgentSupervisor |
| `agent/session.rs` | SubAgentSession + ETS |
| `tools/subagent_spawn.rs` | Elixir `spawn_agent/2` |
| `tools/subagent_manage.rs` | Elixir `list_agents/1`, `kill_agent/1` |
| `tools/subagent_list.rs` | Elixir `list_agents/1` |
| `tools/subagent_registry.rs` | AgentSupervisor + Registry |
| `tools/delegate.rs` | AgentCoordinator delegation |

**Everything else stays in Rust** — channels, providers, tools, memory, security, gateway, approval, cron, daemon, peripherals, observability.

## 8) Risk Tiers
- **Low**: docs, chore, tests-only
- **Medium**: most `src/**` behavior changes without security/boundary impact
- **High**: `src/security/**`, `src/runtime/**`, `src/gateway/**`, `src/tools/**`, `.github/workflows/**`, Elixir bridge, access-control boundaries

When uncertain, classify as higher risk.

## 9) Validation by Change Type
| Change Type | Required Checks |
|-------------|----------------|
| Rust code | `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test` |
| Elixir code | `mix compile --warnings-as-errors`, `mix test`, `mix format --check-formatted` |
| Docs only | Markdown lint, link integrity |
| Workflows | YAML syntax validation |
| Security/gateway/tools | Include boundary/failure-mode test |

## 10) Anti-Patterns
- Do not add heavy dependencies for minor convenience.
- Do not silently weaken security policy.
- Do not add speculative config/feature flags.
- Do not mix formatting-only changes with functional changes.
- Do not modify unrelated modules "while here".
- Do not bypass failing checks without explanation.
- Do not hallucinate Elixir/OTP APIs — read the docs.
- Do not reinvent OTP primitives in Rust or Elixir.
- Do not couple Elixir orchestration to Rust internals beyond the bridge interface.

## 11) Key Reference Docs
- `docs/ELIXIR_ORCHESTRATION_DESIGN.md` — orchestration architecture and implementation plan
- `docs/session-bridge-design.md` — session bridge (Telegram ↔ Claude Code) design and integration points
- `docs/commands-reference.md` — CLI commands
- `docs/providers-reference.md` — LLM provider configuration
- `docs/channels-reference.md` — channel setup
- `docs/config-reference.md` — configuration schema
- `docs/operations-runbook.md` — operational procedures
- `docs/troubleshooting.md` — debugging guide
- `CONTRIBUTING.md` — contribution guidelines
