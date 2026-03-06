# Elixir Orchestration Layer — Design Document
*TEZ-136 follow-up | Created: 2026-03-06*

---

## Context & Decision

ZeroClaw (upstream) is a single-agent Rust system. The multi-agent orchestration layer (AgentBus, SubAgentRegistry, DelegateTool, AgentCoordinator) was built entirely by us on top of it. After a spike analysis (TEZ-136), the decision is:

**Rewrite the orchestration layer in Elixir/OTP. Keep the Rust core (channels, tools, security, providers) intact.**

The Rust base provides: channels (Signal, Telegram, etc.), tool execution (shell, file, web), LLM providers, security/approval model, memory system.

The Elixir layer replaces: agent lifecycle, registry, message bus, delegation, supervisor trees, capability routing.

---

## Architecture

```
┌─────────────────────────────────────────────────────────┐
│                  Elixir/OTP Layer                       │
│                                                         │
│  Application.Supervisor                                 │
│    ├── AgentRegistry (Elixir Registry, :unique)        │
│    ├── AgentSupervisor (DynamicSupervisor)             │
│    │     ├── AgentServer (GenServer) × N               │
│    │     └── ...                                       │
│    ├── AgentCoordinator (GenServer)                    │
│    │     └── capability routing, delegation ACL        │
│    └── BridgeSupervisor                                │
│          └── RustBridge (GenServer → Erlang Port)      │
│                                                        │
└────────────────────┬───────────────────────────────────┘
                     │ Erlang Port (stdin/stdout JSON)
                     │ or HTTP (localhost)
┌────────────────────▼───────────────────────────────────┐
│                  Rust/ZeroClaw Core                     │
│                                                         │
│  - Channel handlers (Signal, Telegram, Discord, etc.)  │
│  - Tool execution (shell, file, web, memory)           │
│  - LLM providers (Anthropic, OpenAI, etc.)             │
│  - Security policy (credential scrubbing, ACL)         │
│  - Memory system                                       │
│  - Approval system                                     │
│                                                        │
└────────────────────────────────────────────────────────┘
```

---

## Elixir Subsystems

### 1. AgentDefinition (data layer)
- Parse agent definition files (YAML frontmatter + markdown body)
- Load from `~/.zeroclaw/agents/*.md`
- Validate fields: name, model, capabilities, delegates_to, persistent
- No Elixir equiv exists — build from scratch using `YamlElixir` + `NimbleOptions`

### 2. AgentRegistry
- Maps agent name → pid
- Use Elixir `Registry` (stdlib, `:unique` mode) — zero custom code needed
- Supervisor registers agents on spawn, deregisters on termination automatically
- **Reference**: SwarmEx uses this pattern exactly (`SwarmEx.AgentRegistry`)

### 3. AgentServer (GenServer)
- One GenServer per agent instance
- State: `%{definition: AgentDefinition.t(), session_id: String.t(), status: atom(), history: list()}`
- Callbacks: `handle_call(:run_task, ...)`, `handle_cast(:send_message, ...)`, `handle_info(:health_check, ...)`
- Periodic health check via `Process.send_after(self(), :health_check, 30_000)`
- 3-state health model: `:healthy | :degraded | :unhealthy`
- **Reference**: SwarmEx `Agent` GenServer (~400 LOC → ours will be similar)

### 4. AgentSupervisor (DynamicSupervisor)
- `:one_for_one`, max 3 restarts per 5 seconds per agent
- Spawns/terminates AgentServer processes
- On crash: supervisor restarts automatically (no manual lifecycle code)
- **Reference**: SwarmEx `ClientSupervisor`, Quoracle supervision tree

### 5. AgentCoordinator (GenServer)
- Capability-based routing: given a task + required capabilities, find best agent
- Delegation strategies: `:sequential | :fanout | :first_available`
- Delegation ACL: per-agent `delegates_to` allowlist from definition
- **Reference**: No direct Elixir equivalent — build from scratch, but simpler than Rust version because Registry handles process lookup

### 6. SubAgentSession (struct + persistence)
- `%SubAgentSession{id, agent_name, task, status, started_at, completed_at, result}`
- Persist to ETS (in-memory, fast) with optional file flush
- **Reference**: AgentSessionManager's `Session` + `Run` structs

### 7. RustBridge (Erlang Port or HTTP)
**Option A — Erlang Port (recommended):**
- GenServer wrapping an Erlang Port pointing to the Rust binary
- JSON-framed messages over stdin/stdout
- Rust binary runs as a managed subprocess, supervised by OTP
- Crash → restart the port process automatically

**Option B — HTTP:**
- Rust binary exposes localhost HTTP server
- Elixir calls it via `Req` or `Finch`
- Simpler to debug, slightly more overhead

