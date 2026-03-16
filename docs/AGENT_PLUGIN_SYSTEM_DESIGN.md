# Agent Plugin System — Design Document

*Branch: `feature/agent-plugin-system` | Created: 2026-03-15*

---

## Problem

RustyClaw currently runs external agents (Claude Code, Codex) by spawning sub-agent sessions that shell out to CLI processes via PTY. This works but is fragile:

**Three layers of indirection.** Orchestrator → sub-agent session → CLI process. Each boundary loses type safety, error context, and control. A rate limit hit in the inner CLI process surfaces as a cryptic exit code or buried log line, not a structured error the orchestrator can act on.

**No rate limit visibility.** Detection relies on grep-scraping debug logs for `429` patterns, which produces false positives from timestamps and unrelated numeric strings. The actual `Retry-After` header — the one piece of information needed — is invisible to the orchestrator.

**No streaming progress.** Hourly cron monitors poll session history to infer whether an agent is making progress, stuck in a loop, or dead. This is a 60-minute blind spot. An agent can spin for 59 minutes before anyone notices.

**Context loss.** Each CLI spawn starts from scratch. No shared project memory, no accumulated understanding, no continuity between tasks.

**No extensibility model.** Adding a new external agent type means copy-pasting shell-out logic, cron monitoring, and ad-hoc log scraping. There's no shared interface, no capability discovery, no way for the orchestrator to reason about what a given agent can do.

---

## Tech Stack

| Component | Choice | Why |
|-----------|--------|-----|
| Orchestration runtime | Elixir/OTP | Already the orchestration layer. GenServers for lifecycle, DynamicSupervisor for crash recovery, ETS for state. |
| Plugin communication | Direct HTTP via `Req` | Most external services (LLM APIs, browser automation, research APIs) are HTTP-based. Direct calls give streaming, rate limit headers, and full OTP control. |
| Streaming | SSE parsing (Elixir) | `Req` supports streaming responses. Parse SSE events as they arrive for real-time progress tracking. |
| Tool execution | RustBridge → Rust tools | Existing shell, file, web, memory tools stay in Rust. Plugins call them via the bridge. |
| State persistence | ETS + optional file flush | Follows the SubAgentSession pattern. Fast in-memory, durable on demand. |

**What we are NOT adding:**
- No plugin marketplace, registry, or hot-install — plugins are configured in app config
- No plugin-to-plugin communication — plugins interact through the orchestrator
- No version management for plugins
- No UI for plugin management

---

## Approach

Replace PTY-based CLI spawning with a generic plugin behaviour that any external agent or service adapter implements. Each plugin is a supervised Elixir module that connects to an external service, executes tasks, streams results, and reports health. The PluginManager handles pool management, dispatch, and capability-based routing. OTP handles lifecycle and crash recovery.

### The Plugin Behaviour

Every plugin implements six callbacks:

```elixir
defmodule RustyclawOrchestrator.Plugins.Behaviour do
  @doc "Establish connection to external agent/service."
  @callback connect(config :: map()) :: {:ok, state :: term()} | {:error, reason :: term()}

  @doc "Send task, get streaming results."
  @callback execute(state :: term(), task :: map()) ::
              {:ok, result :: term(), new_state :: term()} | {:error, reason :: term()}

  @doc "Check liveness."
  @callback health(state :: term()) :: :healthy | :degraded | :unhealthy

  @doc "What this plugin can do."
  @callback capabilities() :: [atom()]

  @doc "Current rate limits and reset time."
  @callback rate_limit_status(state :: term()) ::
              %{remaining: non_neg_integer(), reset_at: DateTime.t() | nil, limited: boolean()}

  @doc "Clean shutdown."
  @callback disconnect(state :: term()) :: :ok
end
```

This is the only extension point. Want a new plugin? Implement the behaviour, add to config.

### Trade-offs

| Gain | Cost |
|------|------|
| Uniform interface for all external agents | Each plugin must implement 6 callbacks |
| Direct rate limit visibility (HTTP headers) | Must implement protocol-specific parsing per plugin |
| OTP supervision (auto-restart, health checks) | More Elixir code vs shelling out to CLI |
| Capability-based routing | PluginManager must track and query capabilities |
| Provider fallback across plugins | Must maintain multiple adapter implementations |
| Eliminates cron-based monitoring entirely | Initial build effort for monitoring infrastructure |

---

## Scope

