# Option B: Rust Main Agent + Elixir Skill Spawner

**Status**: Approved  
**Author**: Sujeeth / Aira  
**Date**: 2026-03-30  
**Related**: `MAIN_AGENT_DESIGN.md` (memory tiers, skill journals — still valid)

---

## 1. Summary

Keep the Rust agent loop as the main agent. Add the ability for Rust to call Elixir when it needs multi-agent work — spawning, supervising, and collecting results from ephemeral skill agents.

**Rust = brain + body. Elixir = workforce manager.**

No routing changes. No message forwarding. Normal messages flow exactly as they do today.

---

## 2. Architecture

```
User → Channel → Rust Gateway → Rust Agent Loop (main agent, unchanged)
                                    │
                                    ├── Simple query → handles directly (no Elixir)
                                    │
                                    └── Needs delegation → POST /api/skills/invoke
                                          │
                                          ▼
                                    Elixir AgentSupervisor
                                      ├── Spawns ephemeral AgentServer(s)
                                      ├── Each calls RustBridge POST /api/agent/run
                                      │   (own LLM call, own tools, isolated context)
                                      ├── Supervises lifecycle (crash recovery, timeout)
                                      └── Returns result(s) to Rust via callback
                                          │
                                          ▼
                                    Rust Agent Loop receives result
                                    → Incorporates into conversation
                                    → Responds to user via channel
```

### What stays the same (everything)

- Rust gateway: axum, webhooks, TLS, auth
- Rust channels: Telegram, WhatsApp, Discord, Signal, IRC, etc.
- Rust agent loop: LLM calls, tools, memory, conversation history
- Rust `process_message`: the main execution path
- Rust `ConversationStore`: per-chat history keyed by `{channel}_{sender}`
- `/btw` routing to Elixir: unchanged

### What's new

- Rust gets a new tool: `invoke_skill` — calls Elixir to spawn skill agents
- Elixir gets a new endpoint: `POST /api/skills/invoke` — spawns and manages skill runs
- Elixir `AgentServer.run_task` gets wired to RustBridge (TEZ-146)
- Elixir returns results via `POST /api/skills/result` callback to Rust

---

## 3. Message Flow

### 3.1 Normal Query (no change)

```
User: "What's the weather?"
  → Channel → Rust agent loop → LLM → response → channel
  (Elixir not involved at all)
```

### 3.2 Multi-Agent Delegation (new)

```
User: "Deploy staging and check the sprint"
  → Channel → Rust agent loop → LLM decides to delegate
  → LLM calls invoke_skill tool twice:
      invoke_skill("coding-skill", "deploy staging")
      invoke_skill("pm-skill", "check Linear sprint")
  → Rust invoke_skill tool → POST /api/skills/invoke to Elixir
  → Elixir spawns 2 ephemeral AgentServers
  → Each calls RustBridge POST /api/agent/run (independent LLM calls)
  → Each completes → Elixir collects results
  → Elixir POSTs results back: POST /api/skills/result to Rust
  → Rust invoke_skill tool returns combined output to agent loop
  → LLM synthesizes → response → channel
```

### 3.3 Parallel Skill Execution (new)

```
User: "Run these 10 refactoring tasks"
  → Rust agent loop → LLM calls invoke_skill_batch
  → POST /api/skills/invoke with batch=true, concurrency=5
  → Elixir spawns 5 AgentServers (BEAM processes, ~2KB each)
  → Each works independently via RustBridge
  → As slots free up, next tasks start
  → Elixir collects all results → POST /api/skills/result
  → Rust agent loop gets summary → responds to user
```

### 3.4 Cron Skill (new)

```
Cron trigger → Elixir spawns ephemeral skill agent
  → Agent calls RustBridge for LLM work
  → Agent writes journal
  → Elixir POSTs result to Rust: POST /api/skills/result
  → Rust stores in memory / optionally notifies user via channel
```

---

## 4. New Components

### 4.1 Rust: `invoke_skill` Tool

A new tool in the Rust tools registry that the LLM can call:

```rust
pub struct InvokeSkillTool {
    elixir_base_url: String,
    client: reqwest::Client,
}

// Tool definition for LLM:
// name: "invoke_skill"
// params: { skill: string, task: string, context: string, timeout_secs: int }
// 
// The tool:
// 1. POSTs to Elixir /api/skills/invoke
// 2. Waits for result (sync HTTP call with long timeout)
//    OR registers a callback and yields (async pattern)
// 3. Returns skill output as tool result to agent loop
```

