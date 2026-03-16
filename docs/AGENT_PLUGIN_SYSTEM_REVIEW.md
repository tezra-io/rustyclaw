# Agent Plugin System — Design Review

*Reviewer: Senior Engineer | Date: 2026-03-15 | Design Doc: AGENT_PLUGIN_SYSTEM_DESIGN.md*

---

## Executive Summary

The design correctly identifies critical pain points in the current PTY-based CLI spawning model and proposes a clean plugin behaviour contract to solve them. However, there are **three blocking architecture issues** that must be resolved before implementation: (1) the plugin behaviour's `execute/2` callback is synchronous but the design requires streaming, (2) the tool execution flow during plugin operation is undefined, and (3) context injection mechanism is unspecified. Additionally, the integration path with existing orchestration components (AgentCoordinator, agent definitions, cron workflows) needs explicit definition.

**Verdict: Approve with changes** — the core approach is sound, but the streaming interface, tool execution protocol, and integration boundaries must be specified before build starts.

---

## Blockers

### B1. Streaming Protocol Architecture Inconsistency

**Issue:** The `Behaviour.execute/2` callback is defined as:

```elixir
@callback execute(state :: term(), task :: map()) ::
            {:ok, result :: term(), new_state :: term()} | {:error, reason :: term()}
```

This is a **synchronous return value** (returns when complete). However, the design claims:

- "SSE parsing (Elixir)" in Tech Stack
- "streaming results" in the behaviour description
- "ProgressTracker receives events in real-time" in the task lifecycle
- "< 100ms from plugin to user terminal" in Performance Requirements

**Why it's a blocker:** Real-time progress monitoring is a core requirement to replace cron-based polling. A synchronous callback cannot emit events during execution — the Worker won't see anything until `execute/2` returns.

**Resolution required:** Choose one of:

**Option A (Callback-based):** Add a streaming callback parameter:
```elixir
@callback execute(state, task, event_handler :: (event :: term() -> :ok)) ::
            {:ok, result, new_state} | {:error, reason}
```
The plugin calls `event_handler.({:progress, text})` during execution. Worker forwards to ProgressTracker.

**Option B (Enumerable stream):** Return a lazy enumerable:
```elixir
@callback execute(state, task) :: Enumerable.t({:chunk, text} | {:done, result})
```
Worker consumes with `Enum.reduce_while` or `Stream.run`, forwarding chunks to ProgressTracker.

**Option C (GenServer cast-based):** Plugin is itself a GenServer. Worker sends `{:execute, task, reply_to}`, plugin sends `{:chunk, ...}` messages back to Worker pid.

**Recommendation:** Option A is the most Elixir-idiomatic and aligns with how RustBridge already handles async tasks via `Task.Supervisor.async_nolink`. It also matches the existing pattern in the ToolSynthesis subsystem.

---

### B2. Tool Execution Flow Undefined

**Issue:** The design states "Workers call Rust tools via the existing RustBridge" and the diagram shows tool_use blocks flowing from plugins to RustBridge. But the mechanism is unspecified:

1. Does the plugin return structured tool_use requests (like Anthropic's API), which the Worker then routes to RustBridge?
2. Or does the plugin call RustBridge directly during its execution loop?
3. If (1), what's the return protocol? Does Worker feed tool results back into the plugin for the next iteration?
4. If (2), how does Worker monitor what tools were called and enforce policy?

**Why it's a blocker:** This is the integration point between the plugin layer and the Rust core. Without specifying this protocol, we can't implement either the plugin adapters (ClaudeCodePlugin, CodexPlugin) or the Worker's task loop.

**Resolution required:** Define the tool execution protocol. Recommended approach based on existing RustBridge pattern:

```elixir
# Worker's task loop
def task_loop(plugin, state, task, iteration) do
  case plugin.execute(state, task, &handle_event/1) do
    {:ok, {:tool_use, tool_calls}, new_state} ->
      results = execute_tools_via_bridge(tool_calls)
      task_with_results = Map.put(task, :tool_results, results)
      task_loop(plugin, new_state, task_with_results, iteration + 1)

    {:ok, {:complete, result}, new_state} ->
      {:ok, result}

    {:error, reason} ->
      {:error, reason}
  end
end
```

This keeps the Worker as the orchestration layer (enforces iteration limits, policy, logging) and plugins as pure execution adapters.

---

### B3. Context Injection Mechanism Unspecified

**Issue:** The design lists "Context injection — project files, recent state fed into system prompt" as in-scope but doesn't specify:

1. Who builds the context? Worker? Plugin? A dedicated ContextBuilder module?
2. What context is included? The existing `AGENTS.md`, `SOUL.md`, `USER.md` bootstrap pattern? Or something plugin-specific?
3. How does context get to the plugin? As part of the task map? Embedded in the system prompt?
4. Does each plugin implement its own context assembly, or is there a shared abstraction?

**Why it's a blocker:** For coding plugins specifically, context is essential (current git branch, recent commits, relevant files). Without defining how context flows, we can't build `ClaudeCodePlugin` or `CodexPlugin`.

**Resolution required:** Specify the context injection pattern. Recommended approach aligned with existing architecture:

**Option 1 (Worker-managed):** Worker calls a `ContextBuilder` module before plugin execution:
```elixir
context = ContextBuilder.build(task, plugin_type: :coding)
task_with_context = Map.put(task, :context, context)
plugin.execute(state, task_with_context, &handle_event/1)
```

**Option 2 (Plugin callback):** Add `build_context/2` to the Behaviour:
```elixir
@callback build_context(task :: map(), opts :: keyword()) :: map()
```
Each plugin implements its own context logic. Worker calls it before `execute/2`.

**Recommendation:** Option 1. Context building is orchestration logic (reading project files, calling RustBridge for git info), not plugin-specific. Keeps plugins as thin adapters. The file layout already includes `context_builder.ex` — make it explicit.

---

### B4. Integration with Existing Orchestration Unclear

**Issue:** The design doesn't specify how PluginManager integrates with the existing AgentCoordinator. Questions:

1. When AgentCoordinator routes a task with capability `:coding`, does it call PluginManager or spawn a coding agent from `~/.rustyclaw/agents/*.md`?
2. Are plugins a replacement for agent definitions, or do they coexist?
3. How do existing agent definitions (which have capabilities) relate to plugin capabilities?
4. Does PluginManager register itself with AgentCoordinator as a capability provider (like the design suggests), and if so, what's the API?

**Why it's a blocker:** Without defining the integration point, we risk building two parallel routing systems that don't compose.

**Resolution required:** Define the integration contract. Recommended:

**AgentCoordinator queries PluginManager:**
```elixir
# In AgentCoordinator.find_matching_agents/2
def find_matching_agents(capabilities, state) do
  agent_matches = find_from_agent_definitions(capabilities, state)  # existing
  plugin_matches = PluginManager.plugins_for_capabilities(capabilities)  # new

  agent_matches ++ plugin_matches
end
```

PluginManager returns plugin names as if they were agent names. AgentCoordinator routes to them identically. This keeps the capability routing centralized and allows gradual migration (coding agents via plugins, other agents via definitions).

---

## Recommendations

### R1. Streaming Callback Should Follow Task.Supervisor Pattern

The existing RustBridge uses `Task.Supervisor.async_nolink` to avoid blocking the GenServer during HTTP calls. The same pattern should apply to plugin execution — the Worker should dispatch plugin execution to a supervised Task and receive streaming events via the event_handler callback. This keeps the Worker GenServer responsive and aligns with established patterns.

Reference: `rust_bridge.ex:92-97` shows the `async_nolink + pending map` pattern.

---

### R2. ProgressTracker Needs Event Schema and Detection Algorithm

The design mentions "stuck/loop detection" and "same pattern repeated 3+ times" but doesn't define:

1. What constitutes a "pattern"? Line-level diffs? Token n-grams? File edit sequences?
2. How does ProgressTracker receive events if `execute/2` is synchronous (blocked by B1)?
3. What's the state schema for tracking repetition?

**Recommendation:** Define the event schema and detection algorithm. Suggested:

```elixir
# Event types emitted by plugin via event_handler callback
{:chunk, text :: String.t()}               # Raw streaming output
{:tool_use, tool_name :: atom(), args}     # Tool call detected
{:tool_result, result :: term()}           # Tool execution completed
{:thinking, text :: String.t()}            # Model's reasoning (if available)
{:artifact, type :: atom(), content}       # Code block, file edit, etc.

# ProgressTracker state
%{
  recent_artifacts: [%{type: :file_edit, path: "...", content: "..."}],
  loop_detection: %{
    window_size: 3,
    similarity_threshold: 0.85
  }
}
```

Loop detection: Use Levenshtein distance or structural diff on the last N artifacts. If similarity > threshold for 3 consecutive artifacts of the same type, trigger intervention.

---

### R3. Quality Gates Should Be Plugin-Specific, Not Capability-Based

The config shows `default_quality_gates = ["test", "lint"]`, but these only make sense for coding plugins. A research plugin or browser automation plugin doesn't produce code to test.

**Recommendation:** Move quality gates into per-plugin config:

```toml
[plugins.claude_code]
quality_gates = ["test", "lint"]

[plugins.codex]
quality_gates = ["test"]

[plugins.research_agent]  # future
quality_gates = []        # no quality gates for research tasks
```

This keeps the configuration close to the plugin definition and avoids runtime checks for "does this capability need testing?"

---

### R4. Context Window Management Must Be Specified for MVP

The design lists "Context window management strategy" as an Open Question, but this is **required for MVP**. Without it, the first multi-turn coding task will hit the context limit and fail.

**Recommendation:** Implement a simple sliding window approach:

1. Worker tracks total tokens per conversation (sum of all turns).
2. When approaching limit (e.g., 90% of model's context window), trigger summarization:
   - Keep: system prompt (context), most recent user message, last 2 assistant turns
   - Summarize: middle turns into a concise summary via a separate LLM call
3. Feed summarized conversation back into the next `execute/2` call.

This is the same approach SwarmEx uses (observation #S1 in recent context mentions this pattern).

---

### R5. Git Workflow for Concurrent Workers Is Not an Open Question

The design lists "Git workflow for concurrent coding plugins" as an Open Question, but **two Workers on the same repo is a blocker scenario**, not a future consideration. This must be resolved.

**Recommendation:** Use the existing `ResourceLock` pattern from the Elixir orchestrator:

1. Workers acquire a lock on `{:repo, repo_path}` before executing coding tasks.
2. Lock is held for the duration of the task (checkout branch → edit → test → commit).
3. If a Worker can't acquire the lock, the task queues or fails fast with `{:error, :repo_locked}`.

Reference: `resource_lock.ex` already exists in the codebase. Apply it to coding plugins.

**Alternative (if true parallelism is required):** Workers on the same repo use separate worktrees (git worktree). Each Worker gets an isolated working directory. Merge conflicts are detected during PR creation, not during execution.

---

### R6. Rate Limit State Should Be Per-Plugin, Not Per-Worker

The design shows RetryScheduler as a separate GenServer but doesn't specify where rate limit state lives. If Worker tracks it, two Workers hitting the same plugin can both exceed the limit.

**Recommendation:** Rate limit state should be part of the plugin's GenServer state (if plugins are GenServers) or managed by PluginManager. When any Worker receives a 429 from a plugin, PluginManager marks that plugin as `:rate_limited` and stops routing new tasks to it until `reset_at`.

This matches how the existing `RustBridge` handles retries (observation #2356 in recent context).

---

### R7. ClaudeCodePlugin and CodexPlugin Should Share a Common Base Module

Both coding plugins will implement:
- SSE parsing
- Tool use block extraction
- Token counting
- Conversation history management

**Recommendation:** Create `Plugins.BaseLLMPlugin` with shared logic, then:

```elixir
defmodule ClaudeCodePlugin do
  use RustyclawOrchestrator.Plugins.BaseLLMPlugin,
    api_endpoint: "https://api.anthropic.com/v1/messages",
    headers_fn: &build_anthropic_headers/1
end
```

This avoids duplication and makes adding future LLM-based plugins easier.

---

### R8. TaskQueue Linear Integration Should Be Phase 2

The design includes Linear polling (`task_sources.linear.auto_pull`) in the core plugin system. This is **significant scope creep** for a first version.

**Recommendation:** Phase 1 should focus on:
- The plugin behaviour
- ClaudeCodePlugin + CodexPlugin
- Worker + ProgressTracker + QualityGate
- Manual task submission via CLI

Phase 2 adds:
- TaskQueue with Linear polling
- Auto-routing based on issue labels
- Batch processing

This keeps the MVP scoped to "replace PTY-based CLI spawning with direct API integration" without adding external service dependencies.

---

## Notes

### N1. File Layout Aligns with Existing Patterns

The proposed file structure under `elixir/rustyclaw_orchestrator/lib/rustyclaw_orchestrator/plugins/` follows the established pattern from `tool_synthesis/`. This is good — consistent structure makes the codebase easier to navigate.

---

### N2. Config Schema Follows Application.get_env Pattern

The TOML config structure matches how the existing RustBridge and ToolSynthesis subsystems are configured. Using `Application.get_env(:rustyclaw_orchestrator, :plugins)` during init is the standard OTP pattern. No issues here.

---

### N3. Security Model Correctly Delegates to Rust Layer

The design states "Plugin Workers execute tools via RustBridge, which routes through Rust's existing security layer — policy enforcement, credential scrubbing, sandbox limits." This is correct — no new execution paths bypass security. The plugin layer is pure orchestration; all privileged operations go through the Rust core.

---

### N4. OTP Supervision Follows DynamicSupervisor + Task.Supervisor Pattern

The component interaction diagram shows Workers supervised by PluginManager (presumably a DynamicSupervisor), with async tasks dispatched via Task.Supervisor. This matches the existing pattern:

- AgentSupervisor (DynamicSupervisor) → AgentServer (GenServer) → Task.Supervisor for delegation
- ToolSynthesis.Sandbox (Task.Supervisor) for sandboxed execution

Consistency is good. No deviation needed.

---

### N5. Performance Targets Are Realistic

The performance requirements table lists:
- Task pickup latency: < 5s
- Streaming event display: < 100ms
- Rate limit response: < 1s

These are achievable with the proposed architecture:
- ETS capability lookup is sub-millisecond
- SSE parsing via `Req` with streaming response is ~10-50ms per chunk
- GenServer state updates are microseconds

The bottleneck will be external API latency (Anthropic/OpenAI), which is outside our control. The targets focus on orchestration overhead, which is appropriate.

---

### N6. The Behaviour Contract Is Minimal and Well-Scoped

Six callbacks is the right size — enough to handle lifecycle (connect/disconnect), execution (execute), monitoring (health, rate_limit_status), and discovery (capabilities). Adding more would overcomplicate; removing any would lose essential functionality.

Compare to Elixir's `GenServer` behaviour (6 required callbacks) or Phoenix's `Channel` behaviour (5 callbacks). This is appropriately sized.

---

### N7. Error UX Examples Are Clear and Actionable

The CLI output examples and task source comment format are well-designed:
- Concise status indicators (`✗`, `↻`, `⚠`)
- Specific next steps ("Review partial changes, fix test, re-assign with `rustyclaw plugin retry TEZ-251`")
- Structured diagnostic info (plugin, reason, files modified, branch)

This follows the Rust core's error message philosophy (from CLAUDE.md: "Fail Fast: Explicit errors for unsupported states").

---

### N8. Cron Replacement Claims Are Valid

The design claims the plugin system eliminates cron-based monitoring by replacing:
- Health polling → OTP supervision (DynamicSupervisor detects crashes instantly)
- Log scraping for rate limits → `rate_limit_status/1` callback (structured data)
- Stuck detection polling → ProgressTracker with real-time events

These claims are **valid** if the streaming architecture (blocked by B1) is fixed. Once plugins can emit events during execution, ProgressTracker can detect stuck/loop conditions in real-time (< 5 min target) vs hourly cron checks (60 min blind spot).

---

### N9. Tool Synthesis Integration Is Correctly Scoped

The design mentions "Integrates With: Tool Synthesis" but correctly notes "no new integration needed" — synthesized tools are already available via the existing flow. Workers just need `synthesize_tool` in their tool list, and they can call `RustBridge.run_task("tool_synthesizer", ...)` if needed.

This is good separation of concerns. The plugin system doesn't need to know about tool synthesis internals.

---

### N10. The Problem Statement Is Accurate and Well-Motivated

The "Three layers of indirection" critique is correct. Current flow:
1. Orchestrator → SubAgentSession
2. SubAgentSession → PTY spawn of CLI process
3. CLI process → LLM API

Each boundary loses type safety and error context. The proposed plugin system eliminates layers 2-3 by calling LLM APIs directly from Elixir. This is a legitimate simplification.

---

## Risk Analysis

### What Breaks First?

**Most likely failure point:** Context window exhaustion on long-running tasks. Without the context management strategy (currently an Open Question), the first coding task with >10 tool calls will hit the limit. This is why R4 promotes it to a required feature for MVP.

**Second most likely:** Concurrent Workers on the same git repo creating conflicting commits. The design acknowledges this in Open Questions but doesn't propose a solution. R5 provides the mitigation (ResourceLock), which must be implemented before production use.

---

### What's Hardest to Change Later?

**Hardest to change:** The `Behaviour` callback signatures. Once plugins are implemented against the interface, changing it (e.g., adding streaming support after the fact) requires updating all adapters. This is why B1 (streaming protocol) must be resolved before build starts.

**Second hardest:** The tool execution protocol (B2). If Workers parse tool_use blocks from plugin responses, that protocol becomes baked into every plugin adapter. Changing it later means coordinated updates across ClaudeCodePlugin, CodexPlugin, and any future plugins.

---

### What's Underspecified?

1. **Worker state persistence** — if a Worker crashes mid-task, what state is checkpointed? The design mentions "partial completion" detection but doesn't specify what gets saved or how resumption works.

2. **Plugin-to-PluginManager communication** — the diagram shows PluginManager dispatching to Workers, but how does PluginManager know when a plugin's health degrades or rate limit status changes? Periodic polling? Event callbacks?

3. **Quality gate extensibility** — the design hardcodes test + lint, then asks "Should quality gates be capability-dependent?" in Open Questions. R3 resolves this, but it should be in the initial design.

---

## Testability

### Can This Be Tested?

**Yes.** The design is highly testable due to clean separation of concerns:

1. **Plugin adapters** can be tested in isolation with mock HTTP clients (existing pattern: ToolSynthesis.Synthesizer tests use a `bridge` function injection).

2. **Worker task loop** can be tested with a mock plugin that implements the Behaviour:
   ```elixir
   defmodule MockPlugin do
     @behaviour Plugins.Behaviour
     def execute(_state, _task, event_fn) do
       event_fn.({:chunk, "test output"})
       {:ok, {:complete, "done"}, %{}}
     end
   end
   ```

3. **ProgressTracker** can be tested by feeding it a sequence of events and asserting on stuck/loop detection.

4. **QualityGate** can be tested by running it against a fixture project with known failing tests.

5. **Integration tests** can use the existing RustBridge pattern — spawn the Rust binary as a subprocess, configure PluginManager to point at it, submit a task, assert on results.

Reference: `tool_synthesis/synthesizer_test.exs` shows the mock bridge + isolated GenServer testing pattern. Apply the same approach here.

---

## Integration with Existing Systems

### Does It Replace Cron Cleanly?

**Mostly, but migration path is undefined.** The design claims to eliminate cron-based monitoring, but doesn't specify:

1. Do existing cron jobs keep running during rollout, or are they turned off immediately?
2. If a cron job was managing a coding session (polling history, detecting stuck), how does that session migrate to a Worker?
3. Is there a "hybrid mode" where cron handles old sessions and plugins handle new ones?

**Recommendation:** Define a migration strategy:

**Phase 1 (parallel operation):**
- Plugins handle new tasks submitted via `rustyclaw plugin exec`
- Cron continues monitoring existing PTY-based sessions
- No breaking changes to existing workflows

**Phase 2 (transition):**
- Existing sessions are marked read-only
- New work routes through plugins exclusively
- Cron shifts to cleanup-only (e.g., pruning old logs)

**Phase 3 (deprecation):**
- Remove cron jobs
- Remove PTY-based spawning code
- Update CLAUDE.md to reflect plugin-only architecture

---

### Does It Integrate with Dev-Build Workflow?

**Yes, but needs explicit CLI commands.** The design shows `rustyclaw plugin exec --capability coding --task "..."` for manual invocation, which integrates with existing `rustyclaw` CLI patterns. Good.

**Missing:** How does this compose with `rustyclaw agent spawn` (existing agent lifecycle commands)? Are those deprecated in favor of `rustyclaw plugin exec`? Or do they coexist?

**Recommendation:** Keep agent commands for non-plugin agents. Add plugin commands as a separate namespace:

```bash
rustyclaw agent spawn researcher     # existing, uses agent definitions
rustyclaw plugin exec coding "..."   # new, uses plugin system
rustyclaw plugin list                # show available plugins + health status
rustyclaw plugin retry TEZ-250       # resume a failed task
```

This avoids breaking existing workflows while introducing the new model.

---

## Verdict: Approve with Changes

**Core approach is sound.** The plugin behaviour contract, OTP supervision, and direct API integration are the right architectural moves. The design correctly identifies the problems with PTY-based spawning and proposes a clean replacement.

**Changes required before build:**

1. **Resolve B1-B4 blockers** — streaming protocol, tool execution flow, context injection, and orchestration integration must be specified.
2. **Address R1-R7 recommendations** — particularly streaming callback pattern, context window management, git workflow, and rate limit coordination.
3. **Define migration strategy** — how existing cron-based workflows transition to the plugin model.

**Once addressed, this design is ready for implementation.**

---

*End of review. Next step: Address blockers and recommendations in a revised design doc (`AGENT_PLUGIN_SYSTEM_DESIGN_v2.md`) before entering implementation phase.*