### In Scope
- `Plugins.Behaviour` — the behaviour contract above
- `ClaudeCodePlugin` — Anthropic Messages API with streaming and tool use
- `CodexPlugin` — OpenAI API with tool use
- `PluginManager` — Pool management, dispatch, capability routing, load balancing
- `Worker` — GenServer running one task loop against any plugin
- `TaskQueue` — Priority queue with optional Linear integration
- `ProgressTracker` — Real-time output monitoring, stuck/loop detection
- `RetryScheduler` — Rate-limit-aware retry with `Retry-After` parsing, provider fallback
- `QualityGate` — Post-task validation (test, lint, smoke test)
- Context injection — project files, recent state fed into system prompt

### Out of Scope
- Plugin marketplace / registry / hot-install
- Plugin version management
- Plugin-to-plugin communication (they go through the orchestrator)
- UI for plugin management
- New Rust tool implementations (use existing 37 tools)
- GUI/dashboard for monitoring

---

## Data Flow

### Component Interaction

```
                          ┌──────────────┐
                          │  Task Source  │
                          │ (Linear/CLI) │
                          └──────┬───────┘
                                 │
                                 ▼
┌─────────────────────────────────────────────────────────────────┐
│                      Plugin Subsystem                           │
│                                                                 │
│  ┌────────────┐    ┌──────────────────┐    ┌────────────────┐  │
│  │ TaskQueue   │───▶│ PluginManager    │───▶│ RetryScheduler │  │
│  │ (GenServer) │    │ (GenServer)      │    │ (GenServer)    │  │
│  └────────────┘    └───────┬──────────┘    └────────────────┘  │
│                            │ spawns/assigns                     │
│                  ┌─────────┼─────────┐                         │
│                  ▼         ▼         ▼                         │
│            ┌──────────┐ ┌──────────┐ ┌──────────┐             │
│            │ Worker 1 │ │ Worker 2 │ │ Worker N │             │
│            │(GenServer)│ │(GenServer)│ │(GenServer)│             │
│            └─────┬────┘ └─────┬────┘ └──────────┘             │
│                  │            │                                 │
│            ┌─────▼────────────▼────┐                           │
│            │   ProgressTracker     │                           │
│            │   (GenServer)         │                           │
│            └───────────────────────┘                           │
│                                                                 │
│            ┌───────────────────────┐                           │
│            │   QualityGate (mod)   │                           │
│            └───────────────────────┘                           │
└───────────────────────┬─────────────────────────────────────────┘
                        │
              ┌─────────┼──────────┐
              ▼                    ▼
    ┌──────────────────┐  ┌──────────────────┐
    │ Plugin A         │  │ Plugin B         │
    │ (e.g. Claude)    │  │ (e.g. Codex)     │
    │ implements       │  │ implements       │
    │ Behaviour        │  │ Behaviour        │
    └────────┬─────────┘  └────────┬─────────┘
             │                     │
             ▼                     ▼
    ┌──────────────────┐  ┌──────────────────┐
    │ External Service │  │ External Service │
    │ (Anthropic API)  │  │ (OpenAI API)     │
    └──────────────────┘  └──────────────────┘
              │
              │ tool_use blocks
              ▼
    ┌──────────────────┐
    │ RustBridge       │
    │ (existing)       │
    │ → Rust tools     │
    └──────────────────┘
```

### Single Task Lifecycle

```
1. TaskQueue receives task (Linear poll, CLI push, or orchestrator delegation)
       │
2. PluginManager finds plugins with matching capabilities
       │
3. PluginManager picks available Worker + plugin
       │
4. Worker builds context:
   ├── Read project files via RustBridge
   ├── Fetch recent state (git log, issue details, etc.)
   └── Assemble system prompt + user message
       │
5. Worker enters task loop:
   ┌──▶ Call plugin.execute(state, task) — streaming
   │    ├── ProgressTracker receives events in real-time
   │    ├── Parse for tool_use blocks (if applicable)
   │    │
   │    ▼
   │    Execute tools via RustBridge (if needed)
   │    ├── shell (run tests, commands)
   │    ├── file (read/write source files)
   │    ├── web (fetch docs, APIs)
   │    └── memory (project context)
   │    │
   │    ▼
   │    Send tool results back to plugin
   └────┤
        │ (loop until: task done, max iterations, or stuck detected)
        ▼
6. QualityGate runs (if configured for this capability type)
       │
7. Report results:
   ├── Update task source (Linear, etc.)
   ├── Post summary
   └── Notify user (via configured channel)
```

---

## User Scenarios

