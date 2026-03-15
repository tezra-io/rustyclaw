# Elixir Delegation Design — Moving DelegateTool from Rust to Elixir/OTP

*Branch: `feature/elixir-delegation` | Created: 2026-03-14*
*Revised: 2026-03-14 — incorporated Codex + Claude Code reviews*

---

## Revision Log

**Rev 2 (2026-03-14):** Major revision based on independent reviews from Codex (gpt-5.3) and Claude Code.

Key changes:
1. **Switched to one-hop architecture** — Elixir returns routing decision, Rust executes. Eliminates second HTTP hop.
2. **Credentials stay in Rust** — Elixir sends `agent_name`, Rust resolves provider/api_key internally. Fixes security regression.
3. **Added Phase 0** — prerequisite work (Elixir HTTP server, AgentDefinition field gaps) called out explicitly.
4. **Scoped down migration** — `first_available` strategy only for initial rollout. Fanout/sequential deferred.
5. **Separated existing Elixir bugs** from design changes. Pre-existing issues tracked as regular Linear issues (not feature branch).
6. **Added cancellation propagation** and timeout alignment requirements.

See reviews: `docs/CODEX_DESIGN_REVIEW.md`, `docs/CLAUDE_CODE_DESIGN_REVIEW.md`

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

## 3. Architecture (Revised — One-Hop Model)

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
│  4. DelegateTool sends HTTP request to Elixir           │
└────────────────────┬────────────────────────────────────┘
                     │ HTTP POST /api/delegate
                     │ {agent_or_capabilities, prompt, context, from_agent}
                     ▼
┌─────────────────────────────────────────────────────────┐
│                  Elixir/OTP Layer                       │
│                                                         │
│  5. DelegationRouter receives request                   │
│  6. AgentCoordinator resolves target agent              │
│     - Capability matching (from definition files)       │
│     - ACL enforcement                                   │
│     - Spawn AgentServer if not running                  │
│  7. AgentServer records session in ETS                  │
│  8. Returns routing decision to Rust:                   │
│     {agent_name, allowed_tools, provenance}             │
│     (NO credentials — Rust resolves those)              │
└────────────────────┬────────────────────────────────────┘
                     │ HTTP Response (routing decision)
                     ▼
┌─────────────────────────────────────────────────────────┐
│                  Rust Core (DelegateTool)               │
│                                                         │
│  9. Resolves agent config from [agents.*] TOML          │
│     - Provider, model, api_key from own config          │
│     - allowed_tools from Elixir's response              │
│  10. Runs sub-agent loop (same as today):               │
│      - Creates provider with in-process credentials     │
│      - Filters tool registry by allowed_tools           │
│      - Applies SecurityPolicy                           │
│      - If agentic: runs tool-call loop                  │
│  11. Returns result to primary agent                    │
│  12. Async: POST result back to Elixir for tracking     │
└────────────────────┬────────────────────────────────────┘
                     │ (async) POST /api/delegate/result
                     ▼
┌─────────────────────────────────────────────────────────┐
│                  Elixir/OTP Layer                       │
│                                                         │
│  13. AgentServer receives result callback               │
│      - Updates session status                           │
│      - Records in history                               │
│      - Saves snapshot if persistent                     │
└─────────────────────────────────────────────────────────┘
```

**Key insight (Rev 2):** One HTTP hop for routing, execution stays in Rust.

Previous design had two synchronous HTTP hops (Rust→Elixir→Rust). Both reviewers flagged this as overengineered and a security risk (API keys over HTTP). The revised architecture:

1. **Rust → Elixir** (synchronous): "Who should handle this?" Elixir returns routing decision + allowed_tools.
2. **Rust executes locally**: Uses its own config for credentials, runs the sub-agent loop it already knows how to run.
3. **Rust → Elixir** (async callback): "Here's what happened." For session tracking/observability. Non-blocking.

**Benefits over two-hop:**
- Credentials never leave Rust process (no security regression)
- One fewer synchronous HTTP call per delegation (~50% less latency overhead)
- Existing `run_tool_call_loop` works unchanged (no need to externalize the 16-parameter function)
- Elixir still owns all orchestration decisions

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

### 5.1 DelegateTool becomes routing-aware

The current `DelegateTool` does everything: agent lookup, provider creation, tool filtering, agent loop execution. The new version splits into two paths based on `delegation_mode`:

**Current behavior (preserved as fallback):**
```
DelegateTool.execute() →
  lookup agent from HashMap →
  create_provider() →
  if agentic: run_tool_call_loop() with filtered tools
  else: provider.chat_with_system()
