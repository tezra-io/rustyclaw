# Agent Plugin System — Design Document v2

*Branch: `feature/agent-plugin-system` | Revised: 2026-03-15*
*Addresses all blockers (B1-B4) and recommendations (R1-R8) from AGENT_PLUGIN_SYSTEM_REVIEW.md*

---

## Problem

RustyClaw runs external agents (Claude Code, Codex) by spawning sub-agent sessions that shell out to CLI processes via PTY. This is fragile in four ways:

1. **Three layers of indirection** (orchestrator → sub-agent session → CLI process) lose type safety and error context at each boundary.
2. **No rate limit visibility** — detection relies on grep-scraping logs for `429` patterns, missing the `Retry-After` header entirely.
3. **60-minute blind spots** — cron polling is the only progress monitoring. An agent can spin for 59 minutes before detection.
4. **No extensibility model** — adding a new agent type means copy-pasting shell-out logic with no shared interface.

---

## Tech Stack

| Component | Choice | Why |
|-----------|--------|-----|
| Orchestration runtime | Elixir/OTP | Already the orchestration layer. GenServers for lifecycle, DynamicSupervisor for crash recovery, ETS for state. |
| Plugin communication | Direct HTTP via `Req` | LLM APIs are HTTP-based. Direct calls give streaming, rate limit headers, and full OTP control. |
| Streaming | Callback-based (`event_handler` function) | Plugin calls `event_handler.({:chunk, text})` during execution. Worker forwards to ProgressTracker. |
| Tool execution | Worker-managed loop via RustBridge | Worker intercepts `tool_use` returns, calls Rust tools, feeds results back into the plugin. |
| State persistence | ETS + optional file flush | Follows the SubAgentSession pattern. |

**What we are NOT adding:**
- No plugin marketplace, registry, or hot-install — plugins are configured in app config
- No plugin-to-plugin communication — plugins interact through the orchestrator
- No Linear integration in MVP (Phase 2)

---

## Approach

Replace PTY-based CLI spawning with a generic plugin behaviour. Each plugin is a supervised Elixir module that connects to an external service, executes tasks with streaming callbacks, and reports health. The PluginManager handles pool management, dispatch, and routing. OTP handles lifecycle and crash recovery.

### The Plugin Behaviour

```elixir
defmodule RustyclawOrchestrator.Plugins.Behaviour do
  @doc "Establish connection to external agent/service."
  @callback connect(config :: map()) :: {:ok, state :: term()} | {:error, reason :: term()}

  @doc "Execute task with streaming event callbacks. Returns :tool_use for tool calls or :complete when done."
  @callback execute(state :: term(), task :: map(), event_handler :: (event :: term() -> :ok)) ::
              {:ok, {:tool_use, tool_calls :: list()}, new_state :: term()}
              | {:ok, {:complete, result :: term()}, new_state :: term()}
              | {:error, reason :: term()}

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

The `execute/3` callback (resolves B1) takes an `event_handler` function parameter. During execution, the plugin calls `event_handler.({:chunk, text})` for streaming progress. The return value signals either a `tool_use` request (Worker handles tool execution and loops back) or `complete` (task finished).

### Event Schema

Plugins emit events through the `event_handler` callback during `execute/3`:

```elixir
{:chunk, text :: String.t()}                          # Raw streaming output
{:tool_use, name :: String.t(), args :: map()}        # Tool call detected (informational — actual routing uses return value)
{:tool_result, result :: term()}                      # Tool execution completed
{:artifact, type :: atom(), content :: String.t()}    # Code block, file edit, etc.
```

All events forward to ProgressTracker for real-time monitoring and loop detection.

### BaseLLMPlugin (Shared Module)

Both `ClaudeCodePlugin` and `CodexPlugin` share common LLM integration logic via `BaseLLMPlugin`:

```elixir
defmodule RustyclawOrchestrator.Plugins.BaseLLMPlugin do
  @doc "Shared logic: SSE parsing, tool_use block extraction, token counting, conversation history."

  defmacro __using__(opts) do
    # Injects: parse_sse_stream/2, extract_tool_calls/1,
    #          count_tokens/1, manage_conversation/2
  end
end