**Manual coding task.** User runs `rustyclaw plugin exec --capability coding --task "Fix auth bug in TEZ-250"`. PluginManager finds plugins with `coding` capability (ClaudeCodePlugin, CodexPlugin), picks the primary, assigns a Worker. Streaming progress appears in the terminal. On completion, results post to Linear.

**Batch queue from Linear.** User configures `plugins.task_sources.linear.auto_pull = true`. TaskQueue polls Linear for issues labeled `rustyclaw-ready`. Each issue's labels map to capabilities (`bug-fix` → `coding`, `research-needed` → `research`). PluginManager routes to the right plugin.

**Rate limit hit with fallback.** Worker using ClaudeCodePlugin receives a 429 with `Retry-After: 45`. RetryScheduler records this, pauses the Worker. PluginManager checks other plugins with the same capability — CodexPlugin also has `coding` — and reassigns. User sees: `[worker-1] Rate limited by claude-code. Retry in 45s. Switching to codex for queued tasks.`

**Stuck detection.** ProgressTracker notices a Worker has been generating the same file edit pattern 4 times (loop detection). Worker is interrupted with an intervention prompt. If the loop persists, the task is suspended and the user is notified.

**Future plugin — research agent.** A `ResearchPlugin` implements the behaviour with `capabilities: [:research, :web_search, :summarization]`. When an orchestrator agent needs background research, PluginManager routes to it. The research plugin connects to a search API, streams findings back. Same Worker lifecycle, same progress tracking, same retry logic — zero new infrastructure.

**Future plugin — browser automation.** A `BrowserPlugin` with `capabilities: [:browser_automation, :web_scraping, :testing]` wraps a headless browser service. Same `connect/execute/health/disconnect` lifecycle. PluginManager routes UI testing tasks to it alongside coding tasks to ClaudeCodePlugin — parallel Workers, shared monitoring.

---

## Agent Scenarios

**Autonomous issue processing.** PluginManager runs as a long-lived supervised process. With Linear integration, it autonomously pulls issues, maps capabilities, assigns Workers to matching plugins, monitors progress, runs quality gates, and updates Linear. Human intervention is only needed for: failed quality gates, stuck loops that self-correction can't resolve, and tasks requiring architectural decisions.

**Multi-worker coordination.** Two Workers operate concurrently — one running a coding task via ClaudeCodePlugin, another running a research task via a hypothetical ResearchPlugin. PluginManager tracks resource allocation per plugin (rate limits, concurrency caps). Workers on the same repo get file-conflict checking.

**Capability-based routing.** The orchestrator's AgentCoordinator asks "who can do coding?" PluginManager responds with all plugins that declared `coding` in their `capabilities/0`. The coordinator picks based on availability, rate limit status, and configured priority order.

**Provider fallback chain.** Configurable per capability: `coding → ["claude_code", "codex"]`. When the primary hits a rate limit, pending tasks automatically route to the next plugin with the same capability. When the primary recovers, new tasks resume on it.

---

## Edge Cases & Failure Modes

| Scenario | Detection | Response |
|----------|-----------|----------|
| **Plugin rate limit (429)** | `rate_limit_status/1` returns `limited: true` | Pause worker, schedule exact retry via `reset_at`, switch pending tasks to fallback plugin with same capability |
| **Plugin API error (5xx)** | HTTP status from `execute/2` | Exponential backoff (1s→2s→4s→8s, max 60s), max 5 retries, then suspend task |
| **Plugin adapter crash** | DynamicSupervisor detects process death | Auto-restart via OTP. Reconnect via `connect/1`. Resume from last checkpointed state. Max 3 restarts per 5 min per worker. |
| **Plugin health degraded** | `health/1` returns `:degraded` or `:unhealthy` | PluginManager stops routing new tasks. Existing tasks complete or timeout. If `:unhealthy`, call `disconnect/1` and attempt `connect/1`. |
| **Capability mismatch** | `capabilities/0` returns no match for requested task | PluginManager returns `{:error, :no_capable_plugin}`. Task stays queued. User notified: "No plugin available for capability :X" |
| **All plugins for a capability exhausted** | All matching plugins rate-limited or unhealthy | Task queued with retry. User notified with ETA based on earliest `reset_at` across plugins. |
| **Stuck in loop** | ProgressTracker: same pattern repeated 3+ times, or no progress for 10 min | Inject intervention. If loop persists, suspend task with diagnostic dump. |
| **Partial completion** | Worker dies mid-task after some work done | On restart, Worker reads checkpoint state (git diff, etc.) to understand partial progress. Resumes, not restarts. |
| **Context window exhaustion** | Token count tracking per conversation turn | Auto-summarize conversation history approaching limit. Keep system prompt + recent turns, compress middle. |
| **connect/1 failure** | `connect/1` returns `{:error, reason}` | Plugin marked `:unhealthy`. Retry with backoff. Other plugins with same capability serve traffic. |
| **Credential expiry** | 401 from external service during `execute/2` | Suspend all workers for that plugin, notify user, wait for credential update. |
| **Slow plugin (timeout)** | `execute/2` exceeds configured timeout | Worker kills the call, records timeout. RetryScheduler may retry or fallback. |
| **Quality gate failure** | Test/lint exit codes | Worker gets failure output, attempts fix (up to 3 iterations). After 3, task suspended with diagnostic. |