```

**New behavior (when delegation_mode = "elixir"):**
```
DelegateTool.execute() →
  HTTP POST to Elixir /api/delegate {agent, capabilities, prompt, context, from_agent}
  ← receive routing decision: {resolved_agent_name, allowed_tools, provenance}
  lookup resolved agent config from own [agents.*] TOML (credentials stay local)
  create_provider() with in-process credentials
  filter tool registry by allowed_tools from Elixir
  run sub-agent loop (same as today)
  async POST result to Elixir /api/delegate/result for tracking
  return result to primary agent
```

**Struct changes:**
- Keep: `agents`, `parent_tools`, `security`, `fallback_credential`, `depth`, `provider_runtime_options`, `multimodal_config`
- Add: `elixir_bridge_url` (Elixir HTTP endpoint), `delegation_mode` (config flag)
- The struct stays largely the same — it still runs the sub-agent loop, just asks Elixir for routing first

### 5.2 No new `/api/agent/run` endpoint needed (Rev 2)

The original design proposed a Rust endpoint for Elixir to call back into. With the one-hop model, this is **no longer needed**. Rust runs the sub-agent loop itself after getting the routing decision. This eliminates:
- The need to externalize `run_tool_call_loop` (16-parameter function)
- API key transmission over HTTP
- A second synchronous HTTP hop

The only new Rust HTTP call is an **async result callback** to Elixir (`POST /api/delegate/result`) for session tracking. This is fire-and-forget — if it fails, delegation still succeeds.

### 5.3 Agent config: single source of truth

**Decision (Rev 2):** Keep `[agents.*]` TOML as the single source of truth for execution config (provider, model, api_key, temperature, agentic, max_iterations). Don't duplicate in YAML.

Elixir YAML agent definitions add orchestration-only fields:
- `capabilities`, `delegates_to`, `persistent`, `memory`, `channels`, `schedule`

The split is clean:
- **TOML (Rust):** How to run the agent (provider, credentials, tool config)
- **YAML (Elixir):** How to manage the agent (routing, lifecycle, permissions)

Rust reads TOML. Elixir reads YAML. No duplication, no reconciliation needed.

---

## 6. Elixir-Side Changes

### 6.0 Phase 0 Prerequisites (NEW — from review)

Before any delegation work, the Elixir app needs:

1. **HTTP server scaffold** — Add Bandit + Plug.Router to the supervision tree. The Elixir app currently has NO web server. This is non-trivial: deps, router module, port config, health endpoint.

2. **AgentDefinition field gaps** — Add missing fields to NimbleOptions schema:
   - `provider` (string, required for Rust config lookup validation)
   - `agentic` (boolean, default: false — or infer from non-empty `allowed_tools`)
   - `max_iterations` (integer, default: 10)
   - Note: `api_key` is NOT added — credentials stay in Rust TOML only

3. **Agent discovery from definitions** — `find_matching_agents` currently only queries running processes. Must also read agent definition files from `~/.rustyclaw/agents/` to support capability routing before agents are spawned.

### 6.1 DelegationRouter (Plug endpoint)

New HTTP endpoint that Rust's DelegateTool calls:

**POST `/api/delegate`**
```json
{
  "agent": "researcher",           // or null if routing by capability
  "capabilities": ["research"],     // used if agent is null
  "from_agent": "primary",          // for ACL enforcement
  "provenance": { ... }             // trace metadata
}
```

**Response (routing decision — NOT execution result):**
```json
{
  "resolved_agent": "researcher",
  "allowed_tools": ["web_search", "web_fetch", "file_read"],
  "provenance": { "trace_id": "...", "delegation_depth": 1 },
  "session_id": "abc123"
}
```

**Flow:**
1. Validate request
2. If `agent` specified → resolve directly (check definition files exist)
3. If `capabilities` specified → AgentCoordinator.find_agents() (from definitions, not just running processes)
4. Check ACL (from_agent → target)
5. Spawn AgentServer if not running
6. Create SubAgentSession in ETS
7. Return routing decision (agent name + allowed_tools)

**POST `/api/delegate/result`** (async callback from Rust)
```json
{
  "session_id": "abc123",
  "success": true,
  "output": "...",
  "tool_calls_made": ["web_search"],
  "duration_ms": 4500
}
```
- Updates SubAgentSession status
- Records in AgentServer history
- Saves snapshot if persistent agent
- Fire-and-forget from Rust's perspective

### 6.2 AgentServer: Session tracking only (Rev 2)

In the one-hop model, AgentServer does NOT call RustBridge for execution. Instead it:
- Tracks session lifecycle (created → running → completed/failed)
- Records results from async callback
- Manages health checks and snapshots
- The `:pending_bridge` placeholder gets replaced with proper session state management

### 6.3 AgentCoordinator: first_available only (Rev 2)

**Scoped down for initial rollout.** Only `first_available` strategy.

Fanout and sequential are deferred until the basic path is stable. Rationale:
- Fanout multiplies failure probability (N agents × HTTP calls)
- Needs cancellation propagation to avoid orphaned Rust loops
- Sequential needs clear retry semantics
- first_available covers 90% of real use cases

### 6.4 Idle timeout for ephemeral agents

AgentServer gets a configurable idle timeout:
- After session completes, schedule `:idle_timeout` via `Process.send_after`
- Cancel timeout if new task arrives
- On timeout, GenServer terminates normally (`:normal` exit)
- Supervisor doesn't restart (`:transient` strategy — only restarts on abnormal exit)
- Default: 5 minutes idle → terminate
- Handle edge case: agent exits while waiting for async result callback

### 6.5 Tool allowlist propagation

AgentDefinition's `allowed_tools` is the source of truth. Returned in the routing decision for Rust to filter its registry.

Special rules:
- `delegate` tool is NEVER included (even if listed) — prevents re-entrant delegation bypassing Elixir
- If `allowed_tools` is empty → non-agentic mode (single LLM call, no tool loop)
- Rust-side SecurityPolicy applies ON TOP of the allowlist (defense in depth)

---

## 7. Migration Plan (Revised)

### Phase 0: Prerequisites (NEW)
- Scaffold Bandit + Plug.Router in Elixir app with `/health` endpoint
- Extend AgentDefinition with `provider`, `agentic`, `max_iterations` fields
- Fix `find_matching_agents` to read definition files from disk (not just running processes)
- Fix GenServer blocking anti-patterns in RustBridge and AgentCoordinator (see existing issue tickets)
- **Test:** Elixir app starts, serves health check, loads agent definitions from YAML files
- **Estimate:** This is prerequisite work and should be done before feature work begins

### Phase 1: Routing endpoint (non-breaking)
- Add `/api/delegate` endpoint to Elixir (returns routing decision only)
- Add `/api/delegate/result` callback endpoint to Elixir (async result tracking)
- Both are new endpoints — nothing existing changes
- **Test:** Call endpoints directly with curl, verify routing logic

### Phase 2: Wire DelegateTool to Elixir routing (feature-flagged)
- Add `delegation_mode` config: `"rust"` (default) or `"elixir"`
- When `"elixir"`: DelegateTool asks Elixir for routing, then executes locally
- When `"rust"`: Current behavior unchanged (full fallback)
- DelegateTool uses `allowed_tools` from Elixir response to filter its registry
- Async result callback after execution
- **Test:** Run same delegation tasks through both paths, compare results
- **Scope:** `first_available` strategy only

### Phase 3: Agent definition split
- Create YAML agent definition files for orchestration fields (capabilities, delegates_to, persistent)
- Keep `[agents.*]` TOML for execution fields (provider, model, api_key, temperature)
- No dual-format for the same fields — clean separation
- **Test:** Define agents with YAML + TOML, verify delegation works

### Phase 4: Stabilize and default
- Once Elixir routing is stable, flip default to `delegation_mode = "elixir"`
- Keep `"rust"` mode as permanent fallback (not deprecated — useful for single-binary deployments)
- **Test:** Full E2E suite passes with Elixir routing

### Phase 5: Advanced features (separate epic — NOT part of this migration)
- Fanout/sequential strategies (requires cancellation propagation)
- Persistent agent snapshots across restarts
- Agent-to-agent async messaging
- Dynamic agent spawning at runtime
- Resource locking for shared resources

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

- **Latency:** One HTTP hop for routing instead of inline HashMap lookup. Expected overhead: ~2-8ms per delegation (localhost HTTP, no execution in the hop). Much better than the original two-hop design (~5-15ms × 2).
- **Simplicity:** More moving parts (Elixir app must be running alongside Rust). Mitigated by daemon mode starting both. Mitigated further by `delegation_mode = "rust"` fallback for single-binary deployments.
- **Single-binary deployment:** Currently RustyClaw is one binary. With Elixir, it's two processes. Fallback mode preserves single-binary option.

---

## 10. Risks & Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| Elixir app crashes, delegation breaks | Medium | High | Feature flag — fall back to Rust inline delegation permanently available |
| HTTP routing latency too high | Low | Low | Only one hop now (routing only, not execution). Benchmark. Erlang Port fallback if needed. |
| Config split confusion (TOML vs YAML) | Low | Medium | Clean separation: TOML = execution, YAML = orchestration. No field overlap. |
| Sub-agent tool call escapes allowlist | Low | Critical | Defense in depth: Elixir sends allowlist, Rust filters AND applies SecurityPolicy |
| Async result callback fails | Medium | Low | Fire-and-forget from Rust. Delegation succeeds regardless. Session tracking may be stale. |
| Agent discovery misses unspawned agents | High (pre-fix) | High | Phase 0 fix: read definition files from disk, not just running processes |
| Timeout mismatch (caller vs worker) | Medium | Medium | Phase 0 fix: align all timeouts, define ownership. Rust owns execution timeout, Elixir owns routing timeout. |

---

## 11. Open Questions

1. **Erlang Port vs HTTP for routing?** — Start with HTTP (simpler debugging). Routing is a lightweight call (JSON in, JSON out). Port migration only if sub-millisecond routing matters.

2. **Should the primary agent also run through Elixir?** — Not in this phase. Primary agent stays in Rust's agent loop. Only delegated sub-agents get Elixir routing. This limits blast radius.

3. **Sub-agent delegating to another sub-agent?** — Supported via delegation depth tracking in provenance. Sub-agent A's DelegateTool calls Elixir, which routes to sub-agent B. Depth incremented at each hop. Max depth enforced by AgentCoordinator.

4. ~~**Fanout result aggregation?**~~ — Deferred to Phase 5 (separate epic).

5. ~~**Agent definition format?**~~ — Resolved: TOML for execution (Rust), YAML for orchestration (Elixir). No overlap.

---

## 12. Linear Issues for Implementation

### Feature branch issues (feature/elixir-delegation)

| Phase | Issue | Description |
|-------|-------|-------------|
| 0 | Elixir: Scaffold Bandit + Plug.Router | Add HTTP server to Elixir app with /health endpoint |
| 0 | Elixir: Extend AgentDefinition schema | Add provider, agentic, max_iterations fields |
| 0 | Elixir: Agent discovery from definition files | find_matching_agents reads YAML files, not just running processes |
| 1 | Elixir: `/api/delegate` endpoint | Returns routing decision (agent + allowed_tools), not execution result |
| 1 | Elixir: `/api/delegate/result` callback | Async result tracking from Rust |
| 2 | Rust: DelegateTool Elixir routing mode | Feature-flagged: ask Elixir for routing, execute locally |
| 2 | Config: `delegation_mode` flag | "rust" (default) or "elixir" |
| 3 | Agent definition YAML/TOML split | YAML for orchestration, TOML for execution |
| 3 | Elixir: Idle timeout for ephemeral agents | Auto-terminate after configurable idle period |
| 4 | Default flip: delegation_mode = "elixir" | Make Elixir routing the default |

### Existing Elixir issues (master branch — regular fixes)

These are pre-existing code issues found during design review, NOT part of the delegation feature. Tracked separately:

| Issue | Description | Severity |
|-------|-------------|----------|
| GenServer blocking in RustBridge | HTTP + retry sleeps inside handle_call block the process | High |
| GenServer blocking in AgentCoordinator | Strategy execution inside handle_call serializes all routing | High |
| Unlinked Task.start in AgentServer | delegate_to_child uses Task.start + GenServer.reply — can strand callers | Medium |
| Health checks ignore external deps | evaluate_health based on recovery_attempts, not bridge/provider state | Medium |
| Snapshot restore for non-persistent agents | maybe_restore_snapshot runs unconditionally, can load stale state | Low |
| AgentServer :pending_bridge placeholder | run_task returns placeholder instead of actual result | Medium |

---

*This document is the source of truth for implementation. Code follows design, not the other way around.*