**Sync vs Async execution:**
- **Sync (simpler, Phase 1):** `invoke_skill` blocks on the HTTP call. Elixir runs the skill, returns result in the same HTTP response. Works for single skills. Timeout: 5 minutes.
- **Async (Phase 2):** `invoke_skill` registers a callback ID, returns immediately with "skill running". Elixir POSTs result when done. Needed for parallel/batch execution.

### 4.2 Elixir: `POST /api/skills/invoke`

New endpoint in Elixir's API router:

```elixir
post "/api/skills/invoke" do
  skill_name = conn.body_params["skill"]
  task = conn.body_params["task"]
  context = conn.body_params["context"]    # optional gist/context from main agent
  timeout = conn.body_params["timeout_ms"] || 300_000

  # Load skill template from ~/.rustyclaw/workspace/skills/{skill_name}.md
  {:ok, definition} = SkillRegistry.load(skill_name)

  # Spawn ephemeral agent
  {:ok, pid} = AgentSupervisor.spawn_agent(definition, parent_pid: self())

  # Execute task via the agent (which calls RustBridge internally)
  case AgentServer.run_task(definition.name, task) do
    {:ok, result} ->
      # Agent dies after this (temporary restart strategy)
      json_response(conn, 200, %{ok: true, result: result})

    {:error, reason} ->
      json_response(conn, 500, %{ok: false, error: inspect(reason)})
  end
end
```

### 4.3 Elixir: Wire AgentServer.run_task → RustBridge (TEZ-146)

The one critical missing piece. Replace the placeholder:

```elixir
def handle_call({:run_task, task, provenance}, from, state) do
  # Build the task with any skill-specific context
  full_task = build_task_with_context(task, state)

  task_ref = Task.Supervisor.async_nolink(@task_supervisor, fn ->
    RustBridge.run_task(state.definition.name, full_task,
      provenance: provenance,
      model: state.definition.model,
      temperature: state.definition.temperature
    )
  end)

  {:noreply, %{state |
    status: :running,
    pending_task: {from, task_ref},
    last_active_at: DateTime.utc_now()
  }}
end

def handle_info({ref, result}, %{pending_task: {from, %Task{ref: ^ref}}} = state) do
  Process.demonitor(ref, [:flush])
  GenServer.reply(from, result)

  {:noreply, %{state |
    status: :idle,
    pending_task: nil
  } |> append_history(:task_completed, result)}
end

def handle_info({:DOWN, _ref, :process, _pid, reason}, %{pending_task: {from, _}} = state) do
  GenServer.reply(from, {:error, {:task_crashed, reason}})

  {:noreply, %{state |
    status: :idle,
    pending_task: nil
  } |> append_history(:task_failed, reason)}
end
```

### 4.4 Elixir: Skill Registry

Loads skill templates from the filesystem:

```elixir
defmodule RustyclawOrchestrator.SkillRegistry do
  @skills_dir "~/.rustyclaw/workspace/skills"

  def load(skill_name) do
    path = Path.join(Path.expand(@skills_dir), "#{skill_name}.md")
    AgentDefinition.from_file(path)
  end

  def list do
    @skills_dir
    |> Path.expand()
    |> File.ls!()
    |> Enum.filter(&String.ends_with?(&1, ".md"))
    |> Enum.map(&String.trim_trailing(&1, ".md"))
  end
end
```

### 4.5 Skill Template Format

Reuses existing AgentDefinition format:

```yaml
---
name: coding-skill
persistent: false
model: anthropic/claude-sonnet-4-20250514
capabilities: ["code", "shell", "file_operations", "git"]
allowed_tools: ["shell", "file_read", "file_write", "file_edit"]
memory: isolated
---
You are a coding agent. You receive a task, execute it thoroughly,
and return a clear summary of what you did and what changed.
```

---

## 5. Changes Required

### 5.1 Rust (~80 lines new, ~0 lines changed in existing code)

| Component | What | Size |
|-----------|------|------|
| `src/tools/invoke_skill.rs` | New tool: invoke_skill | ~60 lines |
| `src/tools/mod.rs` | Register invoke_skill in tools registry | ~5 lines |
| `src/config/schema.rs` | Add `elixir_base_url` config field (optional) | ~10 lines |