---

## Error UX

Errors surface through two channels: CLI output (for interactive use) and task source comments (for autonomous operation).

**CLI output examples:**
```
[worker-1] ✗ Rate limited by claude-code (retry in 45s)
[worker-1] ↻ Switched to codex for TEZ-250
[worker-2] ⚠ Stuck loop detected on TEZ-251 — intervening
[worker-2] ✗ TEZ-251 suspended: test failures after 3 fix attempts
[manager]  ✗ claude-code plugin unhealthy — routing to fallback
[manager]  ✗ No plugin available for capability :browser_automation
```

**Task source comment on failure:**
```
## Task Suspended: TEZ-251

**Plugin:** claude-code
**Reason:** Test failures after 3 fix attempts
**Last error:** `cargo test` exit 1 — `test_auth_middleware` assertion failed
**Files modified:** src/auth/middleware.rs, src/auth/mod.rs
**Branch:** plugin/TEZ-251
**Action needed:** Review partial changes, fix test, re-assign with `rustyclaw plugin retry TEZ-251`
```

Errors never silently swallow. Every failure either self-recovers (with a log entry) or surfaces to the user with specific next steps.

---

## Security & Privacy

**Code execution boundaries.** Plugin Workers execute tools via RustBridge, which routes through Rust's existing security layer — policy enforcement, credential scrubbing, sandbox limits. No new execution paths bypass the security model.

**Credential handling.** API keys (per-plugin) stored in RustyClaw's existing secret management (`~/.rustyclaw/secrets/` or environment variables). Never logged, never included in task comments or agent context. Elixir processes access keys via Application config, not module attributes.

**Plugin isolation.** Each plugin runs in its own supervised process tree. A misbehaving plugin cannot affect other plugins or the core orchestrator. Resource limits (memory, request rate) are enforced per-plugin.

**Context boundaries.** System prompts include project files and task descriptions but never credentials, other users' data, or infrastructure details beyond the project scope.

**Git branch isolation.** Coding plugins operate on dedicated branches (`plugin/<task-id>`). Never push to main/master directly. Changes require quality gates before merge.

**Resource limits.** Per-worker: max 100 task loop iterations, max 60 min wall time, max tool calls per iteration configurable. Prevents runaway API costs.

---

## Performance Requirements

| Metric | Target | Rationale |
|--------|--------|-----------|
| Task pickup latency | < 5s from queue to worker start | Poll interval + dispatch + capability lookup |
| Streaming event display | < 100ms from plugin to user terminal | SSE parsing is synchronous per chunk |
| Rate limit response | < 1s to pause + reschedule | Read `rate_limit_status/1`, update state, notify scheduler |
| Stuck detection | < 5 min from loop onset | 3 repeated patterns OR 10 min no-change threshold |
| Quality gate execution | < 5 min for test + lint | Delegated to Rust tools; depends on project size |
| Concurrent workers | 3-5 default, configurable per plugin | Bounded by external service rate limits, not compute |
| Memory per worker | < 50 MB Elixir process | Conversation history is the main cost; auto-summarize keeps it bounded |
| Plugin health check | < 2s round-trip | `health/1` should be a lightweight probe |
| Capability lookup | < 1ms | In-memory ETS lookup against registered capabilities |

---

## Integration with Existing Systems

### Replaces: Cron-Based Agent Monitoring

The current system uses cron jobs that poll session history, grep logs for rate limits, and restart PTY processes. All replaced:

1. **OTP supervision** replaces health polling — DynamicSupervisor detects crashes instantly
2. **`rate_limit_status/1`** replaces log scraping — structured rate limit data from each plugin
3. **ProgressTracker** replaces cron-based stuck detection — streaming output monitored in real-time

### Integrates With: AgentCoordinator