defmodule RustyclawOrchestrator.Plugins.ClaudeCodePlugin do
  use RustyclawOrchestrator.Plugins.BaseLLMPlugin,
    api_base: "https://api.anthropic.com/v1/messages",
    auth_header: "x-api-key"
  # Plugin-specific: Anthropic message format, anthropic-version header
end

defmodule RustyclawOrchestrator.Plugins.CodexPlugin do
  use RustyclawOrchestrator.Plugins.BaseLLMPlugin,
    api_base: "https://api.openai.com/v1/responses",
    auth_header: "Authorization"
  # Plugin-specific: OpenAI message format, response parsing
end
```

This avoids duplicating SSE parsing, token counting, and conversation history management across LLM-backed plugins.

---

## Worker Task Loop

The Worker is the orchestration core (resolves B2). It manages the execute → tool_use → result → re-execute cycle:

```elixir
defmodule RustyclawOrchestrator.Plugins.Worker do
  # Dispatched via Task.Supervisor.async_nolink (R1)
  def run_task(plugin, plugin_state, task, opts) do
    context = ContextBuilder.build(task, plugin.capabilities())
    task_with_context = Map.put(task, :context, context)
    task_loop(plugin, plugin_state, task_with_context, _iteration = 0, opts)
  end

  defp task_loop(plugin, state, task, iteration, opts) when iteration < opts.max_iterations do
    event_handler = fn event -> ProgressTracker.record(opts.tracker, opts.worker_id, event) end

    case plugin.execute(state, task, event_handler) do
      {:ok, {:tool_use, tool_calls}, new_state} ->
        results = Enum.map(tool_calls, &execute_tool_via_bridge/1)
        task = Map.put(task, :tool_results, results)
        task_loop(plugin, new_state, task, iteration + 1, opts)

      {:ok, {:complete, result}, _new_state} ->
        run_quality_gates(result, opts)

      {:error, {:rate_limited, retry_after}} ->
        PluginManager.report_rate_limit(opts.plugin_name, retry_after)
        {:error, :rate_limited}

      {:error, reason} ->
        {:error, reason}
    end
  end

  defp task_loop(_plugin, _state, _task, iteration, _opts) do
    {:error, {:max_iterations, iteration}}
  end
end
```

Key design decisions:
- **Worker owns the loop**, not the plugin. This enforces iteration limits, policy, and tool call logging in one place.
- **Plugin returns structured `tool_use` lists**, not raw text. Worker routes to RustBridge, collects results, feeds them back as `task.tool_results` on the next iteration.
- **Execution dispatched via `Task.Supervisor.async_nolink`** (R1). The Worker GenServer remains responsive — it sends `{:execute, task, reply_to}` and receives results via `handle_info`. This matches the established RustBridge pattern.
- **Rate limits reported to PluginManager** (R6), not tracked per-Worker.

---

## Context Injection

Worker-managed `ContextBuilder` module assembles context before `execute/3` (resolves B3):

```elixir
defmodule RustyclawOrchestrator.Plugins.ContextBuilder do
  @doc "Builds context map for plugin execution. Reads project state via RustBridge."
  @spec build(task :: map(), capabilities :: [atom()]) :: map()
  def build(task, capabilities) do
    base = %{
      project_files: read_project_files(task.repo_path),
      git_state: fetch_git_state(task.repo_path),
      recent_commits: fetch_recent_commits(task.repo_path, limit: 10)
    }

    # Coding-specific context
    if :coding in capabilities do
      Map.merge(base, %{
        test_status: run_test_summary(task.repo_path),
        relevant_files: find_relevant_files(task)
      })
    else
      base
    end
  end
end
```

Context building is orchestration logic — it reads project files and calls RustBridge for git info. Plugins receive assembled context; they don't build it themselves.

### Context Window Management (R4)

Required for MVP. Worker tracks token usage per conversation and triggers summarization at 90% capacity:

1. Worker maintains `total_tokens` counter, updated after each `execute/3` call.
2. At 90% of the model's context window, Worker triggers summarization:
   - **Keep:** system prompt (context), most recent user message, last 2 assistant turns.
   - **Summarize:** middle turns into a concise summary via a separate LLM call.
3. Summarized conversation replaces full history in the next `execute/3` call.

Token limit is per-plugin in config (`max_context_tokens`). Default: model's published limit minus 20% headroom.

---

## Orchestration Integration

AgentCoordinator queries PluginManager via `plugins_for_capabilities/1` (resolves B4):

```elixir
# In AgentCoordinator.find_matching_agents/2
def find_matching_agents(capabilities, state) do
  agent_matches = find_from_agent_definitions(capabilities, state)    # existing
  plugin_matches = PluginManager.plugins_for_capabilities(capabilities) # new

  agent_matches ++ plugin_matches