**Zero changes to existing Rust code.** Gateway, channels, agent loop, memory — untouched.

### 5.2 Elixir (~200 lines)

| Component | What | Size |
|-----------|------|------|
| `agent_server.ex` | Wire run_task → RustBridge (TEZ-146) | ~40 lines |
| `api_router.ex` | Add `POST /api/skills/invoke` endpoint | ~40 lines |
| `skill_registry.ex` | New: load skill templates from filesystem | ~50 lines |
| `agent_supervisor.ex` | Ensure `:temporary` restart for skill agents | ~10 lines |
| Skill template files | `~/.rustyclaw/workspace/skills/*.md` | ~20 lines each |

### 5.3 Configuration

```toml
# Optional — defaults to auto-discovery via resolve_synth_port()
[elixir]
base_url = "http://127.0.0.1:{synth_port}"
```

---

## 6. What This Does NOT Change

- **Message routing** — all messages stay in Rust, no forwarding
- **Conversation history** — stays in Rust ConversationStore, per-chat
- **Memory** — stays in Rust memory backends
- **Channels** — stay in Rust, no channel_send_inner changes needed
- **BtwRouter** — stays as-is for `/btw` side channel
- **Gateway** — no new endpoints in Rust gateway (invoke_skill is a tool, not an API)
- **Agent loop** — unchanged, just gets a new tool available

---

## 7. Implementation Order

### Phase 1: Skill Invocation MVP
1. Wire AgentServer.run_task → RustBridge (TEZ-146)
2. Create SkillRegistry (load templates from filesystem)
3. Create `POST /api/skills/invoke` endpoint in Elixir
4. Create `invoke_skill` tool in Rust
5. Create 1-2 skill templates (coding-skill, research-skill)
6. Test: user message → LLM calls invoke_skill → Elixir spawns agent → RustBridge executes → result returns

**Result:** Rust main agent can delegate to ephemeral Elixir-supervised skills.

### Phase 2: Parallel Execution
7. Add batch mode to `/api/skills/invoke` (concurrency limit)
8. Add async callback pattern (for non-blocking batch execution)
9. Git worktree isolation for parallel coding skills

**Result:** Multiple skills run concurrently under Elixir supervision.

### Phase 3: Skill Journals
10. Journal writing on skill completion (`~/.rustyclaw/workspace/journals/`)
11. Cron skills load previous journal for continuity
12. Main agent can read journals via file tools

**Result:** Skills have memory across runs without persistence.

### Phase 4: Memory Integration (from MAIN_AGENT_DESIGN.md)
13. Tier 1 gist generation (Hermes)
14. Gist injection into main agent context
15. Compaction triggers gist update

**Result:** Main agent has compacting long-term memory.

---

## 8. Why This Is Better Than "Elixir as Brain"

| Concern | Elixir-as-Brain (wrong doc) | Rust-as-Brain + Elixir Skills (this doc) |
|---------|---------------------------|------------------------------------------|
| Normal message latency | +HTTP round trip to Elixir | Zero overhead |
| Conversation history | Must transfer to Elixir | Stays in Rust (already works) |
| Existing code changes | ~50 lines Rust + routing rewrite | Zero changes to existing Rust |
| Failure mode | Elixir down = no message processing | Elixir down = no delegation (messages still work) |
| Complexity | New conversation state management | Just a new tool + endpoint |
| Blockers | 3 (TEZ-146, routing, identity) | 1 (TEZ-146) |

---

## 9. Future Path to Option A

If time becomes unlimited and a full Elixir gateway is desired:
- This design is forward-compatible
- Elixir skill infrastructure stays exactly as-is
- Just add message routing on top (the wrong doc's approach, but on top of working skills)
- The skill layer must work first regardless of where the main agent lives

---

## 10. Related Documents

- `docs/MAIN_AGENT_DESIGN.md` — Memory tiers, skill journals, parallel execution details (still valid, complements this doc)
- `docs/OPTION_B_REVIEW.md` — Review of the previous (incorrect) version of this doc
- `docs/ELIXIR_ORCHESTRATION_DESIGN.md` — Elixir layer architecture
- `docs/hermes-memory-design.md` — Memory extraction and consolidation