```
AgentCoordinator (existing)
    │
    ├── queries PluginManager for capability matches
    │
    └── PluginManager responds with available plugins:
        PluginManager.plugins_for_capability(:coding)
        → [{ClaudeCodePlugin, :healthy}, {CodexPlugin, :healthy}]
```

PluginManager registers with AgentCoordinator as a capability provider. When any agent needs work done that matches a plugin capability, AgentCoordinator routes through PluginManager.

### Integrates With: Tool Synthesis

Plugins can trigger tool synthesis when they encounter a task requiring a capability no existing tool provides. This is the existing ToolSynthesis flow — no new integration needed, just ensure Workers have `synthesize_tool` in their tool list.

### Integrates With: RustBridge

Workers call Rust tools via the existing RustBridge HTTP interface. No new bridge endpoints needed — tasks use `shell`, `file`, `web`, and `memory` tools which are already bridged.

---

## File Layout

```
elixir/rustyclaw_orchestrator/lib/rustyclaw_orchestrator/
├── plugins/
│   ├── behaviour.ex            # The plugin behaviour contract (6 callbacks)
│   ├── manager.ex              # GenServer — pool management, dispatch, capability routing
│   ├── worker.ex               # GenServer — task loop against any plugin
│   ├── claude_code_plugin.ex   # Plugin: Anthropic Messages API + SSE streaming
│   ├── codex_plugin.ex         # Plugin: OpenAI API + SSE streaming
│   ├── task_queue.ex           # GenServer — priority queue, optional Linear integration
│   ├── progress_tracker.ex     # GenServer — streaming monitor, stuck/loop detection
│   ├── retry_scheduler.ex      # GenServer — rate limit state, backoff, plugin fallback
│   ├── quality_gate.ex         # Module — post-task validation (test, lint, review)
│   └── context_builder.ex      # Module — assembles system prompt from project files
```

---

## Config

```toml
[plugins]
enabled = false                             # Opt-in
max_workers = 3                             # Total concurrent workers across all plugins
max_iterations_per_task = 100               # Task loop cap
max_task_duration_minutes = 60              # Wall time cap
default_quality_gates = ["test", "lint"]    # Run after each task (capability-dependent)

# Capability → plugin priority. First available = primary.
[plugins.routing]
coding = ["claude_code", "codex"]
# research = ["research_agent"]             # Future: add as plugins are implemented
# browser_automation = ["browser_agent"]    # Future

[plugins.claude_code]
module = "RustyclawOrchestrator.Plugins.ClaudeCodePlugin"
api_key_env = "ANTHROPIC_API_KEY"
model = "claude-sonnet-4-5-20250514"
max_tokens = 16384
max_concurrent = 3

[plugins.codex]
module = "RustyclawOrchestrator.Plugins.CodexPlugin"
api_key_env = "OPENAI_API_KEY"
model = "codex-mini-latest"
max_concurrent = 2

[plugins.task_sources.linear]
enabled = false
api_key_env = "LINEAR_API_KEY"
project = ""
auto_pull = false
poll_interval_seconds = 300
ready_label = "rustyclaw-ready"
```

---

## Open Questions

1. **Context window management strategy.** When conversation history exceeds the model's context window, what gets summarized vs dropped? This applies to any LLM-backed plugin, not just coding. Need to benchmark quality trade-offs.

2. **Git workflow for concurrent coding plugins.** When two Workers operate on the same repo via different coding plugins, they get separate branches. But what about dependent tasks? Options: strict sequential, rebase chaining, or dependency detection via task source links.

3. **Cost tracking and budgets.** Should PluginManager enforce per-task or per-day token budgets? The Rust core has `src/cost/` for token tracking — should we extend it, or build Elixir-side tracking that works across all plugin types?

4. **Quality gate extensibility.** Current design hardcodes test + lint for coding tasks. Should quality gates be capability-dependent? Per-plugin? Custom scripts?

5. **Plugin-specific streaming protocols.** SSE, WebSocket, and polling are all valid streaming approaches. How much should the behaviour abstract vs leave to each plugin? Current design: `execute/2` returns a result, but streaming details are plugin-internal.

6. **State continuity across tasks.** Should a Worker that completed task A retain context when starting task B on the same plugin? Options: clean slate, carry project-level learnings, or shared memory via the existing memory system.

7. **Capability granularity.** Is `[:coding]` sufficient, or do we need `[:code_generation, :bug_fix, :refactoring, :test_writing]`? Finer granularity enables better routing but adds complexity to plugin registration.