end
```

PluginManager returns plugin names as agent identifiers. AgentCoordinator routes to them identically. This keeps capability routing centralized and allows gradual migration — coding tasks via plugins, other agents via definitions.

Plugins and agent definitions coexist. No agent definitions are deprecated. The `rustyclaw plugin exec` CLI command routes directly to PluginManager; the `rustyclaw agent spawn` command continues working for non-plugin agents.

---

## Loop Detection (R2)

ProgressTracker monitors event streams for stuck patterns:

```elixir
# ProgressTracker state per worker
%{
  recent_artifacts: :queue.new(),   # bounded queue, last N artifacts
  window_size: 5,
  similarity_threshold: 0.85
}
```

**Detection algorithm:** When an `{:artifact, type, content}` event arrives, ProgressTracker compares it against the last N artifacts of the same type using Levenshtein distance (normalized to 0.0–1.0). If similarity exceeds the threshold for 3 consecutive same-type artifacts, ProgressTracker notifies the Worker to inject an intervention prompt.

If the loop persists after intervention (3 more similar artifacts), the task is suspended with a diagnostic dump.

---

## Rate Limit Coordination (R6)

Rate limit state lives in PluginManager, not in individual Workers:

```elixir
# PluginManager state
%{
  plugins: %{
    "claude_code" => %{
      module: ClaudeCodePlugin,
      state: plugin_state,
      status: :healthy,           # :healthy | :degraded | :rate_limited | :unhealthy
      rate_limit: %{
        remaining: 42,
        reset_at: ~U[2026-03-15 21:30:00Z],
        limited: false
      }
    }
  }
}
```

When any Worker receives a 429 from a plugin, it calls `PluginManager.report_rate_limit(plugin_name, retry_after)`. PluginManager marks the plugin as `:rate_limited`, stops routing new tasks to it, and schedules a status check at `reset_at`. Pending tasks route to fallback plugins with the same capability.

---

## Git Concurrency (R5)

Workers acquire a `ResourceLock` on `{:repo, repo_path}` before executing coding tasks:

```elixir
case ResourceLock.acquire({:repo, task.repo_path}, worker_id, timeout: 5_000) do
  :ok -> execute_coding_task(task)
  {:error, :locked} -> {:error, :repo_locked}
end
```

Lock held for the full task duration (branch checkout → edit → test → commit). If a Worker can't acquire the lock, the task requeues.

**Alternative for true parallelism:** Workers use separate git worktrees (`git worktree add`), each getting an isolated working directory. Merge conflicts detected during PR creation, not during execution. This is configurable per-plugin:

```toml
[plugins.claude_code]
git_strategy = "worktree"   # "lock" (default) or "worktree"
```

---

## Scope

### MVP (Phase 1)
- `Plugins.Behaviour` — the 6-callback contract
- `BaseLLMPlugin` — shared SSE parsing, token counting, conversation history
- `ClaudeCodePlugin` — Anthropic Messages API adapter
- `CodexPlugin` — OpenAI API adapter
- `PluginManager` — pool management, dispatch, rate limit coordination, capability routing
- `Worker` — task loop with tool execution, context injection, context window management
- `ProgressTracker` — real-time event monitoring, loop detection
- `ContextBuilder` — project files, git state, recent commits
- `QualityGate` — post-task validation (per-plugin config)
- Manual task submission via `rustyclaw plugin exec` CLI

### Phase 2
- `TaskQueue` with Linear polling and auto-routing based on issue labels
- Batch processing
- Auto-routing from issue labels to capabilities

### Out of Scope
- Plugin marketplace / registry / hot-install
- Plugin version management
- Plugin-to-plugin communication
- UI for plugin management

---

## Data Flow

### Component Interaction

```
                          ┌──────────────┐
                          │  Task Source  │
                          │  (CLI only   │
                          │   in MVP)    │
                          └──────┬───────┘
                                 │
                                 ▼