Start with Option B for development speed, migrate to Option A if latency matters.

### 8. Tools (Elixir-side wrappers)
- `spawn_agent/2`, `list_agents/1`, `delegate_to/3`, `kill_agent/1`
- These are Elixir functions calling AgentCoordinator/AgentSupervisor
- Not HTTP tools — native function calls in the Elixir layer
- Tool registration with the Rust side via the bridge for backwards compatibility

---

## Key Elixir Libraries to Use

| Library | Purpose | Hex |
|---------|---------|-----|
| `yaml_elixir` | Parse agent definition YAML frontmatter | hex.pm/packages/yaml_elixir |
| `nimble_options` | Validate agent definition fields | hex.pm/packages/nimble_options |
| `req` | HTTP client for Rust bridge (Option B) | hex.pm/packages/req |
| `jason` | JSON encode/decode | hex.pm/packages/jason |
| `telemetry` | Observability spans | built-in |
| `ex_unit` | Testing | built-in |

**Do NOT add**: LangChain-style frameworks, heavy AI libs — the LLM calls stay in Rust.

---

## What to Remove from ZeroClaw

The following Rust files implement what Elixir will replace. They should be **removed or stubbed** after the Elixir layer is working:

| File | Why Remove |
|------|-----------|
| `src/agent/bus.rs` | Replaced by BEAM message passing |
| `src/agent/coordinator.rs` | Replaced by AgentCoordinator GenServer |
| `src/agent/capability_registry.rs` | Replaced by Elixir Registry |
| `src/agent/runner.rs` | Replaced by AgentSupervisor + AgentServer |
| `src/agent/registry.rs` | Replaced by Elixir Registry |
| `src/agent/session.rs` | Replaced by SubAgentSession + ETS |
| `src/tools/subagent_spawn.rs` | Replaced by Elixir spawn_agent tool |
| `src/tools/subagent_manage.rs` | Replaced by Elixir list/kill tools |
| `src/tools/subagent_list.rs` | Replaced by Elixir list tool |
| `src/tools/subagent_registry.rs` | Replaced by AgentSupervisor/Registry |
| `src/tools/delegate.rs` | Replaced by AgentCoordinator delegation |

**Keep everything else** — channels, providers, memory, security, tools (shell/file/web), approval system.

---

## Reference Implementations

Before writing any Elixir code, Claude should read and understand these:

1. **SwarmEx** — https://github.com/nrrso/swarm_ex
   - Especially: `lib/swarm_ex/agent.ex`, `lib/swarm_ex/client.ex`, `lib/swarm_ex/application.ex`
   - Shows minimal OTP agent pattern

2. **AgentSessionManager** — https://github.com/nshkrdotcom/agent_session_manager
   - Especially: Session lifecycle, SessionServer pattern
   - Shows production-grade session management

3. **Quoracle** — https://github.com/shelvick/quoracle
   - Especially: supervision tree, PubSub isolation, capability groups
   - Shows full production Elixir agent system

4. **Spike analysis** — `docs/elixir-spike/01-swarmex.md` in this repo
   - Full comparison of Rust vs Elixir for each subsystem

5. **OTP docs** — https://hexdocs.pm/elixir/GenServer.html, DynamicSupervisor, Registry
   - If uncertain about any OTP primitive, READ THE DOCS, don't guess

---

## Implementation Order

1. **TEZ-141**: Scaffold — mix project, supervision tree, deps, CI
2. **TEZ-142**: AgentDefinition parser + AgentRegistry
3. **TEZ-143**: AgentServer (GenServer) + AgentSupervisor
4. **TEZ-144**: SubAgentSession persistence (ETS)
5. **TEZ-145**: AgentCoordinator (capability routing + delegation ACL)
6. **TEZ-146**: RustBridge (HTTP first, Port later)
7. **TEZ-147**: Integration tests + remove dead Rust code

---

## Anti-Patterns to Avoid

- **Do NOT use global state** — pass pids/names as arguments, not module attributes
- **Do NOT use Process.register globally** — use Registry for namespaced lookups
- **Do NOT call GenServer.call with infinity timeout** — always set explicit timeouts
- **Do NOT reinvent supervision** — use DynamicSupervisor, not manual pid tracking
- **Do NOT hallucinate Elixir APIs** — if unsure, fetch the hexdocs page and read it
- **Do NOT start with a big bang** — one subsystem at a time, tests first

---

## Success Criteria

- [ ] `mix test` passes with >90% coverage on orchestration layer
- [ ] Agent spawn/list/delegate/kill works end-to-end via Elixir
- [ ] Rust bridge routes tool calls correctly
- [ ] Dead Rust orchestration code removed
- [ ] CLAUDE.md updated with new architecture
- [ ] Zero use of `Arc<RwLock<>>` in agent orchestration paths
