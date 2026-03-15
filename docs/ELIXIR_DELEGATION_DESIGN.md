# Elixir Delegation Design — Moving DelegateTool from Rust to Elixir/OTP

*Branch: `feature/elixir-delegation` | Created: 2026-03-14*

---

## 1. Problem Statement

Today, delegation lives entirely in Rust (`src/tools/delegate.rs`). The DelegateTool:
- Creates a throwaway provider for each sub-agent call
- Runs a filtered tool-call loop inline (blocking the parent's agent loop)
- Has no supervision — if a sub-agent hangs, it blocks until timeout
- Sub-agents are ephemeral function calls, not managed processes
- No inter-agent messaging beyond "delegate and wait"
- No way for a sub-agent to persist, restart, or be monitored independently
- Tool filtering is a runtime allowlist against the parent's registry — no per-agent tool registration

Meanwhile, the Elixir orchestration layer already has:
- AgentServer (GenServer per agent with lifecycle, health checks, snapshots)
- AgentSupervisor (DynamicSupervisor with crash recovery)
- AgentCoordinator (capability routing, delegation ACL, fanout/sequential strategies)
- AgentRegistry (Elixir Registry for process lookup)
- RustBridge (HTTP bridge to Rust core for tool execution)
- SubAgentSession (ETS-backed session persistence)
- MessageProvenance (trace propagation through delegation chains)
- ResourceLock (ETS-based exclusive resource locking with priority preemption)

**The gap:** Elixir has the orchestration infrastructure but delegation still goes through Rust. The Rust DelegateTool bypasses all of it.

**Goal:** Make delegation flow through Elixir. Sub-agents become supervised GenServer processes with their own tool registries, managed by OTP, communicating via BEAM messages — while still using the Rust core for actual LLM calls and tool execution.

---

## 2. What Changes

### Moves to Elixir
| Concern | Current (Rust) | New (Elixir) |
|---------|---------------|--------------|
| Delegation routing | DelegateTool picks agent from HashMap | AgentCoordinator routes by capability + ACL |
| Sub-agent lifecycle | Ephemeral function call | Supervised GenServer (AgentServer) |
| Tool registry per agent | Filtered subset of parent's tools | Per-agent `allowed_tools` in AgentDefinition, enforced by Elixir before calling Rust |
| Crash recovery | None (timeout or hang) | OTP supervisor restarts |
| Inter-agent messaging | None (call and wait) | BEAM messages: sync (call) and async (cast) |
| Session tracking | None | SubAgentSession in ETS |
| Delegation strategy | Single (call one agent) | first_available, sequential, fanout |
| Depth limiting | Depth counter in Rust | Delegation depth in MessageProvenance, enforced by Coordinator |
| Resource contention | None | ResourceLock with priority preemption |

### Stays in Rust
| Concern | Why |
|---------|-----|
| LLM provider calls | Provider implementations, auth, streaming, token counting — all Rust |
| Tool execution | Shell, file I/O, memory, browser, cron — all Rust |
| Security policy | Credential scrubbing, path traversal checks, rate limiting — Rust |
| Channel handling | Telegram, Signal, Discord etc. — Rust |
| Agent loop (`run_tool_call_loop`) | The actual LLM ↔ tool iteration — Rust |

---

## 3. Architecture

```
User Message
    │
    ▼
┌─────────────────────────────────────────────────────────┐
│                  Rust Core (Gateway)                    │
│                                                         │
│  1. Receives message from channel                       │
│  2. Runs primary agent loop                             │
│  3. Primary agent calls "delegate" tool                 │
│  4. NEW: delegate tool sends HTTP request               │
│     to Elixir instead of running sub-agent inline       │
└────────────────────┬────────────────────────────────────┘
                     │ HTTP POST /api/delegate
                     ▼
┌─────────────────────────────────────────────────────────┐
│                  Elixir/OTP Layer                       │
│                                                         │
│  5. DelegationRouter receives request                   │
│  6. AgentCoordinator resolves target agent(s)           │
│     - Capability matching                               │
│     - ACL enforcement                                   │
│     - Strategy selection (first/sequential/fanout)      │
│  7. AgentSupervisor spawns AgentServer if not running   │
│  8. AgentServer receives task                           │
│     - Validates tool allowlist                          │
│     - Tracks session in ETS                             │
│     - Records provenance                                │
│                                                         │
│  9. AgentServer calls RustBridge to execute             │
└────────────────────┬────────────────────────────────────┘
                     │ HTTP POST /api/agent/run
                     ▼
┌─────────────────────────────────────────────────────────┐
│                  Rust Core (Agent Runner)               │
│                                                         │
│  10. Runs sub-agent's LLM call with:                    │
│      - Agent-specific provider/model                    │
│      - Agent-specific system prompt                     │
│      - Filtered tool set (from allowed_tools)           │
│      - Security policy applied                          │
│  11. If agentic: runs tool-call loop                    │
│  12. Returns result to Elixir                           │
└────────────────────┬────────────────────────────────────┘
                     │ HTTP Response
                     ▼
┌─────────────────────────────────────────────────────────┐
│                  Elixir/OTP Layer                       │
│                                                         │
│  13. AgentServer receives result                        │
│      - Updates session status                           │
│      - Records in history                               │
│      - Saves snapshot if persistent                     │
│  14. Returns result to DelegationRouter                 │
└────────────────────┬────────────────────────────────────┘
                     │ HTTP Response
                     ▼
┌─────────────────────────────────────────────────────────┐
│                  Rust Core (Primary Agent)              │
│                                                         │
│  15. DelegateTool receives result                       │
│  16. Primary agent continues with sub-agent's output    │
└─────────────────────────────────────────────────────────┘
```

**Key insight:** Two HTTP hops per delegation:
1. Rust → Elixir (delegate request)  
2. Elixir → Rust (execute sub-agent's LLM call)

This is intentional. Elixir owns the orchestration decisions (who runs, with what tools, under what constraints). Rust owns the execution (LLM calls, tool runs, security).

---

## 4. Functional Requirements

### 4.1 Delegation Flow

**FR-1: Basic delegation**
- Primary agent calls `delegate` tool with `{agent, prompt, context}`
- Rust DelegateTool sends HTTP POST to Elixir `/api/delegate`
- Elixir resolves agent, spawns if needed, executes via RustBridge
- Result returns to primary agent

**FR-2: Capability-based routing**
- Primary agent can delegate by capability instead of name: `{capabilities: ["research"], prompt: "..."}`
- AgentCoordinator finds matching agents and applies strategy
- If no agents match, returns clean error

**FR-3: Delegation ACL**
- Each agent defines `delegates_to: ["agent_a", "agent_b"]`
- AgentCoordinator enforces: agent X can only delegate to agents in its allowlist
- Empty `delegates_to` = can delegate to any agent (backward compat)

**FR-4: Delegation strategies**
- `first_available`: Route to first matching agent. Fast.
- `sequential`: Try agents in order, fall through on failure. Resilient.
- `fanout`: Send to all matching agents concurrently, collect all results. Thorough.
- Strategy specified per delegation call, defaults to `first_available`

### 4.2 Sub-Agent Tool Management

**FR-5: Per-agent tool allowlists**
- Each AgentDefinition has `allowed_tools: ["shell", "file_read", "web_search"]`
- When Elixir calls RustBridge to execute a sub-agent task, it passes the allowed tool list
- Rust core filters its tool registry to only those tools before running the agent loop
- Empty `allowed_tools` = no tools (pure LLM response, no tool loop)

**FR-6: Tool isolation**
- Sub-agents CANNOT access tools not in their allowlist
- The `delegate` tool is NEVER available to sub-agents (prevents infinite delegation without going through Elixir)
- Security policy still applies on top of the allowlist (defense in depth)

**FR-7: Tool exposure to LLM**
- When Rust runs a sub-agent, only the allowed tools appear in the LLM's tool schema
- The LLM doesn't even see tools it can't use — cleaner context, fewer hallucinated tool calls

### 4.3 Sub-Agent Lifecycle

**FR-8: Lazy spawning**
- Agents are spawned on first delegation, not at startup
- AgentSupervisor.spawn_agent creates a new AgentServer GenServer
- Subsequent delegations to the same agent reuse the running process

**FR-9: Persistent agents**
- Agents with `persistent: true` survive restarts via snapshots
- On crash, OTP supervisor restarts the GenServer
- State (accumulated_state, recent history) restored from snapshot

**FR-10: Ephemeral agents**
- Agents with `persistent: false` (default) are fire-and-forget
- They stay alive for a configurable idle timeout after last task
- Auto-terminate when idle (AgentServer sends itself a delayed `:idle_timeout` message)

**FR-11: Health monitoring**
- Each AgentServer runs periodic health checks (every 30s)
- 3-state health: healthy → degraded → unhealthy
- Unhealthy agents reject new tasks with `{:error, :unhealthy}`
- Supervisor restarts agents that crash (max 3 restarts per 5s)

### 4.4 Inter-Agent Communication

**FR-12: Synchronous delegation**
- `AgentServer.run_task(agent_name, task)` — call and wait for result
- Timeout enforced (configurable per agent, default 30s)
- Used for: "delegate this task and give me the answer"

**FR-13: Asynchronous messaging**
- `AgentServer.send_message(agent_name, message)` — fire and forget
- Used for: "notify this agent about something" without blocking
- Result delivered via `:child_report` message to parent

**FR-14: Parent-child relationships**
- When agent A delegates to agent B, B's `parent_pid` is set to A
- If A dies, B is notified via `:DOWN` monitor
- B can report results back to A via `report_to_parent`
- A tracks all children in `child_pids` MapSet

### 4.5 Resource Management

**FR-15: Exclusive resource locking**
- Resources like browser sessions need exclusive access
- ResourceLock provides ETS-based locking with priority preemption
- Main agent tasks (`:main`) can preempt BTW/side-channel tasks (`:btw`)
- Preempted process receives `{:resource_preempted, resource}` for graceful cleanup

**FR-16: Memory limits**
- Per-agent `max_memory_mb` enforced at GenServer level
- Memory checked before task execution and state updates
- Over-limit agents reject new work with `{:error, :memory_limit_exceeded}`

### 4.6 Observability

**FR-17: Message provenance**
- Every delegation creates a MessageProvenance record
- Tracks: trace_id, origin_agent, source_agent, delegation_depth, timestamps
- Propagated through the entire chain (Rust → Elixir → Rust → Elixir...)
- Stored in TraceStore for debugging

**FR-18: Session tracking**
- Every delegation creates a SubAgentSession in ETS
- Fields: id, agent_name, task, status, started_at, completed_at, result
- Queryable for monitoring/debugging

**FR-19: Structured logging**
- All delegation events logged with agent name, trace_id, timestamps
- Log levels: info for normal flow, warning for degraded/rejected, error for failures

---

## 5. Rust-Side Changes

### 5.1 DelegateTool becomes a thin HTTP client

The current `DelegateTool` does everything: provider creation, tool filtering, agent loop execution. The new version does one thing: sends an HTTP request to Elixir and returns the result.

**Current behavior (to be replaced):**
```
DelegateTool.execute() →
  create_provider() →
  if agentic: run_tool_call_loop() with filtered tools
  else: provider.chat_with_system()
```

**New behavior:**
```
DelegateTool.execute() →
  HTTP POST to Elixir /api/delegate {agent, prompt, context, provenance}
  ← wait for response
  return result
```

The DelegateTool struct shrinks dramatically:
- Remove: `parent_tools`, `depth`, `multimodal_config`, `provider_runtime_options`
- Keep: `security` (still enforce policy on the delegate call itself), `fallback_credential` (passed to Elixir for RustBridge auth)
- Add: `elixir_bridge_url` (Elixir HTTP endpoint)

### 5.2 New Rust endpoint: `/api/agent/run`

Elixir needs a way to tell Rust "run this agent task with these parameters." This endpoint:

**Request:**
```json
{
  "agent_name": "researcher",
  "task": "Research quantum computing",
  "system_prompt": "You are a research assistant.",
  "provider": "anthropic",
  "model": "claude-sonnet-4-20250514",
  "api_key": "sk-...",
  "temperature": 0.3,
  "allowed_tools": ["web_search", "web_fetch", "file_read"],
  "agentic": true,
  "max_iterations": 10,
  "provenance": { "trace_id": "...", "delegation_depth": 1 }
}
```

**Response:**
```json
{
  "success": true,
  "output": "Quantum computing uses qubits...",
  "tool_calls_made": ["web_search", "web_fetch"],
  "token_usage": { "input": 1500, "output": 800 },
  "duration_ms": 4500
}
```

**Key:** Rust doesn't decide which tools are available — Elixir tells it via `allowed_tools`. Rust filters its registry based on that list and runs the agent loop.

### 5.3 Agent config migration

Current config in `config.toml`:
```toml
[agents.researcher]
provider = "anthropic"
model = "claude-sonnet-4-20250514"
system_prompt = "You are a research assistant."
agentic = true
allowed_tools = ["web_search", "web_fetch"]
max_iterations = 10
```

This config needs to be readable by **both** layers:
- Elixir reads it via AgentDefinition (YAML files in `~/.rustyclaw/agents/`) for orchestration decisions
- Rust reads `allowed_tools` from the Elixir request to filter the tool registry

**Decision:** Agent definitions move to YAML files (Elixir's format). Rust `[agents.*]` config becomes legacy/deprecated. During migration, support both — Elixir reads YAML, falls back to querying Rust for TOML-defined agents.

---

## 6. Elixir-Side Changes

### 6.1 New: DelegationRouter (Plug endpoint)

New HTTP endpoint in the Elixir app that Rust's DelegateTool calls:

**POST `/api/delegate`**
```json
{
  "agent": "researcher",           // or null if routing by capability
  "capabilities": ["research"],     // used if agent is null
  "prompt": "Research this topic",
  "context": "Some prior context",
  "strategy": "first_available",    // optional, default: first_available
  "from_agent": "primary",          // for ACL enforcement
  "provenance": { ... },            // trace metadata
  "timeout_ms": 30000               // optional
}
```

**Flow:**
1. Validate request
2. If `agent` specified → resolve directly
3. If `capabilities` specified → AgentCoordinator.find_agents()
4. Check ACL (from_agent → target)
5. Spawn target agent if not running
6. Execute via chosen strategy
7. Return result

### 6.2 AgentServer: Connect to RustBridge for real execution

Currently, `AgentServer.handle_call({:run_task, ...})` returns `{:ok, %{status: :pending_bridge}}`. This needs to actually call RustBridge:

```
handle_call({:run_task, task, provenance}) →
  1. Check health (reject if unhealthy)
  2. Check memory limit
  3. Build RustBridge request from AgentDefinition:
     - provider, model, system_prompt from definition
     - allowed_tools from definition
     - agentic = true if allowed_tools non-empty
  4. Call RustBridge.run_task(agent_name, task, opts)
  5. Record result in history
  6. Update SubAgentSession
  7. Return result
```

### 6.3 AgentCoordinator: Wire up delegation strategies

The strategies exist in code but need to be connected to real RustBridge execution. Specifically:

- **fanout**: Use `Task.async_stream` to parallelize RustBridge calls, collect results
- **sequential**: Try agents in order, short-circuit on first success
- **first_available**: Pick one, call it (already works)

### 6.4 New: Idle timeout for ephemeral agents

AgentServer gets a configurable idle timeout:
- After completing a task, schedule `:idle_timeout` message
- On timeout, GenServer terminates normally
- Supervisor doesn't restart (`:transient` restart strategy — only restarts on abnormal exit)
- Default: 5 minutes idle → terminate

### 6.5 Tool allowlist propagation

When AgentServer builds a RustBridge request, it includes `allowed_tools` from the AgentDefinition. This is the **source of truth** for what tools a sub-agent can use.

Special rules:
- `delegate` tool is NEVER included (even if listed) — prevents re-entrant delegation that bypasses Elixir
- If `allowed_tools` is empty → non-agentic mode (single LLM call, no tool loop)
- Rust-side security policy applies ON TOP of the allowlist

---

## 7. Migration Plan

### Phase 1: Bridge endpoint (non-breaking)
- Add `/api/agent/run` endpoint to Rust (accepts agent config + task, returns result)
- Add `/api/delegate` endpoint to Elixir
- Both are new endpoints — nothing existing changes
- **Test:** Call both endpoints directly with curl

### Phase 2: Wire Elixir delegation path (feature-flagged)
- Add `delegation_mode` config: `"rust"` (default) or `"elixir"`
- When `"elixir"`: DelegateTool sends to Elixir instead of running inline
- When `"rust"`: Current behavior unchanged
- AgentServer calls RustBridge.run_task for real execution
- **Test:** Run same delegation tasks through both paths, compare results

### Phase 3: Agent definition migration
- Create YAML agent definition files from existing TOML config
- Elixir loads from YAML, falls back to TOML query for backward compat
- **Test:** Define agents in both formats, verify same behavior

### Phase 4: Remove Rust delegation code
- Once Elixir path is stable, flip default to `"elixir"`
- Deprecate `[agents.*]` TOML config (keep parsing, emit warning)
- Remove inline tool filtering from DelegateTool
- Remove `parent_tools`, `depth`, `multimodal_config` from DelegateTool struct
- **Test:** Full E2E suite passes with Elixir-only delegation

### Phase 5: Advanced features (post-migration)
- Fanout/sequential strategies in real use
- Persistent agent snapshots across restarts
- Agent-to-agent async messaging (not just delegate-and-wait)
- Dynamic agent spawning at runtime (user creates agents on the fly)
- Resource locking for shared resources (browser sessions, serial ports)

---

## 8. What We Gain

| Feature | Rust-only (today) | Elixir delegation (proposed) |
|---------|-------------------|------------------------------|
| Crash recovery | None — sub-agent crash = lost work | OTP supervisor auto-restarts |
| Concurrent agents | Blocked — inline execution | True concurrency via BEAM processes |
| Agent monitoring | None | Health checks, session tracking, provenance |
| Delegation strategy | One agent at a time | first_available, sequential, fanout |
| Tool isolation | Runtime filter (same process) | Process-level isolation (different GenServers) |
| Persistent agents | None | Snapshot + restore across restarts |
| Inter-agent messaging | None | Sync and async via BEAM |
| Resource management | None | Exclusive locks with priority preemption |
| Depth limiting | Integer counter | Provenance-tracked with full trace chain |
| Idle cleanup | None — agents don't exist as processes | Auto-terminate after idle timeout |
| Observability | Log lines | Structured traces, session records, health states |

---

## 9. What We Lose (Temporarily)

- **Latency:** Two HTTP hops instead of inline function call. Expected overhead: ~5-15ms per delegation (localhost HTTP). Acceptable for most use cases.
- **Simplicity:** More moving parts (Elixir app must be running alongside Rust). Mitigated by daemon mode starting both.
- **Single-binary deployment:** Currently RustyClaw is one binary. With Elixir, it's two processes. Mitigated by the daemon orchestrating both.

---

## 10. Risks & Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| Elixir app crashes, delegation breaks | Medium | High | Feature flag — fall back to Rust inline delegation |
| HTTP bridge latency too high | Low | Medium | Benchmark. If needed, migrate to Erlang Port (stdin/stdout JSON) |
| Config format split (TOML vs YAML) | Medium | Medium | Phase 3 migration. Emit warnings on TOML usage. |
| Fanout strategy overwhelms Rust | Low | Medium | Concurrency limit in AgentCoordinator (max N parallel calls) |
| Sub-agent tool call escapes allowlist | Low | Critical | Defense in depth: Elixir sends allowlist, Rust filters AND applies SecurityPolicy |

---

## 11. Open Questions

1. **Erlang Port vs HTTP for RustBridge?** — Start with HTTP (simpler debugging, already implemented). Migrate to Port if latency matters.

2. **Agent definition format?** — YAML (Elixir's current format) vs TOML (Rust's format). Recommendation: YAML for new agents, backward-compat TOML parsing with deprecation warning.

3. **Should the primary agent also run through Elixir?** — Not in this phase. Primary agent stays in Rust's agent loop. Only delegated sub-agents go through Elixir. This limits blast radius.

4. **Fanout result aggregation?** — When 3 agents return 3 different results, how does the primary agent use them? Options: concatenate, let primary agent choose, structured merge. **Recommendation:** Return all results as a JSON array, let the primary agent's LLM decide how to use them.

5. **Sub-agent delegating to another sub-agent?** — Supported via delegation depth tracking in provenance. Sub-agent A can call delegate (which goes to Elixir), which spawns sub-agent B. Depth is incremented. Max depth enforced by AgentCoordinator.

---

## 12. Linear Issues for Implementation

| Phase | Issue | Description |
|-------|-------|-------------|
| 1 | Rust: `/api/agent/run` endpoint | Accept agent config + task, run filtered agent loop, return result |
| 1 | Elixir: `/api/delegate` endpoint (DelegationRouter) | Accept delegation request, route to AgentCoordinator |
| 2 | Elixir: AgentServer real execution via RustBridge | Replace pending_bridge placeholder with actual bridge call |
| 2 | Rust: DelegateTool HTTP mode | Feature-flagged: send to Elixir instead of inline execution |
| 2 | Config: `delegation_mode` flag | "rust" (default) or "elixir" |
| 3 | Agent definition YAML migration | Create YAML files, backward-compat TOML parsing |
| 3 | Elixir: Idle timeout for ephemeral agents | Auto-terminate after configurable idle period |
| 4 | Rust: Strip inline delegation code | Remove parent_tools, depth, multimodal_config from DelegateTool |
| 4 | Default flip: delegation_mode = "elixir" | Make Elixir the default path |
| 5 | Elixir: Fanout strategy with RustBridge | Parallel execution, result aggregation |
| 5 | Elixir: Dynamic agent spawning (runtime) | Users create agents on the fly |

---

*This document is the source of truth for implementation. Code follows design, not the other way around.*