┌─────────────────────────────────────────────────────────────────┐
│                      Plugin Subsystem                           │
│                                                                 │
│  ┌────────────────────────────────────────────────────────────┐ │
│  │ PluginManager (GenServer)                                  │ │
│  │ ├── plugin pool + health status                            │ │
│  │ ├── rate limit state (per-plugin)                          │ │
│  │ ├── capability routing                                     │ │
│  │ └── Worker dispatch via DynamicSupervisor                  │ │
│  └───────┬────────────────────────────────────────────────────┘ │
│          │ spawns Workers                                       │
│          ▼                                                      │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐         │
│  │ Worker 1     │  │ Worker 2     │  │ Worker N     │         │
│  │ (GenServer)  │  │ (GenServer)  │  │ (GenServer)  │         │
│  │              │  │              │  │              │         │
│  │ Dispatches   │  │              │  │              │         │
│  │ execute/3 to │  │              │  │              │         │
│  │ Task.Sup     │  │              │  │              │         │
│  └──────┬───────┘  └──────────────┘  └──────────────┘         │
│         │ event_handler forwards                                │
│         ▼                                                       │
│  ┌──────────────────────┐    ┌──────────────────────┐          │
│  │ ProgressTracker      │    │ ContextBuilder (mod) │          │
│  │ (GenServer)          │    │ builds context before │          │
│  │ loop detection,      │    │ each execute/3 call   │          │
│  │ artifact tracking    │    └──────────────────────┘          │
│  └──────────────────────┘                                       │
└──────────────────────┬──────────────────────────────────────────┘
                       │
             ┌─────────┼──────────┐
             ▼                    ▼
   ┌──────────────────┐  ┌──────────────────┐
   │ ClaudeCodePlugin │  │ CodexPlugin      │
   │ (BaseLLMPlugin)  │  │ (BaseLLMPlugin)  │
   └────────┬─────────┘  └────────┬─────────┘
            │                     │
            ▼                     ▼
   ┌──────────────────┐  ┌──────────────────┐
   │ Anthropic API    │  │ OpenAI API       │
   └──────────────────┘  └──────────────────┘
            │
            │ tool_use return value
            ▼
   ┌──────────────────┐
   │ RustBridge       │
   │ (existing)       │
   │ → Rust tools     │
   └──────────────────┘
```

### Single Task Lifecycle

```
1. CLI: `rustyclaw plugin exec --capability coding --task "Fix TEZ-250"`
       │
2. PluginManager.dispatch(capability, task)
   ├── Find plugins via plugins_for_capabilities(:coding)
   ├── Check rate_limit status, pick available plugin
   └── Spawn Worker via DynamicSupervisor
       │
3. Worker.run_task(plugin, state, task, opts)
   ├── ContextBuilder.build(task, [:coding])
   │   ├── Read project files via RustBridge
   │   ├── Fetch git state (branch, status, recent commits)
   │   └── Find relevant files for task
   └── Dispatch to Task.Supervisor.async_nolink
       │
4. Task loop (in supervised task):
   ┌──▶ plugin.execute(state, task, event_handler)
   │    ├── event_handler.({:chunk, "..."}) → ProgressTracker
   │    ├── event_handler.({:artifact, :file_edit, "..."}) → ProgressTracker
   │    │
   │    Return value:
   │    ├── {:ok, {:tool_use, [%{name: "shell", args: %{cmd: "cargo test"}}]}, new_state}
   │    │   │
   │    │   ▼
   │    │   Worker routes tool_calls to RustBridge
   │    │   Worker puts results in task.tool_results
   │    │   Worker checks iteration < max_iterations
   │    │   Worker checks ProgressTracker for loop detection
   │    │   Worker checks token count (summarize at 90%)
   │    │   │
   │    └───┘ (loop)
   │
   │    └── {:ok, {:complete, result}, _state}
   │        │
   │        ▼
5. QualityGate.run(result, plugin_config.quality_gates)
   ├── "test" → shell: cargo test
   ├── "lint" → shell: cargo clippy
   └── If fail: retry up to 3x, then suspend
       │
6. Report results via configured channel
```

---

## Edge Cases & Failure Modes

| Scenario | Detection | Response |
|----------|-----------|----------|
| **Rate limit (429)** | `execute/3` returns `{:error, {:rate_limited, retry_after}}` | PluginManager marks plugin `:rate_limited`, schedules check at `reset_at`, routes pending to fallback |
| **API error (5xx)** | HTTP status from plugin | Exponential backoff (1s→2s→4s→8s, max 60s), max 5 retries, then suspend |
| **Worker crash** | DynamicSupervisor detects death | Auto-restart via OTP. Resume from last checkpoint (git diff for partial progress). Max 3 restarts/5 min. |
| **Plugin unhealthy** | `health/1` returns `:unhealthy` | PluginManager stops routing. Call `disconnect/1`, attempt `connect/1` with backoff. |
| **Stuck loop** | ProgressTracker: Levenshtein similarity > 0.85 for 3 consecutive same-type artifacts | Inject intervention prompt. If persists after 3 more, suspend with diagnostic. |
| **Context window exhaustion** | Worker token counter hits 90% of limit | Summarize middle turns, keep system prompt + last 2 turns. Continue with compressed history. |
| **All plugins exhausted** | All matching plugins rate-limited or unhealthy | Task queued. User notified with ETA from earliest `reset_at`. |
| **Git repo locked** | `ResourceLock.acquire` returns `{:error, :locked}` | Task requeues. Worker picks next available task. |
| **Credential expiry (401)** | HTTP 401 during `execute/3` | Suspend all workers for plugin. Notify user. Wait for credential update. |
| **Quality gate failure** | Test/lint exit codes | Worker feeds failure output back to plugin (up to 3 fix iterations), then suspends. |
| **Partial completion** | Worker dies mid-task | On restart, Worker reads checkpoint (git diff, last iteration) to understand partial progress. Resumes, not restarts. |

---

## Security & Privacy

**Code execution boundaries.** Workers execute tools via RustBridge → Rust security layer (policy enforcement, credential scrubbing, sandbox limits). No new execution paths bypass the security model.

**Credential handling.** Per-plugin API keys stored in RustyClaw's existing secret management (`~/.rustyclaw/secrets/` or env vars). Never logged, never in task comments or agent context. Accessed via `Application.get_env`, not module attributes.

**Plugin isolation.** Each plugin runs in its own supervised process tree. Resource limits (memory, request rate) enforced per-plugin in PluginManager.

**Git branch isolation.** Coding plugins operate on dedicated branches (`plugin/<task-id>`). Never push to main/master directly. Quality gates required before merge.

**Resource limits.** Per-worker: max iterations (default 100), max wall time (default 60 min), max tool calls per iteration (configurable). Prevents runaway API costs.

---

## Performance Requirements

| Metric | Target | Rationale |
|--------|--------|-----------|
| Task pickup latency | < 5s | Dispatch + capability lookup |
| Streaming event display | < 100ms | event_handler callback is synchronous per chunk |
| Rate limit response | < 1s | PluginManager state update + reroute |
| Stuck detection | < 5 min | Levenshtein on artifact window, checked per event |
| Concurrent workers | 3-5 default, per-plugin | Bounded by external rate limits |
| Memory per worker | < 50 MB | Auto-summarize keeps conversation bounded |
| Capability lookup | < 1ms | In-memory ETS |

---

## Config

```toml
[plugins]
enabled = false                             # Opt-in
max_workers = 3                             # Total concurrent workers

# Capability → plugin priority (first available = primary)
[plugins.routing]
coding = ["claude_code", "codex"]

[plugins.claude_code]
module = "RustyclawOrchestrator.Plugins.ClaudeCodePlugin"
api_key_env = "ANTHROPIC_API_KEY"
model = "claude-sonnet-4-5-20250514"
max_tokens = 16384
max_concurrent = 3
max_iterations = 100
max_duration_minutes = 60
max_context_tokens = 180000               # Summarize at 90% of this
quality_gates = ["test", "lint"]          # Per-plugin, not global
git_strategy = "lock"                     # "lock" or "worktree"

[plugins.codex]
module = "RustyclawOrchestrator.Plugins.CodexPlugin"
api_key_env = "OPENAI_API_KEY"
model = "codex-mini-latest"
max_concurrent = 2
max_iterations = 100
max_duration_minutes = 60
max_context_tokens = 128000
quality_gates = ["test"]
git_strategy = "lock"
```

---

## File Layout

```
elixir/rustyclaw_orchestrator/lib/rustyclaw_orchestrator/
├── plugins/
│   ├── behaviour.ex            # Plugin behaviour (6 callbacks, execute/3 with event_handler)
│   ├── base_llm_plugin.ex      # Shared: SSE parsing, token counting, conversation history
│   ├── manager.ex              # GenServer: pool, dispatch, rate limits, capability routing
│   ├── worker.ex               # GenServer: task loop, tool execution, context management
│   ├── claude_code_plugin.ex   # Anthropic Messages API adapter (uses BaseLLMPlugin)
│   ├── codex_plugin.ex         # OpenAI API adapter (uses BaseLLMPlugin)
│   ├── progress_tracker.ex     # GenServer: event monitoring, Levenshtein loop detection
│   ├── context_builder.ex      # Module: project files, git state, relevant file discovery
│   └── quality_gate.ex         # Module: post-task validation (test, lint per plugin config)
```

---

## Migration Strategy

### Phase 1 — Parallel Operation

Plugins handle new tasks; existing cron continues for active sessions.

- New tasks submitted via `rustyclaw plugin exec` route through PluginManager → Worker → Plugin.
- Existing PTY-based sessions continue under cron monitoring. No changes to active workflows.
- Both systems coexist. No breaking changes.
- **Exit criteria:** 10+ tasks completed successfully via plugins with quality gates passing.

### Phase 2 — Transition

New work routes exclusively through plugins.

- Existing PTY-based sessions are marked read-only (finish but don't accept new tasks).
- All new coding tasks route through PluginManager.
- Cron shifts to cleanup-only (pruning old logs, closing stale sessions).
- TaskQueue with Linear polling added in this phase.
- **Exit criteria:** Zero active PTY-based sessions. All work flowing through plugins for 2+ weeks.

### Phase 3 — Deprecation

Remove legacy code.

- Remove cron-based agent monitoring jobs.
- Remove PTY-based spawning code from SubAgentSession.
- Remove shell-out log scraping for rate limit detection.
- Update CLAUDE.md and operational docs to reflect plugin-only architecture.
- **Exit criteria:** Clean removal. No references to PTY spawning in codebase.

### CLI Coexistence

```bash
rustyclaw agent spawn researcher      # Existing — uses agent definitions
rustyclaw plugin exec coding "..."    # New — uses plugin system
rustyclaw plugin list                 # Show plugins + health + rate limit status
rustyclaw plugin retry TEZ-250        # Resume a failed/suspended task
```

Agent commands remain for non-plugin agents. Plugin commands are a separate namespace.

---

## Integration with Existing Systems

### AgentCoordinator

PluginManager registers as a capability provider. AgentCoordinator's `find_matching_agents/2` queries both agent definitions and PluginManager, returning a unified list. Plugin names serve as agent identifiers in the routing table.

### RustBridge

Workers call Rust tools via the existing RustBridge HTTP interface. No new bridge endpoints needed. Tool calls use existing `shell`, `file`, `web`, and `memory` tools.

### Tool Synthesis

Workers include `synthesize_tool` in their tool list. When a task requires a capability no existing tool provides, the existing ToolSynthesis flow handles generation. No new integration needed.

### ResourceLock

Workers acquire `{:repo, path}` locks for coding tasks via the existing ResourceLock module. This prevents concurrent Workers from creating git conflicts on the same repo.

---

## Open Questions (Reduced)

1. **Cost tracking and budgets.** Should PluginManager enforce per-task or per-day token budgets? The Rust core has `src/cost/` — extend it or build Elixir-side tracking?

2. **State continuity across tasks.** Should a Worker that completed task A retain context for task B on the same plugin? Options: clean slate, carry project-level learnings, or shared memory via existing memory system.

3. **Capability granularity.** Is `[:coding]` sufficient, or do we need `[:code_generation, :bug_fix, :refactoring]`? Finer granularity enables better routing but adds complexity.
